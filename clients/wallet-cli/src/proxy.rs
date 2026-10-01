//! The wallet's local inference proxy (`ac-wallet market serve`, spec `clients/wallet-cli`
//! "本地推理代理").
//!
//! Unmodified OpenAI SDKs talk to it on the loopback interface; it seals each request to the
//! gateway with the user's key, attaches a cumulative voucher equal to everything already paid,
//! and after each response checks the double-signed receipt (signatures, genesis, gateway,
//! model, token counts against the usage it saw, fee at the on-chain price, the gateway's billed
//! total) before paying exactly that total. A receipt that fails any check halts all payments.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use ac_crypto::KemPublicKey;
use ac_crypto::sig::SigningKey;
use ac_market_proto::openai::{SSE_DONE, error_json, sse_data, usage_of, with_usage};
use ac_market_proto::toploc::ToplocProofs;
use ac_market_proto::{ChatRequest, ErrorCode, GatewayMsg, Payment, Usage, UserMsg, msg};
use ac_primitives::market::voucher::VOUCHER_CONTEXT;
use ac_primitives::market::{
    MicroUsd, ModelId, ReceiptBody, SignedReceipt, SignedVoucher, VoucherBody,
};
use anyhow::{Context, Result, anyhow, bail};
use async_trait::async_trait;
use serde_json::{Value, json};
use sp_core::H256;
use sp_runtime::AccountId32;
use tokio::sync::Mutex;

use crate::NodeClient;
use crate::http::{self, BodySender, Client, Request, Response};
use crate::sealed_http::{self, SealedResponse};
use crate::work::{ReceiptFacts, check};

/// Default listening address (loopback only).
pub const DEFAULT_LISTEN: &str = "127.0.0.1:8411";

/// Where receipt facts come from (the node; fixed in tests).
#[async_trait]
pub trait Facts: Send + Sync {
    /// Chain facts a receipt is checked against.
    async fn receipt_facts(&self, body: &ReceiptBody) -> Result<ReceiptFacts>;
}

#[async_trait]
impl Facts for NodeClient {
    async fn receipt_facts(&self, body: &ReceiptBody) -> Result<ReceiptFacts> {
        NodeClient::receipt_facts(self, body).await
    }
}

/// Static configuration.
pub struct Config {
    /// The user account.
    pub user: AccountId32,
    /// The channel's voucher key.
    pub key: SigningKey,
    /// Genesis hash.
    pub genesis: H256,
    /// The gateway account.
    pub gateway: AccountId32,
    /// Its base URL.
    pub gateway_url: String,
    /// Its encapsulation key (checked against its announcement and account key beforehand).
    pub gateway_kem: KemPublicKey,
    /// Current channel number.
    pub channel: u32,
    /// Stop paying beyond this cumulative total.
    pub max_usd: Option<MicroUsd>,
    /// File keeping the paid total across restarts.
    pub state_path: PathBuf,
}

/// What has been paid on the channel.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Paid {
    /// Channel number.
    pub channel: u32,
    /// Cumulative total paid.
    pub total: MicroUsd,
}

/// Request header naming the provider a request must go to (spec `clients/wallet-cli`
/// "代理指定提供者"); its value is an account address.
pub const PROVIDER_HEADER: &str = "x-agentcoin-provider";

/// A finished non-streamed request whose bill was checked and paid.
#[derive(Clone, Debug)]
pub struct Completed {
    /// The Chat Completions response (JSON bytes, `usage` from the receipt).
    pub response: Vec<u8>,
    /// The double-signed receipt.
    pub receipt: SignedReceipt,
    /// The TOPLOC proofs it commits to, if any.
    pub proofs: Option<ToplocProofs>,
    /// The fee paid for it.
    pub fee: MicroUsd,
}

/// What a receipt must match besides the chain facts.
#[derive(Clone, Debug, Default)]
struct Expect {
    /// The model, when the request names it unambiguously.
    model: Option<ModelId>,
    /// The provider a pinned request names.
    provider: Option<AccountId32>,
}

