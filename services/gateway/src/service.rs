//! The gateway's HTTP endpoints (spec `market/gateway-service`).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ac_crypto::kem::KemSecretKey;
use ac_crypto::sealed::{Acceptor, ReplayCache, recipient_id};
use ac_crypto::sig::SigningKey;
use ac_crypto::{KemPublicKey, OsRng, PqPublicKey};
use ac_market_proto::openai::{Assembler, has_content};
use ac_market_proto::{
    ChatRequest, ErrorCode, GatewayKey, GatewayMsg, Payment, ProviderMsg, ProviderReq, Usage,
    UserMsg, msg,
};
use ac_primitives::market::receipt::{RECEIPT_CONTEXT, ReceiptContext, check_receipt, fee_for};
use ac_primitives::market::voucher::key_fingerprint;
use ac_primitives::market::work::JobKind;
use ac_primitives::market::{ModelId, ReceiptBody, SignedReceipt, SignedVoucher};
use ac_wallet::http::{self, Client, Request, Response};
use ac_wallet::sealed_http::{self, SealedWriter, now_secs};
use anyhow::{Context, Result};
use serde_json::json;
use sp_core::H256;
use sp_runtime::AccountId32;

use crate::book::{Book, ChannelFacts};
use crate::chain::Chain;
use crate::logging::TARGET;
use crate::routing::{Choice, Router};

/// Largest request body accepted.
pub const MAX_BODY: usize = 32 * 1024 * 1024;
/// How long a provider that could not be reached is skipped.
pub const UNREACHABLE_PAUSE: Duration = Duration::from_secs(30);
/// How long a provider that signed an invalid receipt is skipped.
pub const INVALID_RECEIPT_PAUSE: Duration = Duration::from_secs(3600);
const KEY_CACHE: Duration = Duration::from_secs(12);

/// Static configuration.
pub struct Config {
    /// The gateway account.
    pub account: AccountId32,
    /// Its current key (receipts and handshakes to providers).
    pub key: SigningKey,
    /// The gateway's X-Wing key.
    pub kem: KemSecretKey,
    /// Its encapsulation key.
    pub kem_public: KemPublicKey,
    /// Genesis hash.
    pub genesis: H256,
    /// Output-token limit applied when a request gives none.
    pub max_output_tokens: u32,
    /// Ledger directory.
    pub data_dir: PathBuf,
}

/// The running gateway.
pub struct Gateway {
    account: AccountId32,
    key: Arc<SigningKey>,
    key_fingerprint: [u8; 32],
    kem: KemSecretKey,
    recipient: [u8; 32],
    genesis: H256,
    max_output_tokens: u32,
    announcement: Vec<u8>,
    chain: Arc<dyn Chain>,
    router: Arc<Router>,
    book: Book,
    replay: Mutex<ReplayCache>,
    keys: Mutex<HashMap<AccountId32, (Instant, Option<PqPublicKey>)>>,
    http: Client,
}

impl Gateway {
    /// Builds the gateway and signs its key announcement.
    ///
    /// # Errors
    ///
    /// Key, signing or ledger failures.
    pub fn new(config: Config, chain: Arc<dyn Chain>, router: Arc<Router>) -> Result<Arc<Self>> {
        let announcement = GatewayKey::sign(
            config.genesis,
            config.account.clone(),
            config.kem_public.clone(),
            &config.key,
            &mut OsRng::new()?,
        )?
        .to_json();
        Ok(Arc::new(Self {
            key_fingerprint: key_fingerprint(&config.key.public_key()?),
            account: config.account,
            key: Arc::new(config.key),
            recipient: recipient_id(&config.kem_public)?,
            kem: config.kem,
            genesis: config.genesis,
            max_output_tokens: config.max_output_tokens,
            announcement,
            chain,
            router,
            book: Book::open(&config.data_dir.join("channels"))?,
            replay: Mutex::new(ReplayCache::default()),
            keys: Mutex::new(HashMap::new()),
            http: Client::new()?,
        }))
    }

    /// The ledger.
    #[must_use]
    pub const fn book(&self) -> &Book {
        &self.book
    }

    /// The gateway account.
    #[must_use]
    pub const fn account(&self) -> &AccountId32 {
        &self.account
    }

