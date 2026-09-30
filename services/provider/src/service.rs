//! The sealed endpoint: gateway checks, engine forwarding, streaming and receipts (spec
//! `market/provider-agent`).

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use ac_crypto::kem::KemSecretKey;
use ac_crypto::sealed::{Acceptor, ReplayCache, recipient_id};
use ac_crypto::sig::SigningKey;
use ac_crypto::{KemPublicKey, OsRng};
use ac_market_proto::openai::{SseDecoder, SseEvent, has_content, usage_of};
use ac_market_proto::{ChatRequest, ErrorCode, ProviderMsg, ProviderReq, Usage, msg};
use ac_primitives::market::receipt::{RECEIPT_CONTEXT, ReceiptContext, check_receipt, fee_for};
use ac_primitives::market::voucher::key_fingerprint;
use ac_primitives::market::work::JobKind;
use ac_primitives::market::{ModelId, PricePerMTok, ReceiptBody, SignedReceipt};
use ac_wallet::http::{self, Client, Request, Response};
use ac_wallet::sealed_http::{self, SealedWriter, now_secs};
use anyhow::{Context, Result};
use sp_core::H256;
use sp_runtime::AccountId32;

use crate::chain::{Cached, Directory};
use crate::logging::TARGET;
use crate::store::Store;

/// Largest request body accepted.
pub const MAX_BODY: usize = 32 * 1024 * 1024;

/// A model the provider serves: its engine name and on-chain price.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelEntry {
    /// Name of the model in the local engine.
    pub engine_name: String,
    /// On-chain price.
    pub price: PricePerMTok,
}

/// Static configuration of the service.
pub struct Config {
    /// The provider account.
    pub account: AccountId32,
    /// Its current key (receipts are signed with it).
    pub key: SigningKey,
    /// The X-Wing secret key registered on chain.
    pub kem: KemSecretKey,
    /// Its encapsulation key.
    pub kem_public: KemPublicKey,
    /// Genesis hash of the chain.
    pub genesis: H256,
    /// Served models.
    pub models: BTreeMap<ModelId, ModelEntry>,
    /// Engine base URL.
    pub engine: String,
    /// Receipt directory.
    pub store_dir: PathBuf,
}

/// The running service.
pub struct Service {
    account: AccountId32,
    key: SigningKey,
    key_fingerprint: [u8; 32],
    kem: KemSecretKey,
    recipient: [u8; 32],
    genesis: H256,
    models: RwLock<BTreeMap<ModelId, ModelEntry>>,
    engine: String,
    http: Client,
    directory: Cached<Box<dyn Directory>>,
    replay: Mutex<ReplayCache>,
    store: Store,
    block: AtomicU64,
}

impl Service {
    /// Builds the service.
    ///
    /// # Errors
    ///
    /// Key or store failures.
    pub fn new(config: Config, directory: Box<dyn Directory>) -> Result<Arc<Self>> {
        let public = config.key.public_key()?;
        Ok(Arc::new(Self {
            account: config.account,
            key_fingerprint: key_fingerprint(&public),
            key: config.key,
            recipient: recipient_id(&config.kem_public)?,
            kem: config.kem,
            genesis: config.genesis,
            models: RwLock::new(config.models),
            engine: config.engine,
            http: Client::new()?,
            directory: Cached::new(directory, Duration::from_secs(12)),
            replay: Mutex::new(ReplayCache::default()),
            store: Store::open(&config.store_dir)?,
            block: AtomicU64::new(0),
        }))
    }

    /// Replaces the served models (after a price change on chain).
    pub fn set_models(&self, models: BTreeMap<ModelId, ModelEntry>) {
        if let Ok(mut m) = self.models.write() {
            *m = models;
        }
    }

    /// Records the latest block (receipts are stamped with it).
    pub fn set_block(&self, block: u64) {
        self.block.store(block, Ordering::Relaxed);
    }

    /// The receipt store.
    #[must_use]
    pub const fn store(&self) -> &Store {
        &self.store
    }