/// The paid total and the voucher that paid it: requests reuse that voucher, so the gateway
/// recognizes it and need not check it on chain again.
struct PaidState {
    paid: Paid,
    voucher: Option<SignedVoucher>,
}

type ModelCache = Option<(std::time::Instant, Vec<(ModelId, String)>)>;

/// How long the gateway's model list is cached.
const MODEL_CACHE: Duration = Duration::from_secs(30);

/// The running proxy.
pub struct Proxy {
    cfg: Config,
    http: Client,
    facts: Box<dyn Facts>,
    paid: Mutex<PaidState>,
    halted: std::sync::Mutex<Option<String>>,
    models: std::sync::Mutex<ModelCache>,
}

impl Proxy {
    /// Builds the proxy, restoring the paid total of this gateway and channel (a new channel
    /// number starts from zero).
    ///
    /// # Errors
    ///
    /// An unreadable state file.
    pub fn new(cfg: Config, facts: Box<dyn Facts>) -> Result<Arc<Self>> {
        let paid = load_state(&cfg)?;
        Ok(Arc::new(Self {
            http: Client::new()?,
            facts,
            paid: Mutex::new(PaidState {
                paid,
                voucher: None,
            }),
            halted: std::sync::Mutex::new(None),
            models: std::sync::Mutex::new(None),
            cfg,
        }))
    }

    /// The paid total.
    pub async fn paid(&self) -> Paid {
        self.paid.lock().await.paid
    }

    /// Why payments stopped, if they did.
    #[must_use]
    pub fn halted(&self) -> Option<String> {
        self.halted.lock().ok().and_then(|h| h.clone())
    }

    fn halt(&self, reason: String) {
        if let Ok(mut h) = self.halted.lock() {
            h.get_or_insert(reason);
        }
    }

    /// Handles one local HTTP request.
    pub async fn handle(self: Arc<Self>, req: Request) -> Response {
        match (req.method().as_str(), req.uri().path()) {
            ("GET", "/v1/models" | "/models") => self.models().await,
            ("POST", "/v1/chat/completions" | "/chat/completions") => {
                let pin = match pinned_provider(&req) {
                    Ok(p) => p,
                    Err(e) => return error(ErrorCode::BadRequest, &e),
                };
                let Ok(body) = http::read_body(req.into_body(), sealed_http::MAX_MESSAGE).await
                else {
                    return error(ErrorCode::BadRequest, "unreadable body");
                };
                self.chat(body.to_vec(), pin).await
            }
            _ => http::json(404, error_json(ErrorCode::BadRequest, "unknown path")),
        }
    }

    async fn models(&self) -> Response {
        let url = http::join(&self.cfg.gateway_url, "/ac/v1/models");
        match self.http.get_bytes(&url, 4 << 20).await {
            Ok(b) => http::json(200, b),
            Err(e) => error(
                ErrorCode::ProviderFailed,
                &format!("gateway unavailable: {e:#}"),
            ),
        }
    }

    async fn model_ids(&self) -> Vec<(ModelId, String)> {
        let cached = self
            .models
            .lock()
            .ok()
            .and_then(|m| m.clone())
            .filter(|(at, _)| at.elapsed() < MODEL_CACHE);
        if let Some((_, list)) = cached {
            return list;
        }
        let list = self.fetch_model_ids().await;
        if !list.is_empty()
            && let Ok(mut m) = self.models.lock()
        {
            *m = Some((std::time::Instant::now(), list.clone()));
        }
        list
    }