    /// Handles one HTTP request.
    pub async fn handle(self: Arc<Self>, req: Request) -> Response {
        match (req.method().as_str(), req.uri().path()) {
            ("GET", "/ac/v1/key") => http::json(200, self.announcement.clone()),
            ("GET", "/ac/v1/models") => http::json(200, self.models_json()),
            ("GET", "/health") => http::full(200, "text/plain", "ok"),
            ("POST", sealed_http::PATH) => self.sealed(req).await,
            _ => http::full(404, "text/plain", "not found"),
        }
    }

    fn models_json(&self) -> Vec<u8> {
        let data: Vec<serde_json::Value> = self
            .router
            .models()
            .iter()
            .map(|m| {
                let prices: Vec<_> = m
                    .providers
                    .iter()
                    .filter_map(|(_, p)| p.models.iter().find(|x| x.model == m.id).map(|x| x.price))
                    .collect();
                let range = |f: fn(&ac_primitives::market::PricePerMTok) -> u128| {
                    let v: Vec<u128> = prices.iter().map(f).collect();
                    json!({ "min": v.iter().min(), "max": v.iter().max() })
                };
                json!({
                    "id": format!("0x{}", hex::encode(m.id.0)),
                    "object": "model",
                    "owned_by": "agentcoin",
                    "name": m.name,
                    "providers": m.providers.len(),
                    "input_micro_usd_per_mtok": range(|p| p.input.0),
                    "output_micro_usd_per_mtok": range(|p| p.output.0),
                })
            })
            .collect();
        json!({ "object": "list", "data": data })
            .to_string()
            .into_bytes()
    }

    async fn user_key(&self, who: &AccountId32) -> Result<Option<PqPublicKey>> {
        let hit = self.keys.lock().ok().and_then(|k| {
            k.get(who)
                .filter(|(at, _)| at.elapsed() < KEY_CACHE)
                .map(|(_, v)| v.clone())
        });
        if let Some(v) = hit {
            return Ok(v);
        }
        let fresh = self.chain.current_key(who).await?;
        if let Ok(mut k) = self.keys.lock() {
            k.insert(who.clone(), (Instant::now(), fresh.clone()));
        }
        Ok(fresh)
    }

    async fn sealed(self: Arc<Self>, req: Request) -> Response {
        let started = Instant::now();
        let Ok(body) = http::read_body(req.into_body(), MAX_BODY).await else {
            return http::full(400, "text/plain", "unreadable body");
        };
        let handshake = match sealed_http::handshake_of(&body) {
            Ok(h) => h,
            Err(r) => return r.response(),
        };
        let sender = sealed_http::to_account32(&handshake.handshake.sender);
        let key = match self.user_key(&sender).await {
            Ok(k) => k,
            Err(e) => {
                log::warn!(target: TARGET, "key lookup failed: {e:#}");
                return http::full(503, "text/plain", "chain unavailable");
            }
        };
        let accepted = {
            let acceptor = Acceptor {
                secret: &self.kem,
                recipient: self.recipient,
                now: now_secs(),
            };
            let Ok(mut replay) = self.replay.lock() else {
                return http::full(500, "text/plain", "internal error");
            };
            sealed_http::accept(&body, &acceptor, |_| key, &mut replay)
        };
        let request = match accepted {
            Ok(r) => r,
            Err(r) => {
                log::info!(target: TARGET, "refused a sealed request: {r:?}");
                return r.response();
            }
        };
        let (mut writer, response, user, message) = request.respond();
        let this = Arc::clone(&self);
        tokio::spawn(async move {
            match msg::decode::<UserMsg>(&message) {
                Ok(UserMsg::Chat {
                    request,
                    payment: Payment::Transparent(voucher),
                }) => {
                    let call = ChatCall {
                        request: &request,
                        voucher: &voucher,
                        started,
                    };
                    this.chat(&user, &call, &mut writer).await;
                }
                Ok(UserMsg::Pay {
                    payment: Payment::Transparent(voucher),
                }) => {
                    let reply = this.pay(&user, &voucher).await;
                    writer.send(&msg::encode(&reply), true).await;
                }
                Err(_) => {
                    writer
                        .send(
                            &msg::encode(&gw_error(ErrorCode::BadRequest, "malformed message")),
                            true,
                        )
                        .await;
                }
            }
        });
        response
    }

