//! The agent wired to the real world: the node (reads through `AuditApi`, transactions signed
//! by the auditor account), the payment accounts' proxies, the re-check engines, the evidence
//! service over sealed HTTP, and the loops of `ac-auditor run`.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use ac_crypto::PqPublicKey;
use ac_crypto::kem::KemSecretKey;
use ac_crypto::sealed::{Acceptor, ReplayCache, recipient_id};
use ac_crypto::sig::SigningKey;
use ac_market_proto::ErrorCode;
use ac_market_proto::audit::{EvidenceRequest, EvidenceResponse};
use ac_market_proto::toploc::AUDIT_THRESHOLDS;
use ac_primitives::market::audit::{
    AdjustableParams, AuditParams, AuditorStatus, RoundIndex, VerdictRecord,
};
use ac_primitives::market::model::QuantType;
use ac_primitives::market::{MicroUsd, ModelId};
use ac_runtime::RuntimeCall;
use ac_wallet::http::{self, Client, Request, Response};
use ac_wallet::ops::Signer;
use ac_wallet::proxy::{Completed, Proxy};
use ac_wallet::sealed_http::{self, now_secs};
use ac_wallet::wallet::read_password_file;
use ac_wallet::{NodeClient, Wallet, parse_address};
use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use pallet_audit::AuditorEndpoint;
use parity_scale_codec::{Decode, Encode};
use sp_runtime::AccountId32;
use tokio::sync::Mutex;

use super::config::Config;
use super::ports::{AuditCall, AuditChain, Dispute, FetchEvidence, Recheck, Shop};
use super::prompts::{OutputRange, Prompts};
use super::rand::OsRand;
use super::store::EvidenceStore;
use super::{Agent, Counters, plan, preflight, short, sync_endpoint};
use crate::case::{RecheckCase, Report};
use crate::engine::EngineClient;
use crate::logging::TARGET;
use crate::recheck::Verifier;
use crate::socket::Rows;

/// Largest evidence request body accepted.
const MAX_REQUEST: usize = 64 * 1024;

/// The node and the auditor's signer.
pub struct NodeChain {
    /// The node.
    pub node: NodeClient,
    /// The auditor account's signer.
    pub signer: Signer,
}

#[async_trait]
impl AuditChain for NodeChain {
    async fn best_block(&self) -> Result<u32> {
        Ok(u32::try_from(self.node.best_block().await?).unwrap_or(u32::MAX))
    }

    async fn round(&self) -> Result<Option<(RoundIndex, u32, u32)>> {
        self.node.audit_round().await
    }

    async fn params(&self) -> Result<Option<(AuditParams, AdjustableParams)>> {
        self.node.audit_params().await
    }

    async fn assigned_to(
        &self,
        round: RoundIndex,
        auditor: &AccountId32,
    ) -> Result<Vec<(AccountId32, bool)>> {
        self.node.audit_assigned_to(round, auditor).await
    }

    async fn provider_models(&self, provider: &AccountId32) -> Result<Vec<ModelId>> {
        Ok(self
            .node
            .market_provider(provider)
            .await?
            .map(|p| p.models.iter().map(|m| m.model).collect())
            .unwrap_or_default())
    }

    async fn model_quant(&self, model: ModelId) -> Result<Option<QuantType>> {
        Ok(self
            .node
            .market_model(model)
            .await?
            .map(|m| m.manifest.quant))
    }

    async fn verdicts(
        &self,
        round: RoundIndex,
        provider: &AccountId32,
    ) -> Result<Vec<VerdictRecord<AccountId32>>> {
        self.node.audit_verdicts(round, provider).await
    }

    async fn open_disputes(&self) -> Result<Vec<(AccountId32, u64)>> {
        let mut all = Vec::new();
        let mut after: Option<AccountId32> = None;
        loop {
            let page = self.node.audit_open_disputes(after.as_ref(), 256).await?;
            let full = page.len() == 256;
            after = page.last().map(|(p, _)| p.clone());
            all.extend(page);
            if !full {
                return Ok(all);
            }
        }
    }

    async fn dispute(&self, id: u64) -> Result<Option<Dispute>> {
        self.node.audit_dispute(id).await
    }

    async fn endpoint(&self, who: &AccountId32) -> Result<Option<AuditorEndpoint>> {
        self.node.audit_endpoint(who).await
    }