    /// Handles one HTTP request.
    pub async fn handle(self: Arc<Self>, req: Request) -> Response {
        match (req.method().as_str(), req.uri().path()) {
            ("GET", "/health") => return http::full(200, "text/plain", "ok"),
            ("POST", sealed_http::PATH) => {}
            _ => return http::full(404, "text/plain", "not found"),
        }
        let started = Instant::now();
        let Ok(body) = http::read_body(req.into_body(), MAX_BODY).await else {
            return http::full(400, "text/plain", "unreadable body");
        };
        let handshake = match sealed_http::handshake_of(&body) {
            Ok(h) => h,
            Err(r) => return r.response(),
        };
        let sender = sealed_http::to_account32(&handshake.handshake.sender);
        let view = match self.directory.gateway(&sender).await {
            Ok(Some(v)) => v,
            Ok(None) => return http::full(403, "text/plain", "sender is not a registered gateway"),
            Err(e) => {
                log::warn!(target: TARGET, "gateway lookup failed: {e:#}");
                return http::full(503, "text/plain", "chain unavailable");
            }
        };
        if !view.active {
            log::info!(target: TARGET, "refused a request from a non-active gateway");
            return http::full(403, "text/plain", "sender is not an active gateway");
        }
        let accepted = {
            let acceptor = Acceptor {
                secret: &self.kem,
                recipient: self.recipient,
                now: now_secs(),
            };
            let Ok(mut replay) = self.replay.lock() else {
                return http::full(500, "text/plain", "internal error");
            };
            sealed_http::accept(&body, &acceptor, |_| Some(view.key.clone()), &mut replay)
        };
        let request = match accepted {
            Ok(r) => r,
            Err(r) => {
                log::info!(target: TARGET, "refused a sealed request: {r:?}");
                return r.response();
            }
        };
        let (mut writer, response, gateway, message) = request.respond();
        let this = Arc::clone(&self);
        tokio::spawn(async move {
            match msg::decode::<ProviderReq>(&message) {
                Ok(ProviderReq::Infer {
                    request_id,
                    kind,
                    model,
                    request,
                }) => {
                    let job = Job {
                        gateway,
                        request_id,
                        kind,
                        model,
                        started,
                    };
                    this.infer(&job, &request, &mut writer).await;
                }
                Ok(ProviderReq::Cosigned(receipt)) => {
                    let reply = match this.cosigned(&gateway, &view.key, &receipt) {
                        Ok(()) => ProviderMsg::Ack,
                        Err(e) => {
                            log::warn!(target: TARGET, "rejected a co-signed receipt: {e:#}");
                            error(ErrorCode::BadRequest, "invalid co-signed receipt")
                        }
                    };
                    writer.send(&msg::encode(&reply), true).await;
                }
                Err(_) => {
                    writer
                        .send(
                            &msg::encode(&error(ErrorCode::BadRequest, "malformed message")),
                            true,
                        )
                        .await;
                }
            }
        });
        response
    }

    async fn infer(&self, job: &Job, request: &[u8], writer: &mut SealedWriter) {
        let id = short(&job.request_id);
        match self.run_engine(job, request, writer).await {
            Ok(Some((usage, fee, ttft, total))) => log::info!(
                target: TARGET,
                "served {id} model {} for gateway {}: {} + {} tokens, fee {} micro-USD, ttft {ttft} ms, total {total} ms",
                short(&job.model.0), short_account(&job.gateway), usage.prompt_tokens, usage.completion_tokens, fee
            ),
            Ok(None) => log::info!(target: TARGET, "request {id}: the gateway went away"),
            Err((code, reason)) => {
                log::warn!(target: TARGET, "request {id} failed: {} ({reason})", code.as_str());
                writer.send(&msg::encode(&error(code, reason)), true).await;
            }
        }
    }