    async fn fetch_model_ids(&self) -> Vec<(ModelId, String)> {
        let url = http::join(&self.cfg.gateway_url, "/ac/v1/models");
        let Ok(b) = self.http.get_bytes(&url, 4 << 20).await else {
            return Vec::new();
        };
        let Ok(v) = serde_json::from_slice::<Value>(&b) else {
            return Vec::new();
        };
        v.get("data")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|m| {
                let id = m.get("id")?.as_str()?.strip_prefix("0x")?;
                let id: [u8; 32] = hex::decode(id).ok()?.try_into().ok()?;
                Some((
                    ModelId(id),
                    m.get("name")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                ))
            })
            .collect()
    }

    /// The model a request's receipt must name, when it can be known from the request.
    async fn expected_model(&self, requested: &str) -> Option<ModelId> {
        if let Some(hex) = requested.strip_prefix("0x") {
            return hex::decode(hex)
                .ok()
                .and_then(|b| <[u8; 32]>::try_from(b).ok())
                .map(ModelId);
        }
        let matches: Vec<ModelId> = self
            .model_ids()
            .await
            .into_iter()
            .filter(|(_, n)| n == requested)
            .map(|(i, _)| i)
            .collect();
        match matches.as_slice() {
            [one] => Some(*one),
            _ => None,
        }
    }

    fn voucher(&self, cumulative: MicroUsd) -> Result<SignedVoucher> {
        let body = VoucherBody {
            genesis: self.cfg.genesis,
            user: self.cfg.user.clone(),
            gateway: self.cfg.gateway.clone(),
            channel: self.cfg.channel,
            cumulative,
        };
        let signature = self.cfg.key.sign(
            &body.payload()?,
            VOUCHER_CONTEXT,
            &mut ac_crypto::OsRng::new()?,
        )?;
        Ok(SignedVoucher {
            body,
            public_key: self.cfg.key.public_key()?,
            signature,
        })
    }

    async fn send(&self, message: &UserMsg) -> Result<SealedResponse> {
        sealed_http::post(
            &self.http,
            &self.cfg.gateway_url,
            &self.cfg.gateway_kem,
            (&self.cfg.user, &self.cfg.key),
            &msg::encode(message),
        )
        .await
    }

    /// Sends a request (pinned to `pin` if given) with a voucher for everything paid so far and
    /// returns the gateway's first answer, retrying while a concurrent bill settles.
    async fn start(
        &self,
        body: &[u8],
        pin: Option<&AccountId32>,
    ) -> Result<(SealedResponse, GatewayMsg, Expect, bool), (ErrorCode, String)> {
        if let Some(reason) = self.halted() {
            return Err((
                ErrorCode::PaymentRequired,
                format!("payments to this gateway are halted: {reason}"),
            ));
        }
        let chat = ChatRequest::parse(body).map_err(|e| (ErrorCode::BadRequest, e.to_string()))?;
        let paid = self.paid().await;
        if self.cfg.max_usd.is_some_and(|max| paid.total >= max) {
            return Err((
                ErrorCode::PaymentRequired,
                "the spending limit (--max-usd) is reached".into(),
            ));
        }
        let expect = Expect {
            model: self.expected_model(chat.model()).await,
            provider: pin.cloned(),
        };
        let streaming = chat.stream();
        // A concurrent request's bill may reach the gateway between reading the paid total and
        // the gateway admitting this one; then the voucher is one bill behind and is refused.
        // Retry after the payment in progress settles.
        let mut attempt = 0u32;
        loop {
            attempt = attempt.saturating_add(1);
            let voucher = {
                let mut state = self.paid.lock().await;
                let total = state.paid.total;
                match state.voucher.clone().filter(|v| v.body.cumulative == total) {
                    Some(v) => v,
                    None => {
                        let v = self
                            .voucher(total)
                            .map_err(|e| (ErrorCode::Internal, format!("{e:#}")))?;
                        state.voucher = Some(v.clone());
                        v
                    }
                }
            };
            let payment = Payment::Transparent(voucher);
            let message = match pin {
                Some(provider) => UserMsg::ChatTo {
                    provider: provider.clone(),
                    request: body.to_vec(),
                    payment,
                },
                None => UserMsg::Chat {
                    request: body.to_vec(),
                    payment,
                },
            };
            let mut resp = self.send(&message).await.map_err(|e| {
                (
                    ErrorCode::ProviderFailed,
                    format!("gateway unavailable: {e:#}"),
                )
            })?;
            let first = next_msg(&mut resp)
                .await
                .map_err(|e| (ErrorCode::ProviderFailed, format!("{e:#}")))?;
            match first {
                GatewayMsg::Error {
                    code: ErrorCode::PaymentRequired,
                    ..
                } if attempt < 4 => {
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
                GatewayMsg::Error { code, message } => {
                    return Err((code, String::from_utf8_lossy(&message).into_owned()));
                }
                other => return Ok((resp, other, expect, streaming)),
            }
        }
    }

    /// Sends one non-streamed request (pinned to `pin` if given), checks and pays its bill, and
    /// returns the response with its receipt and proofs: the library form of the proxy, for
    /// callers that need the receipt (the auditor agent).
    ///
    /// # Errors
    ///
    /// The OpenAI error code and message the HTTP proxy would answer with; a streamed request
    /// is refused.
    pub async fn complete(
        &self,
        body: &[u8],
        pin: Option<&AccountId32>,
    ) -> Result<Completed, (ErrorCode, String)> {
        let (resp, first, expect, streaming) = self.start(body, pin).await?;
        if streaming {
            return Err((
                ErrorCode::BadRequest,
                "only non-streamed requests are supported here".into(),
            ));
        }
        self.finish(resp, first, &expect).await
    }

    async fn chat(self: Arc<Self>, body: Vec<u8>, pin: Option<AccountId32>) -> Response {
        let (resp, first, expect, streaming) = match self.start(&body, pin.as_ref()).await {
            Ok(r) => r,
            Err((code, message)) => return error(code, &message),
        };
        if streaming {
            let (tx, response) = http::streaming(200, "text/event-stream");
            tokio::spawn(async move { self.stream(resp, first, expect, tx).await });
            response
        } else {
            match self.finish(resp, first, &expect).await {
                Ok(c) => http::json(200, c.response),
                Err((code, message)) => error(code, &message),
            }
        }
    }

    async fn stream(
        &self,
        mut resp: SealedResponse,
        first: GatewayMsg,
        expect: Expect,
        tx: BodySender,
    ) {
        let mut observed: Option<Usage> = None;
        let mut next = Some(first);
        loop {
            let m = match next.take() {
                Some(m) => m,
                None => match next_msg(&mut resp).await {
                    Ok(m) => m,
                    Err(e) => {
                        tx.send(sse_data(&error_json(
                            ErrorCode::ProviderFailed,
                            &format!("{e:#}"),
                        )))
                        .await;
                        return;
                    }
                },
            };
            match m {
                GatewayMsg::Delta(d) => {
                    if let Some(u) = usage_of(&d) {
                        observed = Some(u);
                    }
                    if !tx.send(sse_data(&d)).await {
                        // The client went away; the bill still has to be settled.
                        if let Ok(GatewayMsg::Billing {
                            receipt,
                            fee,
                            billed_total,
                            toploc,
                        }) = drain(&mut resp).await
                        {
                            let _ = self
                                .settle(
                                    (&receipt, toploc.as_ref()),
                                    (fee, billed_total),
                                    &expect,
                                    observed,
                                )
                                .await;
                        }
                        return;
                    }
                }
                GatewayMsg::Billing {
                    receipt,
                    fee,
                    billed_total,
                    toploc,
                } => {
                    if let Err(e) = self
                        .settle(
                            (&receipt, toploc.as_ref()),
                            (fee, billed_total),
                            &expect,
                            observed,
                        )
                        .await
                    {
                        tx.send(sse_data(&error_json(
                            ErrorCode::PaymentRequired,
                            &format!("{e:#}"),
                        )))
                        .await;
                        return;
                    }
                    tx.send(SSE_DONE).await;
                    return;
                }
                GatewayMsg::Error { code, message } => {
                    tx.send(sse_data(&error_json(
                        code,
                        &String::from_utf8_lossy(&message),
                    )))
                    .await;
                    return;
                }
                GatewayMsg::Completion(_) | GatewayMsg::Paid { .. } => {}
            }
        }
    }

    async fn finish(
        &self,
        mut resp: SealedResponse,
        first: GatewayMsg,
        expect: &Expect,
    ) -> Result<Completed, (ErrorCode, String)> {
        let GatewayMsg::Completion(completion) = first else {
            return Err((
                ErrorCode::ProviderFailed,
                "unexpected gateway response".into(),
            ));
        };
        let (receipt, fee, billed, toploc) = match next_msg(&mut resp).await {
            Ok(GatewayMsg::Billing {
                receipt,
                fee,
                billed_total,
                toploc,
            }) => (receipt, fee, billed_total, toploc),
            Ok(GatewayMsg::Error { code, message }) => {
                return Err((code, String::from_utf8_lossy(&message).into_owned()));
            }
            Ok(_) => {
                return Err((
                    ErrorCode::ProviderFailed,
                    "unexpected gateway response".into(),
                ));
            }
            Err(e) => return Err((ErrorCode::ProviderFailed, format!("{e:#}"))),
        };
        let observed = usage_of(&completion);
        self.settle((&receipt, toploc.as_ref()), (fee, billed), expect, observed)
            .await
            .map_err(|e| (ErrorCode::PaymentRequired, format!("{e:#}")))?;
        let usage = Usage {
            prompt_tokens: receipt.body.in_tokens,
            completion_tokens: receipt.body.out_tokens,
        };
        Ok(Completed {
            response: with_usage(&completion, usage),
            receipt,
            proofs: toploc,
            fee,
        })
    }

    /// Checks a bill and, if it is right, pays exactly the new total.
    async fn settle(
        &self,
        (receipt, toploc): (&SignedReceipt, Option<&ToplocProofs>),
        (fee, billed_total): (MicroUsd, MicroUsd),
        expect: &Expect,
        observed: Option<Usage>,
    ) -> Result<()> {
        let mut state = self.paid.lock().await;
        let verdict = match ac_market_proto::toploc::check(&receipt.body, toploc) {
            // The proofs must be those the double-signed receipt commits to (spec "本地推理代理").
            Err(e) => Err(anyhow::anyhow!(
                "TOPLOC proofs do not match the receipt: {e}"
            )),
            Ok(()) => {
                self.verify(receipt, fee, billed_total, (state.paid, expect, observed))
                    .await
            }
        };
        if let Err(e) = verdict {
            self.halt(format!("{e:#}"));
            return Err(e);
        }
        let voucher = self.voucher(billed_total)?;
        let mut resp = self
            .send(&UserMsg::Pay {
                payment: Payment::Transparent(voucher.clone()),
            })
            .await?;
        match next_msg(&mut resp).await? {
            GatewayMsg::Paid { billed_total: t } if t == billed_total => {}
            GatewayMsg::Paid { billed_total: t } => {
                self.halt(format!(
                    "the gateway acknowledged {} micro-USD instead of {}",
                    t.0, billed_total.0
                ));
                bail!("payment acknowledgement mismatch");
            }
            GatewayMsg::Error { message, .. } => {
                bail!("payment refused: {}", String::from_utf8_lossy(&message))
            }
            _ => bail!("unexpected answer to a payment"),
        }
        state.paid.total = billed_total;
        state.voucher = Some(voucher);
        save_state(&self.cfg, state.paid)?;
        Ok(())
    }

    async fn verify(
        &self,
        receipt: &SignedReceipt,
        fee: MicroUsd,
        billed_total: MicroUsd,
        (paid, expect, observed): (Paid, &Expect, Option<Usage>),
    ) -> Result<()> {
        let body = &receipt.body;
        if body.genesis != self.cfg.genesis || body.gateway != self.cfg.gateway {
            bail!("the receipt is for another chain or gateway");
        }
        if expect.model.is_some_and(|m| m != body.model) {
            bail!("the receipt names another model");
        }
        if expect
            .provider
            .as_ref()
            .is_some_and(|p| *p != body.provider)
        {
            bail!("the receipt names another provider than the one the request is pinned to");
        }
        if observed.is_some_and(|u| {
            (u.prompt_tokens, u.completion_tokens) != (body.in_tokens, body.out_tokens)
        }) {
            bail!("the receipt's token counts differ from the usage returned");
        }
        if fee != body.fee {
            bail!("the bill's fee differs from the receipt's");
        }
        let facts = self.facts.receipt_facts(body).await?;
        check(receipt, &facts).map_err(|e| anyhow!("receipt check failed: {e:#}"))?;
        let expected_total = paid.total.0.checked_add(fee.0).context("overflow")?;
        if billed_total.0 != expected_total {
            bail!(
                "the gateway billed a total of {} micro-USD, but {} were paid and this request costs {}",
                billed_total.0,
                paid.total.0,
                fee.0
            );
        }
        Ok(())
    }
}