    async fn is_active_auditor(&self, who: &AccountId32) -> Result<bool> {
        Ok(self
            .node
            .audit_auditor(who)
            .await?
            .is_some_and(|a| a.status == AuditorStatus::Active))
    }

    async fn submit(&self, call: AuditCall) -> Result<bool> {
        let call = match call {
            AuditCall::Verdict(verdict) => pallet_audit::Call::submit_verdict { verdict },
            AuditCall::Vote { provider, id, vote } => {
                pallet_audit::Call::vote { provider, id, vote }
            }
            AuditCall::Close { provider, id } => pallet_audit::Call::close_dispute { provider, id },
            AuditCall::SetEndpoint(endpoint) => pallet_audit::Call::set_endpoint { endpoint },
        };
        let inclusion = self
            .signer
            .submit(&self.node, RuntimeCall::Audit(call))
            .await?;
        Ok(inclusion.success)
    }
}

impl NodeChain {
    async fn account_key(&self, who: &AccountId32) -> Option<PqPublicKey> {
        self.node
            .current_key(who)
            .await
            .ok()
            .flatten()
            .map(|(k, _)| k)
    }
}

/// One (payment account, gateway) route.
pub struct Route {
    /// The payment account (for logs).
    pub payer: AccountId32,
    /// The gateway.
    pub gateway: AccountId32,
    /// Its proxy.
    pub proxy: Arc<Proxy>,
    /// Its spending limit.
    pub max: Option<MicroUsd>,
}

/// The payment accounts' routes.
pub struct ProxyShop {
    /// Every route.
    pub routes: Vec<Route>,
}

#[async_trait]
impl Shop for ProxyShop {
    fn routes(&self) -> usize {
        self.routes.len()
    }

    async fn usable(&self, route: usize) -> bool {
        let Some(r) = self.routes.get(route) else {
            return false;
        };
        if let Some(reason) = r.proxy.halted() {
            log::warn!(target: TARGET, "payer {} at gateway {} halted: {reason}", short(&r.payer), short(&r.gateway));
            return false;
        }
        let paid = r.proxy.paid().await.total;
        r.max.is_none_or(|m| paid < m)
    }

    async fn buy(
        &self,
        route: usize,
        body: &[u8],
        provider: &AccountId32,
    ) -> Result<Completed, (ErrorCode, String)> {
        let r = self
            .routes
            .get(route)
            .ok_or((ErrorCode::Internal, "no such route".to_owned()))?;
        r.proxy.complete(body, Some(provider)).await
    }
}

/// The re-check engines, one per model; each serves one re-check at a time.
pub struct Engines {
    engines: BTreeMap<ModelId, (String, Mutex<Verifier>)>,
}

#[async_trait]
impl Recheck for Engines {
    fn engine_model(&self, model: ModelId) -> Option<String> {
        self.engines.get(&model).map(|(name, _)| name.clone())
    }

    async fn recheck(&self, case: &RecheckCase, quant: QuantType) -> Report {
        let model = ac_wallet::market::parse_model_id(&case.model).ok();
        match model.and_then(|m| self.engines.get(&m)) {
            Some((_, verifier)) => verifier.lock().await.recheck(case, quant).await,
            // Never asked for a model without an engine; judged as an engine failure if it is.
            None => Report {
                outcome: "inconclusive",
                reason: "no re-check engine for the model".into(),
                request: None,
                thresholds_version: AUDIT_THRESHOLDS.version,
                chunks: Vec::new(),
                verdict: crate::case::Outcome::Inconclusive(crate::case::Inconclusive::Engine),
            },
        }
    }
}

/// Evidence requests over sealed HTTP, signed by the auditor's key.
pub struct SealedFetch {
    http: Client,
    account: AccountId32,
    key: SigningKey,
}

#[async_trait]
impl FetchEvidence for SealedFetch {
    async fn fetch(
        &self,
        endpoint: &AuditorEndpoint,
        dispute: u64,
        commitment: [u8; 32],
    ) -> Result<EvidenceResponse> {
        let base = String::from_utf8_lossy(&endpoint.0).into_owned();
        let request = EvidenceRequest {
            dispute,
            commitment,
        };
        let mut resp = sealed_http::post(
            &self.http,
            &base,
            &endpoint.1,
            (&self.account, &self.key),
            &request.encode(),
        )
        .await?;
        let bytes = resp.next().await?.context("empty evidence response")?;
        Ok(EvidenceResponse::decode(&mut &bytes[..])?)
    }
}