    /// Checks a voucher on chain: valid for this gateway and the user's channel.
    async fn channel_facts(
        &self,
        user: &AccountId32,
        voucher: &SignedVoucher,
    ) -> Result<Result<ChannelFacts, String>> {
        if voucher.body.user != *user {
            return Ok(Err("the voucher belongs to another user".into()));
        }
        let Some(channel) = self.chain.channel(user, &self.account).await? else {
            return Ok(Err("no credit channel with this gateway".into()));
        };
        if self.book.voucher(user).as_ref() != Some(voucher)
            && let Err(e) = self.chain.check_voucher(voucher).await?
        {
            return Ok(Err(e.to_string()));
        }
        Ok(Ok(ChannelFacts {
            number: channel.number,
            escrow: channel.escrow,
            redeemed: channel.redeemed,
            rate: self.chain.rate().await?,
        }))
    }

    async fn pay(&self, user: &AccountId32, voucher: &SignedVoucher) -> GatewayMsg {
        let facts = match self.channel_facts(user, voucher).await {
            Ok(Ok(f)) => f,
            Ok(Err(reason)) => return gw_error(ErrorCode::PaymentRequired, &reason),
            Err(e) => {
                log::warn!(target: TARGET, "payment check failed: {e:#}");
                return gw_error(ErrorCode::Internal, "chain unavailable");
            }
        };
        match self.book.pay(user, facts.number, voucher) {
            Ok(Ok(billed_total)) => GatewayMsg::Paid { billed_total },
            Ok(Err(e)) => gw_error(ErrorCode::PaymentRequired, &e.to_string()),
            Err(e) => {
                log::warn!(target: TARGET, "ledger write failed: {e:#}");
                gw_error(ErrorCode::Internal, "internal error")
            }
        }
    }

    async fn chat(&self, user: &AccountId32, call: &ChatCall<'_>, writer: &mut SealedWriter) {
        if let Err((code, reason)) = self.chat_inner(user, call, writer).await {
            log::info!(target: TARGET, "request refused or failed: {}", code.as_str());
            writer
                .send(&msg::encode(&gw_error(code, &reason)), true)
                .await;
        }
    }

    async fn chat_inner(
        &self,
        user: &AccountId32,
        call: &ChatCall<'_>,
        writer: &mut SealedWriter,
    ) -> Result<(), (ErrorCode, String)> {
        let (request, voucher, started) = (call.request, call.voucher, call.started);
        let internal = |e: anyhow::Error| {
            log::warn!(target: TARGET, "internal failure: {e:#}");
            (ErrorCode::Internal, "internal error".to_string())
        };
        let mut chat =
            ChatRequest::parse(request).map_err(|e| (ErrorCode::BadRequest, e.to_string()))?;
        if chat.choices() != 1 {
            return Err((ErrorCode::BadRequest, "only n = 1 is supported".into()));
        }
        let model = self.router.resolve(chat.model())?;
        let max_out = chat.max_output_tokens().unwrap_or(self.max_output_tokens);
        chat.set_max_output_tokens(max_out);
        let input_bound = chat.input_bound();
        let streaming = chat.stream();
        let (choices, worst) = self.router.candidates(model, input_bound, max_out);
        let Some(max_fee) = worst.filter(|_| !choices.is_empty()) else {
            return Err((
                ErrorCode::NoProvider,
                "no serviceable provider for this model".into(),
            ));
        };
        let facts = self
            .channel_facts(user, voucher)
            .await
            .map_err(internal)?
            .map_err(|reason| (ErrorCode::PaymentRequired, reason))?;
        let mut request_id = [0u8; 32];
        rand_fill(
            &mut OsRng::new().map_err(|e| internal(e.into()))?,
            &mut request_id,
        );
        self.book
            .admit(user, facts, voucher, (request_id, max_fee))
            .map_err(internal)?
            .map_err(|e| (ErrorCode::PaymentRequired, e.to_string()))?;
        let job = Job {
            user: user.clone(),
            request_id,
            model,
            input_bound,
            max_out,
            streaming,
            started,
        };
        let outcome = self.route(&job, &chat.to_bytes(), &choices, writer).await;
        if outcome.is_err() {
            self.book.release(user, &request_id);
        }
        outcome
    }