/// Builds the proxy configuration after checking, against the chain, that the user has a
/// channel with the gateway, that the wallet key is the channel's voucher key and that the
/// gateway's published encryption key is signed by the gateway account's key.
///
/// # Errors
///
/// A missing channel or gateway, a key mismatch or a bad announcement.
pub async fn connect(
    node: &NodeClient,
    (user, key): (AccountId32, SigningKey),
    gateway: AccountId32,
    gateway_url: Option<String>,
    (max_usd, state_path): (Option<MicroUsd>, PathBuf),
) -> Result<Config> {
    use ac_primitives::market::voucher::key_fingerprint;
    let genesis = node.chain_context().await?.genesis_hash;
    let record = node
        .market_gateway(&gateway)
        .await?
        .context("the gateway is not registered")?;
    let channel = node.market_channel(&user, &gateway).await?.context(
        "no credit channel with this gateway: deposit escrow first (market escrow deposit)",
    )?;
    let now = u32::try_from(node.best_block().await?).unwrap_or(u32::MAX);
    if channel.key_at(now) != key_fingerprint(&key.public_key()?) {
        bail!("the wallet's current key is not the channel's voucher key");
    }
    let gateway_url =
        gateway_url.unwrap_or_else(|| String::from_utf8_lossy(&record.endpoint).into_owned());
    let announcement = Client::new()?
        .get_bytes(&http::join(&gateway_url, "/ac/v1/key"), 65_536)
        .await
        .with_context(|| format!("fetching the gateway key from {gateway_url}"))?;
    let announcement = ac_market_proto::GatewayKey::from_json(&announcement)?;
    let (gateway_key, _) = node
        .current_key(&gateway)
        .await?
        .context("the gateway account has no key")?;
    announcement
        .verify(genesis, &gateway, &gateway_key)
        .context("the gateway's encryption key is not signed by the gateway account")?;
    Ok(Config {
        user,
        key,
        genesis,
        gateway,
        gateway_url,
        gateway_kem: announcement.kem_key,
        channel: channel.number,
        max_usd,
        state_path,
    })
}