/// The evidence service's HTTP side.
struct EvidenceService {
    agent: Arc<Agent>,
    chain: Arc<NodeChain>,
    kem: KemSecretKey,
    recipient: [u8; 32],
    replay: std::sync::Mutex<ReplayCache>,
}

impl EvidenceService {
    async fn handle(self: Arc<Self>, req: Request) -> Response {
        match (req.method().as_str(), req.uri().path()) {
            ("GET", "/health") => return http::full(200, "text/plain", "ok"),
            ("POST", sealed_http::PATH) => {}
            _ => return http::full(404, "text/plain", "not found"),
        }
        let Ok(body) = http::read_body(req.into_body(), MAX_REQUEST).await else {
            return http::full(400, "text/plain", "unreadable body");
        };
        let handshake = match sealed_http::handshake_of(&body) {
            Ok(h) => h,
            Err(r) => return r.response(),
        };
        let sender = sealed_http::to_account32(&handshake.handshake.sender);
        let key = self.chain.account_key(&sender).await;
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
            Err(r) => return r.response(),
        };
        let (mut writer, response, requester, message) = request.respond();
        let agent = Arc::clone(&self.agent);
        tokio::spawn(async move {
            let answer = match EvidenceRequest::decode(&mut &message[..]) {
                Ok(req) => agent.answer(&requester, req).await,
                Err(_) => EvidenceResponse::Refused,
            };
            writer.send(&answer.encode(), true).await;
        });
        response
    }
}