    /// Tries providers in order until one has delivered a first chunk; then relays it to the end.
    async fn route(
        &self,
        job: &Job,
        request: &[u8],
        choices: &[Choice],
        writer: &mut SealedWriter,
    ) -> Result<(), (ErrorCode, String)> {
        let infer = msg::encode(&ProviderReq::Infer {
            request_id: job.request_id,
            kind: JobKind::Inference,
            model: job.model,
            request: request.to_vec(),
        });
        for choice in choices {
            let endpoint = String::from_utf8_lossy(&choice.record.endpoint).into_owned();
            let mut resp = match sealed_http::post(
                &self.http,
                &endpoint,
                &choice.record.kem_pk,
                (&self.account, &self.key),
                &infer,
            )
            .await
            {
                Ok(r) => r,
                Err(e) => {
                    log::info!(target: TARGET, "provider {} unreachable: {e:#}", short_account(&choice.provider));
                    self.router.suspend(&choice.provider, UNREACHABLE_PAUSE);
                    continue;
                }
            };
            // Before the first chunk the request can still move to the next provider.
            let first = match resp
                .next()
                .await
                .map(|m| m.map(|b| msg::decode::<ProviderMsg>(&b)))
            {
                Ok(Some(Ok(ProviderMsg::Delta(d)))) => d,
                Ok(Some(Ok(ProviderMsg::Error { code, .. }))) => {
                    log::info!(target: TARGET, "provider {} refused: {}", short_account(&choice.provider), code.as_str());
                    self.router.suspend(&choice.provider, UNREACHABLE_PAUSE);
                    continue;
                }
                _ => {
                    self.router.suspend(&choice.provider, UNREACHABLE_PAUSE);
                    continue;
                }
            };
            return self.relay(job, choice, (first, resp), writer).await;
        }
        Err((ErrorCode::ProviderFailed, "every provider failed".into()))
    }

    async fn relay(
        &self,
        job: &Job,
        choice: &Choice,
        (first, mut resp): (Vec<u8>, sealed_http::SealedResponse),
        writer: &mut SealedWriter,
    ) -> Result<(), (ErrorCode, String)> {
        let failed = |why: &str| (ErrorCode::ProviderFailed, why.to_string());
        let mut assembler = Assembler::new();
        let mut ttft = None;
        let mut pending = Some(first);
        let (body, key, signature, usage) = loop {
            let next = match pending.take() {
                Some(d) => ProviderMsg::Delta(d),
                None => match resp.next().await {
                    Ok(Some(b)) => msg::decode::<ProviderMsg>(&b)
                        .map_err(|_| failed("malformed provider message"))?,
                    _ => return Err(failed("the provider stream broke")),
                },
            };
            match next {
                ProviderMsg::Delta(d) => {
                    if ttft.is_none() && has_content(&d) {
                        ttft = Some(millis(job.started.elapsed()));
                    }
                    if job.streaming {
                        if !writer
                            .send(&msg::encode(&GatewayMsg::Delta(d)), false)
                            .await
                        {
                            return Err(failed("the user went away"));
                        }
                    } else {
                        assembler.push(&d);
                    }
                }
                ProviderMsg::Receipt {
                    body,
                    key,
                    signature,
                    usage,
                } => break (body, key, signature, usage),
                ProviderMsg::Error { .. } | ProviderMsg::Ack => {
                    return Err(failed("the provider failed mid-stream"));
                }
            }
        };
        if let Some(t) = ttft {
            self.router.observe(&choice.provider, t);
        }
        let receipt = match self
            .cosign(job, choice, (body, key, signature, usage))
            .await
        {
            Ok(r) => r,
            Err(e) => {
                log::warn!(target: TARGET, "invalid receipt from provider {}: {e:#}", short_account(&choice.provider));
                self.router.suspend(&choice.provider, INVALID_RECEIPT_PAUSE);
                return Err(failed("the provider's receipt is invalid"));
            }
        };
        let fee = receipt.body.fee;
        let billed_total = self.book.bill(&job.user, receipt.clone()).map_err(|e| {
            log::warn!(target: TARGET, "ledger write failed: {e:#}");
            (ErrorCode::Internal, "internal error".to_string())
        })?;
        if !job.streaming {
            writer
                .send(
                    &msg::encode(&GatewayMsg::Completion(assembler.finish(usage))),
                    false,
                )
                .await;
        }
        writer
            .send(
                &msg::encode(&GatewayMsg::Billing {
                    receipt: receipt.clone(),
                    fee,
                    billed_total,
                }),
                true,
            )
            .await;
        log::info!(
            target: TARGET,
            "request {} via provider {} model {}: {} + {} tokens, fee {} micro-USD, ttft {} ms, total {} ms",
            short(&job.request_id), short_account(&choice.provider), short(&job.model.0),
            usage.prompt_tokens, usage.completion_tokens, fee.0, ttft.unwrap_or(0), millis(job.started.elapsed())
        );
        self.hand_back(choice, receipt);
        Ok(())
    }