async fn next_msg(resp: &mut SealedResponse) -> Result<GatewayMsg> {
    let bytes = resp
        .next()
        .await?
        .context("the gateway ended the response early")?;
    Ok(msg::decode(&bytes)?)
}

async fn drain(resp: &mut SealedResponse) -> Result<GatewayMsg> {
    loop {
        match next_msg(resp).await? {
            GatewayMsg::Delta(_) => {}
            other => return Ok(other),
        }
    }
}

/// The provider a request's [`PROVIDER_HEADER`] names, if any.
fn pinned_provider(req: &Request) -> Result<Option<AccountId32>, String> {
    let Some(value) = req.headers().get(PROVIDER_HEADER) else {
        return Ok(None);
    };
    let text = value
        .to_str()
        .map_err(|_| format!("{PROVIDER_HEADER} is not text"))?;
    crate::parse_address(text)
        .map(Some)
        .map_err(|e| format!("{PROVIDER_HEADER}: {e}"))
}

fn error(code: ErrorCode, message: &str) -> Response {
    http::json(code.http_status(), error_json(code, message))
}

fn load_state(cfg: &Config) -> Result<Paid> {
    let fresh = Paid {
        channel: cfg.channel,
        total: MicroUsd::ZERO,
    };
    let Ok(text) = std::fs::read_to_string(&cfg.state_path) else {
        return Ok(fresh);
    };
    let v: Value = serde_json::from_str(&text)
        .with_context(|| format!("reading {}", cfg.state_path.display()))?;
    let gateway = v.get("gateway").and_then(Value::as_str).unwrap_or_default();
    let channel = v
        .get("channel")
        .and_then(Value::as_u64)
        .and_then(|c| u32::try_from(c).ok());
    let total = v
        .get("paid")
        .and_then(Value::as_str)
        .and_then(|p| p.parse::<u128>().ok());
    match (channel, total) {
        (Some(c), Some(t))
            if c == cfg.channel && gateway == hex::encode(AsRef::<[u8]>::as_ref(&cfg.gateway)) =>
        {
            Ok(Paid {
                channel: c,
                total: MicroUsd(t),
            })
        }
        _ => Ok(fresh),
    }
}