/// Loads, checks and runs the agent until it fails (spec "审计员代理服务").
///
/// # Errors
///
/// A failed start-up check, or the evidence service failing to listen.
pub async fn run(cfg: Config) -> Result<()> {
    let password = read_password_file(&cfg.password_file)?;
    let wallet = Wallet::load(&cfg.wallet)?;
    let signer = Signer::from_wallet(&wallet, &password)?;
    let me = signer.account.clone();
    let key = wallet.current_key(&password)?;
    let node = NodeClient::new(&cfg.node)?;
    let chain = Arc::new(NodeChain {
        node: node.clone(),
        signer,
    });

    let models = cfg
        .engines
        .iter()
        .map(|e| ac_wallet::market::parse_model_id(&e.model))
        .collect::<Result<Vec<_>>>()?;
    let mut payers = Vec::new();
    for p in &cfg.payers {
        payers.push(Wallet::load(&p.wallet)?.account()?);
    }
    preflight(chain.as_ref(), &me, &payers, &models).await?;
    let mut engines = BTreeMap::new();
    for (e, model) in cfg.engines.iter().zip(models) {
        let rows = Rows::listen(&e.socket)?;
        let verifier = Verifier::new(EngineClient::new(&e.engine)?, rows.clone());
        let deadline = tokio::time::Instant::now() + Duration::from_secs(cfg.connect_wait);
        while rows.connections() == 0 {
            if tokio::time::Instant::now() >= deadline {
                bail!(
                    "the re-check engine's plugin did not connect to {}",
                    e.socket.display()
                );
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        engines.insert(model, (e.engine_model.clone(), Mutex::new(verifier)));
    }
    let mut routes = Vec::new();
    for p in &cfg.payers {
        let payer_wallet = Wallet::load(&p.wallet)?;
        let payer_password = read_password_file(&p.password_file)?;
        let payer = payer_wallet.account()?;
        let payer_key = payer_wallet.current_key(&payer_password)?;
        let max = p
            .max_usd
            .as_deref()
            .map(ac_wallet::market::parse_usd)
            .transpose()?;
        for g in &p.gateways {
            let gateway = parse_address(g)?;
            let state = cfg.data_dir.join(format!(
                "paid-{}-{}.json",
                hex::encode(AsRef::<[u8]>::as_ref(&payer)),
                hex::encode(AsRef::<[u8]>::as_ref(&gateway))
            ));
            let proxy_cfg = ac_wallet::proxy::connect(
                &node,
                (payer.clone(), payer_key.clone()),
                gateway.clone(),
                None,
                (max, state),
            )
            .await
            .with_context(|| format!("payment account at gateway {g}"))?;
            routes.push(Route {
                payer: payer.clone(),
                gateway,
                proxy: Proxy::new(proxy_cfg, Box::new(node.clone()))?,
                max,
            });
        }
    }
    let output = OutputRange {
        min: cfg.prompts.min_tokens.unwrap_or(32),
        max: cfg.prompts.max_tokens.unwrap_or(256),
    };
    let mut prompts = Prompts::generator(output);
    if let Some(bank) = &cfg.prompts.bank {
        prompts = prompts.with_bank(bank, cfg.prompts.bank_percent)?;
    }
    let store = EvidenceStore::open(&cfg.data_dir.join("evidence"))?;

    // The evidence endpoint (spec "审计员代理服务" / "自动登记证据地址").
    let kem_password = match &cfg.kem_password_file {
        Some(p) => read_password_file(p)?,
        None => password.clone(),
    };
    let (kem, kem_public) = ac_wallet::kem_key::load(&cfg.kem_key, &kem_password)?;
    let wanted: AuditorEndpoint = (
        frame_support::BoundedVec::try_from(cfg.public_endpoint.as_bytes().to_vec())
            .map_err(|_| anyhow::anyhow!("the public endpoint is too long"))?,
        kem_public.clone(),
    );
    if sync_endpoint(chain.as_ref(), &me, wanted).await? {
        log::info!(target: TARGET, "registered the evidence endpoint {}", cfg.public_endpoint);
    }

    let agent = Arc::new(Agent {
        me: me.clone(),
        chain: chain.clone(),
        shop: Arc::new(ProxyShop { routes }),
        engines: Arc::new(Engines { engines }),
        fetch: Arc::new(SealedFetch {
            http: Client::new()?,
            account: me.clone(),
            key,
        }),
        store,
        prompts,
        rng: Mutex::new(Box::new(OsRand::new()?)),
        margin_percent: cfg.margin_percent,
        counters: Counters::default(),
        reviews: Mutex::new(std::collections::BTreeMap::new()),
    });

    let listener = tokio::net::TcpListener::bind(&cfg.listen)
        .await
        .with_context(|| format!("listening on {}", cfg.listen))?;
    log::info!(target: TARGET, "evidence service on {}", listener.local_addr()?);
    let service = Arc::new(EvidenceService {
        agent: Arc::clone(&agent),
        chain: Arc::clone(&chain),
        recipient: recipient_id(&kem_public)?,
        kem,
        replay: std::sync::Mutex::new(ReplayCache::default()),
    });
    tokio::spawn(http::serve(listener, move |req| {
        Arc::clone(&service).handle(req)
    }));
    tokio::spawn(review_loop(Arc::clone(&agent)));
    tokio::spawn(tidy_loop(Arc::clone(&agent)));
    round_loop(agent).await
}

/// Waits for each new round and schedules its audits.
async fn round_loop(agent: Arc<Agent>) -> Result<()> {
    let mut seen: Option<RoundIndex> = None;
    loop {
        if let Ok(Some((round, start, next))) = agent.chain.round().await
            && seen != Some(round)
        {
            match agent.chain.assigned_to(round, &agent.me).await {
                Ok(assigned) => {
                    seen = Some(round);
                    let planned = {
                        let mut rng = agent.rng.lock().await;
                        plan((start, next), &assigned, agent.margin_percent, rng.as_mut())
                    };
                    log::info!(target: TARGET, "round {round}: {} audits planned", planned.len());
                    for p in planned {
                        let a = Arc::clone(&agent);
                        tokio::spawn(async move {
                            while a.chain.best_block().await.is_ok_and(|b| b < p.at) {
                                tokio::time::sleep(Duration::from_millis(500)).await;
                            }
                            a.audit(round, &p.provider).await;
                        });
                    }
                }
                Err(e) => {
                    log::warn!(target: TARGET, "round {round}: assignments unavailable: {e:#}")
                }
            }
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

async fn review_loop(agent: Arc<Agent>) {
    loop {
        agent.review_all().await;
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}

async fn tidy_loop(agent: Arc<Agent>) {
    loop {
        agent.tidy().await;
        tokio::time::sleep(Duration::from_secs(10)).await;
    }
}