    /// Streams the engine's answer; `Ok(None)` if the gateway disconnected.
    async fn run_engine(
        &self,
        job: &Job,
        request: &[u8],
        writer: &mut SealedWriter,
    ) -> Result<Option<(Usage, u128, u32, u32)>, (ErrorCode, &'static str)> {
        if job.kind != JobKind::Inference {
            return Err((ErrorCode::UnsupportedJob, "only inference is supported"));
        }
        let entry = self
            .models
            .read()
            .ok()
            .and_then(|m| m.get(&job.model).cloned())
            .ok_or((
                ErrorCode::ModelNotFound,
                "model not served by this provider",
            ))?;
        let mut chat = ChatRequest::parse(request)
            .map_err(|_| (ErrorCode::BadRequest, "malformed request"))?;
        chat.set_model(&entry.engine_name);
        chat.set_stream(true);
        let resp = self
            .http
            .post(
                &http::join(&self.engine, "/v1/chat/completions"),
                "application/json",
                chat.to_bytes(),
            )
            .await
            .map_err(|_| (ErrorCode::EngineFailed, "engine unreachable"))?;
        if !resp.status().is_success() {
            return Err((ErrorCode::EngineFailed, "engine returned an error status"));
        }
        let mut body = resp.into_body();
        let mut sse = SseDecoder::new();
        let (mut usage, mut ttft, mut done) = (None, None, false);
        while !done {
            let Some(bytes) = http::next_chunk(&mut body)
                .await
                .map_err(|_| (ErrorCode::EngineFailed, "engine stream broke"))?
            else {
                break;
            };
            for event in sse
                .push(&bytes)
                .map_err(|_| (ErrorCode::EngineFailed, "engine event too large"))?
            {
                match event {
                    SseEvent::Done => done = true,
                    SseEvent::Data(data) => {
                        if let Some(u) = usage_of(&data) {
                            usage = Some(u);
                        }
                        if ttft.is_none() && has_content(&data) {
                            ttft = Some(millis(job.started.elapsed()));
                        }
                        if !writer
                            .send(&msg::encode(&ProviderMsg::Delta(data)), false)
                            .await
                        {
                            return Ok(None);
                        }
                    }
                }
            }
        }
        let usage = match (done, usage) {
            (true, Some(u)) => u,
            _ => return Err((ErrorCode::EngineFailed, "engine stream ended without usage")),
        };
        let fee = fee_for(&entry.price, usage.prompt_tokens, usage.completion_tokens)
            .ok_or((ErrorCode::Internal, "fee overflow"))?;
        let total = millis(job.started.elapsed());
        let body = ReceiptBody {
            genesis: self.genesis,
            gateway: job.gateway.clone(),
            provider: self.account.clone(),
            kind: job.kind,
            model: job.model,
            request_id: job.request_id,
            in_tokens: usage.prompt_tokens,
            out_tokens: usage.completion_tokens,
            fee,
            // No proof in this phase: engine-side TOPLOC comes with the next change.
            toploc_commit: [0; 32],
            ttft_ms: ttft.unwrap_or(total),
            total_ms: total,
        };
        let (key, signature) = self
            .sign(&body)
            .map_err(|_| (ErrorCode::Internal, "signing failed"))?;
        let receipt = ProviderMsg::Receipt {
            body,
            key,
            signature,
            usage,
        };
        if !writer.send(&msg::encode(&receipt), true).await {
            return Ok(None);
        }
        Ok(Some((usage, fee.0, ttft.unwrap_or(total), total)))
    }

    fn sign(&self, body: &ReceiptBody) -> Result<(ac_crypto::PqPublicKey, ac_crypto::PqSignature)> {
        let payload = body.payload()?;
        let signature = self
            .key
            .sign(&payload, RECEIPT_CONTEXT, &mut OsRng::new()?)?;
        Ok((self.key.public_key()?, signature))
    }

    fn cosigned(
        &self,
        gateway: &AccountId32,
        gateway_key: &ac_crypto::PqPublicKey,
        receipt: &SignedReceipt,
    ) -> Result<()> {
        anyhow::ensure!(
            receipt.body.provider == self.account,
            "not this provider's receipt"
        );
        anyhow::ensure!(
            &receipt.body.gateway == gateway,
            "receipt of another gateway"
        );
        let price = self
            .models
            .read()
            .ok()
            .and_then(|m| m.get(&receipt.body.model).map(|e| e.price));
        check_receipt(
            receipt,
            &ReceiptContext {
                genesis: &self.genesis,
                provider_key: &self.key_fingerprint,
                gateway_key: &key_fingerprint(gateway_key),
                price: price.as_ref(),
            },
        )
        .map_err(|e| anyhow::anyhow!("{e}"))?;
        self.store
            .save(receipt, self.block.load(Ordering::Relaxed))
            .context("storing the receipt")
    }
}

struct Job {
    gateway: AccountId32,
    request_id: [u8; 32],
    kind: JobKind,
    model: ModelId,
    started: Instant,
}

fn error(code: ErrorCode, message: &str) -> ProviderMsg {
    ProviderMsg::Error {
        code,
        message: message.as_bytes().to_vec(),
    }
}

fn millis(d: Duration) -> u32 {
    u32::try_from(d.as_millis()).unwrap_or(u32::MAX)
}

fn short(bytes: &[u8]) -> String {
    hex::encode(bytes.get(..6).unwrap_or(bytes))
}

fn short_account(a: &AccountId32) -> String {
    let bytes: &[u8] = a.as_ref();
    short(bytes)
}