fn save_state(cfg: &Config, paid: Paid) -> Result<()> {
    let body = json!({
        "gateway": hex::encode(AsRef::<[u8]>::as_ref(&cfg.gateway)),
        "channel": paid.channel,
        "paid": paid.total.0.to_string(),
    });
    let tmp = cfg.state_path.with_extension("tmp");
    std::fs::write(&tmp, body.to_string()).with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, &cfg.state_path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ac_crypto::kem::KemSecretKey;
    use ac_crypto::sig::SecretSeed;
    use ac_crypto::{KemAlg, SigAlg};

    fn config(channel: u32, path: &std::path::Path) -> Config {
        Config {
            user: AccountId32::new([1; 32]),
            key: SigningKey::from_seed(SigAlg::MlDsa44, &SecretSeed::new([1; 32])).unwrap(),
            genesis: H256([0; 32]),
            gateway: AccountId32::new([2; 32]),
            gateway_url: "http://127.0.0.1:9".into(),
            gateway_kem: KemSecretKey::from_seed(KemAlg::XWing, &SecretSeed::new([3; 32]))
                .unwrap()
                .public_key()
                .unwrap(),
            channel,
            max_usd: None,
            state_path: path.to_path_buf(),
        }
    }

    // The paid total survives restarts; a new channel number (reset channel) or another gateway
    // starts from zero.
    #[test]
    fn state_file_round_trip_and_reset() {
        let path =
            std::env::temp_dir().join(format!("ac-wallet-proxy-state-{}.json", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let cfg = config(4, &path);
        assert_eq!(
            load_state(&cfg).unwrap(),
            Paid {
                channel: 4,
                total: MicroUsd::ZERO
            }
        );
        save_state(
            &cfg,
            Paid {
                channel: 4,
                total: MicroUsd(1_234),
            },
        )
        .unwrap();
        assert_eq!(load_state(&cfg).unwrap().total, MicroUsd(1_234));
        assert_eq!(
            load_state(&config(5, &path)).unwrap(),
            Paid {
                channel: 5,
                total: MicroUsd::ZERO
            }
        );
        let mut other = config(4, &path);
        other.gateway = AccountId32::new([9; 32]);
        assert_eq!(load_state(&other).unwrap().total, MicroUsd::ZERO);
        std::fs::remove_file(&path).unwrap();
    }
}