    /// Checks the provider's receipt against this request and the chain, then adds the
    /// gateway's signature.
    async fn cosign(
        &self,
        job: &Job,
        choice: &Choice,
        (body, provider_key, provider_sig, usage): (
            ReceiptBody,
            PqPublicKey,
            ac_crypto::PqSignature,
            Usage,
        ),
    ) -> Result<SignedReceipt> {
        anyhow::ensure!(body.genesis == self.genesis, "wrong genesis");
        anyhow::ensure!(
            body.gateway == self.account && body.provider == choice.provider,
            "wrong parties"
        );
        anyhow::ensure!(
            body.request_id == job.request_id && body.model == job.model,
            "wrong request"
        );
        anyhow::ensure!(body.kind == JobKind::Inference, "wrong job kind");
        anyhow::ensure!(
            body.in_tokens == usage.prompt_tokens && body.out_tokens == usage.completion_tokens,
            "token counts differ from the usage"
        );
        anyhow::ensure!(
            u64::from(body.in_tokens) <= job.input_bound && body.out_tokens <= job.max_out,
            "token counts above the request's bounds"
        );
        anyhow::ensure!(
            fee_for(&choice.price, body.in_tokens, body.out_tokens) == Some(body.fee),
            "fee differs from the listed price"
        );
        let registered = self
            .chain
            .current_key(&choice.provider)
            .await?
            .context("the provider has no key")?;
        let gateway_sig = self
            .key
            .sign(&body.payload()?, RECEIPT_CONTEXT, &mut OsRng::new()?)?;
        let receipt = SignedReceipt {
            body,
            provider_key,
            provider_sig,
            gateway_key: self.key.public_key()?,
            gateway_sig,
        };
        check_receipt(
            &receipt,
            &ReceiptContext {
                genesis: &self.genesis,
                provider_key: &key_fingerprint(&registered),
                gateway_key: &self.key_fingerprint,
                price: Some(&choice.price),
            },
        )
        .map_err(|e| anyhow::anyhow!("{e}"))?;
        Ok(receipt)
    }

    /// Returns the co-signed receipt to the provider for its records (best effort, retried).
    fn hand_back(&self, choice: &Choice, receipt: SignedReceipt) {
        let (http, endpoint, kem) = (
            self.http.clone(),
            String::from_utf8_lossy(&choice.record.endpoint).into_owned(),
            choice.record.kem_pk.clone(),
        );
        let (account, key) = (self.account.clone(), Arc::clone(&self.key));
        tokio::spawn(async move {
            let message = msg::encode(&ProviderReq::Cosigned(receipt));
            for attempt in 0..3u32 {
                if let Ok(mut r) =
                    sealed_http::post(&http, &endpoint, &kem, (&account, &key), &message).await
                    && matches!(r.next().await, Ok(Some(_)))
                {
                    return;
                }
                tokio::time::sleep(Duration::from_secs(1u64 << attempt)).await;
            }
            log::info!(target: TARGET, "could not hand a co-signed receipt back to its provider");
        });
    }
}

struct ChatCall<'a> {
    request: &'a [u8],
    voucher: &'a SignedVoucher,
    started: Instant,
}

struct Job {
    user: AccountId32,
    request_id: [u8; 32],
    model: ModelId,
    input_bound: u64,
    max_out: u32,
    streaming: bool,
    started: Instant,
}

fn gw_error(code: ErrorCode, message: &str) -> GatewayMsg {
    GatewayMsg::Error {
        code,
        message: message.as_bytes().to_vec(),
    }
}

fn rand_fill(rng: &mut OsRng, out: &mut [u8]) {
    rand_core::Rng::fill_bytes(rng, out);
}

fn millis(d: Duration) -> u32 {
    u32::try_from(d.as_millis()).unwrap_or(u32::MAX)
}

fn short(bytes: &[u8]) -> String {
    hex::encode(bytes.get(..6).unwrap_or(bytes))
}

fn short_account(a: &AccountId32) -> String {
    short(AsRef::<[u8]>::as_ref(a))
}
