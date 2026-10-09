//! The agent's decisions against stand-ins for the chain, the gateways, the engines and the
//! accusers (m6-auditor-agent groups 5 and 6); each test names the spec `market/auditor-agent`
//! scenario it covers.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;

use ac_crypto::sig::SigningKey;
use ac_crypto::{KemAlg, KemPublicKey, SigAlg};
use ac_market_proto::ErrorCode;
use ac_market_proto::audit::{AuditEvidence, EvidenceRequest, EvidenceResponse};
use ac_market_proto::toploc::AUDIT_THRESHOLDS;
use ac_primitives::market::audit::{
    Accuser, AdjustableParams, AuditParams, CURRENT_STATS, DisputeKind, FailReason as ChainFail,
    RoundIndex, SprtEntry, SprtState, StatsConfig, VerdictOutcome, VerdictRecord, Vote,
};
use ac_primitives::market::model::QuantType;
use ac_primitives::market::receipt::RECEIPT_CONTEXT;
use ac_primitives::market::work::JobKind;
use ac_primitives::market::{MicroUsd, ModelId, ReceiptBody, SignedReceipt};
use ac_wallet::proxy::Completed;
use anyhow::Result;
use async_trait::async_trait;
use pallet_audit::AuditorEndpoint;
use parity_scale_codec::Encode;
use sp_core::H256;
use sp_runtime::AccountId32;
use tokio::sync::Mutex;

use super::ports::{AuditCall, AuditChain, Dispute, FetchEvidence, Recheck, Shop};
use super::prompts::{OutputRange, Prompts, words};
use super::rand::tests::Seeded;
use super::review::Progress;
use super::store::{EvidenceStore, Stored};
use super::{
    Agent, Audited, Counters, PROMPT_ATTEMPTS, Planned, plan, preflight, screen_bank, sync_endpoint,
};
use crate::case::{ChunkMetrics, FailReason, Outcome, RecheckCase, Report, StatsReport};

const MODEL: ModelId = ModelId([0x44; 32]);
const OTHER_MODEL: ModelId = ModelId([0x45; 32]);

fn acc(n: u8) -> AccountId32 {
    AccountId32::new([n; 32])
}

fn key(name: &str) -> SigningKey {
    SigningKey::from_seed(SigAlg::MlDsa44, &ac_crypto::dev_seed(name).unwrap()).unwrap()
}

fn receipt(provider: &AccountId32, id: u8) -> SignedReceipt {
    let (p, g) = (key("agent-p"), key("agent-g"));
    let body = ReceiptBody {
        genesis: H256([1; 32]),
        gateway: acc(90),
        provider: provider.clone(),
        kind: JobKind::Inference,
        model: MODEL,
        request_id: [id; 32],
        in_tokens: 12,
        out_tokens: 5,
        fee: MicroUsd(3),
        toploc_commit: [0; 32],
        ttft_ms: 1,
        total_ms: 2,
    };
    let payload = body.payload().unwrap();
    SignedReceipt {
        provider_sig: p.sign_deterministic(&payload, RECEIPT_CONTEXT).unwrap(),
        provider_key: p.public_key().unwrap(),
        gateway_sig: g.sign_deterministic(&payload, RECEIPT_CONTEXT).unwrap(),
        gateway_key: g.public_key().unwrap(),
        body,
    }
}

fn endpoint(n: u8) -> AuditorEndpoint {
    (
        frame_support::BoundedVec::truncate_from(format!("http://auditor-{n}").into_bytes()),
        KemPublicKey::new(KemAlg::XWing, &[n; 1216]).unwrap(),
    )
}

/// Verdicts by round and provider.
type Verdicts = BTreeMap<(RoundIndex, AccountId32), Vec<VerdictRecord<AccountId32>>>;

/// A chain in memory.
#[derive(Default)]
struct FakeChain {
    active: bool,
    version: u16,
    round: (RoundIndex, u32, u32),
    block: u32,
    models: BTreeMap<AccountId32, Vec<ModelId>>,
    verdicts: StdMutex<Verdicts>,
    disputes: StdMutex<BTreeMap<u64, Dispute>>,
    endpoints: StdMutex<BTreeMap<AccountId32, AuditorEndpoint>>,
    stats: Option<StatsConfig>,
    states: StdMutex<BTreeMap<AccountId32, SprtState<AccountId32>>>,
    sent: StdMutex<Vec<String>>,
    /// The account the next transactions are signed by.
    me: StdMutex<Option<AccountId32>>,
}

#[async_trait]
impl AuditChain for FakeChain {
    async fn best_block(&self) -> Result<u32> {
        Ok(self.block)
    }
    async fn round(&self) -> Result<Option<(RoundIndex, u32, u32)>> {
        Ok(Some(self.round))
    }
    async fn params(&self) -> Result<Option<(AuditParams, AdjustableParams)>> {
        let a = AdjustableParams {
            stake_usd: MicroUsd(1_000_000_000),
            payment_usd: MicroUsd(50_000),
            thresholds_version: self.version,
        };
        Ok(Some((AuditParams::LIVE, a)))
    }
    async fn assigned_to(
        &self,
        _round: RoundIndex,
        _auditor: &AccountId32,
    ) -> Result<Vec<(AccountId32, bool)>> {
        Ok(Vec::new())
    }
    async fn provider_models(&self, provider: &AccountId32) -> Result<Vec<ModelId>> {
        Ok(self.models.get(provider).cloned().unwrap_or_default())
    }
    async fn model_quant(&self, model: ModelId) -> Result<Option<QuantType>> {
        Ok((model == MODEL || model == OTHER_MODEL).then_some(QuantType::Bf16))
    }
    async fn verdicts(
        &self,
        round: RoundIndex,
        provider: &AccountId32,
    ) -> Result<Vec<VerdictRecord<AccountId32>>> {
        Ok(self
            .verdicts
            .lock()
            .unwrap()
            .get(&(round, provider.clone()))
            .cloned()
            .unwrap_or_default())
    }
    async fn open_disputes(&self) -> Result<Vec<(AccountId32, u64)>> {
        Ok(self
            .disputes
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, d)| d.outcome.is_none())
            .map(|(id, d)| (d.provider.clone(), *id))
            .collect())
    }
    async fn dispute(&self, id: u64) -> Result<Option<Dispute>> {
        Ok(self.disputes.lock().unwrap().get(&id).cloned())
    }
    async fn endpoint(&self, who: &AccountId32) -> Result<Option<AuditorEndpoint>> {
        Ok(self.endpoints.lock().unwrap().get(who).cloned())
    }
    async fn is_active_auditor(&self, _who: &AccountId32) -> Result<bool> {
        Ok(self.active)
    }
    async fn stats_config(&self) -> Result<Option<StatsConfig>> {
        Ok(self.stats)
    }
    async fn sprt_state(&self, provider: &AccountId32) -> Result<SprtState<AccountId32>> {
        Ok(self
            .states
            .lock()
            .unwrap()
            .get(provider)
            .cloned()
            .unwrap_or_default())
    }
    async fn submit(&self, call: AuditCall) -> Result<bool> {
        let line = match &call {
            AuditCall::Verdict(v) => {
                // The chain records it as `pallet-audit` would.
                let record = VerdictRecord {
                    auditor: self.me.lock().unwrap().clone().unwrap_or_else(|| acc(1)),
                    outcome: v.outcome,
                    thresholds_version: v.thresholds_version,
                    evidence: v.evidence,
                    receipt_hash: H256(ac_crypto::hash::blake3_256(&v.receipt.encode())),
                    request_id: v.receipt.body.request_id,
                    gateway: v.receipt.body.gateway.clone(),
                    stats: v.stats,
                };
                self.verdicts
                    .lock()
                    .unwrap()
                    .entry((v.round, v.provider.clone()))
                    .or_default()
                    .push(record);
                format!("verdict {:?}", v.outcome)
            }
            AuditCall::Vote { id, vote, .. } => format!("vote {id} {vote:?}"),
            AuditCall::Close { id, .. } => {
                if let Some(d) = self.disputes.lock().unwrap().get_mut(id) {
                    d.outcome = Some(ac_primitives::market::audit::DisputeOutcome::Undecided);
                }
                format!("close {id}")
            }
            AuditCall::SetEndpoint(e) => {
                if let (Some(e), Some(me)) = (e, self.me.lock().unwrap().clone()) {
                    self.endpoints.lock().unwrap().insert(me.clone(), e.clone());
                }
                "endpoint".into()
            }
        };
        self.sent.lock().unwrap().push(line);
        Ok(true)
    }
}

/// Gateways that answer pinned requests, or fail on some routes.
struct FakeShop {
    routes: usize,
    failing: Vec<usize>,
    exhausted: bool,
    bought: StdMutex<Vec<(usize, AccountId32, String)>>,
}

#[async_trait]
impl Shop for FakeShop {
    fn routes(&self) -> usize {
        self.routes
    }
    async fn usable(&self, _route: usize) -> bool {
        !self.exhausted
    }
    async fn buy(
        &self,
        route: usize,
        body: &[u8],
        provider: &AccountId32,
    ) -> Result<Completed, (ErrorCode, String)> {
        self.bought.lock().unwrap().push((
            route,
            provider.clone(),
            String::from_utf8(body.to_vec()).unwrap(),
        ));
        if self.failing.contains(&route) {
            return Err((ErrorCode::NoProvider, "down".into()));
        }
        let response = serde_json::json!({
            "choices": [{"index": 0, "message": {"role": "assistant", "content": "an answer"}, "finish_reason": "stop"}],
        });
        // A fresh request ID per purchase, as a gateway draws them.
        static ID: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(1);
        let id = ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Ok(Completed {
            response: response.to_string().into_bytes(),
            receipt: receipt(provider, id),
            proofs: None,
            fee: MicroUsd(3),
        })
    }
}

/// The prompt token count of the stand-in re-checks (inside the audit length band).
const PROMPT_TOKENS: u32 = 200;

/// Chunk metrics of a re-check: a prefill chunk with mean mantissa error `prefill` and three
/// decode chunks with mean `decode`, in hundredths.
fn chunks(prefill: u32, decode: u32) -> Vec<ChunkMetrics> {
    let chunk = |mean: u32| ChunkMetrics {
        exp_mismatches: 0,
        mant_err_sum: mean,
        mant_count: 100,
        median: Some(0),
    };
    vec![chunk(prefill), chunk(decode), chunk(decode), chunk(decode)]
}

/// Engines whose outcome is set by the test; they remember the cases they saw.
/// Prompt token counting of the stand-in engines.
type CountFn = Box<dyn Fn(&serde_json::Value) -> u32 + Send + Sync>;

struct FakeEngines {
    outcome: StdMutex<Outcome>,
    /// The chunk metrics the re-checks report (honest-looking by default).
    chunks: StdMutex<Vec<ChunkMetrics>>,
    seen: StdMutex<Vec<RecheckCase>>,
    /// Counts prompt tokens; by default a token per word, plus four per message.
    count: CountFn,
}

/// The default count: like a chat template, a few tokens per message around its words.
fn template_count(messages: &serde_json::Value) -> u32 {
    let per_message = messages.as_array().map_or(0, Vec::len) * 4;
    u32::try_from(words(messages) + per_message).unwrap()
}

impl FakeEngines {
    fn new(outcome: Outcome) -> Self {
        Self::counting(outcome, Box::new(template_count))
    }

    fn counting(outcome: Outcome, count: CountFn) -> Self {
        Self {
            outcome: StdMutex::new(outcome),
            chunks: StdMutex::new(chunks(50, 100)),
            seen: StdMutex::new(Vec::new()),
            count,
        }
    }
}

#[async_trait]
impl Recheck for FakeEngines {
    fn engine_model(&self, model: ModelId) -> Option<String> {
        (model == MODEL).then(|| "mock-model".to_owned())
    }
    async fn recheck(&self, case: &RecheckCase, _quant: QuantType) -> Report {
        self.seen.lock().unwrap().push(case.clone());
        let verdict = *self.outcome.lock().unwrap();
        let chunks = self.chunks.lock().unwrap().clone();
        Report {
            outcome: verdict.kind(),
            reason: verdict.reason(),
            request: None,
            thresholds_version: AUDIT_THRESHOLDS.version,
            stats: StatsReport::of(&verdict, Some(PROMPT_TOKENS), &chunks),
            prompt_tokens: Some(PROMPT_TOKENS),
            chunks,
            verdict,
        }
    }
    async fn prompt_tokens(&self, _model: ModelId, messages: &serde_json::Value) -> Result<u32> {
        Ok((self.count)(messages))
    }
}

/// Accusers whose evidence is served from their agents' stores.
#[derive(Default)]
struct FakeFetch {
    by_endpoint: StdMutex<BTreeMap<Vec<u8>, Arc<Agent>>>,
    requester: Option<AccountId32>,
}

#[async_trait]
impl FetchEvidence for FakeFetch {
    async fn fetch(
        &self,
        endpoint: &AuditorEndpoint,
        dispute: u64,
        commitment: [u8; 32],
    ) -> Result<EvidenceResponse> {
        let agent = self
            .by_endpoint
            .lock()
            .unwrap()
            .get(endpoint.0.as_slice())
            .cloned();
        match agent {
            Some(a) => Ok(a
                .answer(
                    self.requester.as_ref().unwrap(),
                    EvidenceRequest {
                        dispute,
                        commitment,
                    },
                )
                .await),
            None => anyhow::bail!("connection refused"),
        }
    }
}

const FAIL: Outcome = Outcome::Fail(FailReason::Threshold {
    chunk: 1,
    metric: ac_market_proto::toploc::Metric::ExpMismatches,
});

fn agent_with(
    me: AccountId32,
    chain: Arc<FakeChain>,
    shop: Arc<FakeShop>,
    engines: Arc<FakeEngines>,
    fetch: Arc<FakeFetch>,
) -> Agent {
    // A directory per agent: tests run in parallel and reuse account numbers.
    static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("ac-auditor-agent-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    Agent {
        me,
        chain,
        shop,
        engines,
        fetch,
        store: EvidenceStore::open(&dir).unwrap(),
        prompts: Prompts::generator(OutputRange { min: 8, max: 16 }),
        rng: Mutex::new(Box::new(Seeded::new(u64::from(n)))),
        margin_percent: 25,
        counters: Counters::default(),
        reviews: Mutex::new(BTreeMap::<u64, Progress>::new()),
    }
}

fn chain_for(me: AccountId32) -> FakeChain {
    let mut models = BTreeMap::new();
    models.insert(acc(10), vec![MODEL, OTHER_MODEL]);
    models.insert(acc(11), vec![OTHER_MODEL]);
    FakeChain {
        active: true,
        version: AUDIT_THRESHOLDS.version,
        stats: Some(StatsConfig {
            version: CURRENT_STATS.version,
            enabled: true,
        }),
        round: (4, 81, 101),
        block: 85,
        models,
        me: StdMutex::new(Some(me)),
        ..FakeChain::default()
    }
}

fn shop(routes: usize) -> Arc<FakeShop> {
    Arc::new(FakeShop {
        routes,
        failing: Vec::new(),
        exhausted: false,
        bought: StdMutex::new(Vec::new()),
    })
}

// ---- 审计员代理服务 ----

#[tokio::test]
async fn start_up_checks() {
    // "未登记的审计员", "付款账户与审计员账户相同"; models must be registered.
    let me = acc(1);
    let mut chain = chain_for(me.clone());
    preflight(&chain, &me, &[acc(2)], &[MODEL]).await.unwrap();
    assert!(
        preflight(&chain, &me, &[acc(2), me.clone()], &[MODEL])
            .await
            .unwrap_err()
            .to_string()
            .contains("payment account")
    );
    assert!(
        preflight(&chain, &me, &[], &[ModelId([9; 32])])
            .await
            .is_err()
    );
    chain.active = false;
    assert!(
        preflight(&chain, &me, &[], &[MODEL])
            .await
            .unwrap_err()
            .to_string()
            .contains("not a registered")
    );
}

#[tokio::test]
async fn the_endpoint_is_registered_when_it_differs() {
    // "自动登记证据地址".
    let me = acc(1);
    let chain = chain_for(me.clone());
    assert!(sync_endpoint(&chain, &me, endpoint(1)).await.unwrap());
    assert_eq!(chain.endpoints.lock().unwrap().get(&me), Some(&endpoint(1)));
    assert!(!sync_endpoint(&chain, &me, endpoint(1)).await.unwrap());
    assert_eq!(chain.sent.lock().unwrap().len(), 1);
}

// ---- 神秘顾客请求 ----

#[test]
fn audits_are_spread_over_the_round_and_done_ones_are_left_out() {
    // "每个被分配的提供者一次请求", "发起时刻随机".
    let assigned = vec![(acc(10), false), (acc(11), true), (acc(12), false)];
    let mut rng = Seeded::new(5);
    let p = plan((100, 120), &assigned, 25, &mut rng);
    assert_eq!(
        p.iter().map(|x| x.provider.clone()).collect::<Vec<_>>(),
        vec![acc(10), acc(12)]
    );
    // Over many rounds the times cover the usable part [100, 115) and never the margin.
    let mut seen = [false; 15];
    for _ in 0..500 {
        for Planned { at, .. } in plan((100, 120), &assigned, 25, &mut rng) {
            assert!((100..115).contains(&at), "{at}");
            seen[usize::try_from(at - 100).unwrap()] = true;
        }
    }
    assert!(seen.iter().all(|s| *s));
    // A round too short for a margin still gets its audits, at its start.
    assert_eq!(plan((7, 8), &assigned, 25, &mut rng)[0].at, 7);
}

#[tokio::test]
async fn honest_providers_get_a_pass_with_statistics() {
    // "诚实的提供者" (m6-audit-sprt): the pass carries the statistics and the commitment to the
    // evidence stored first; the request is pinned and asks for the provider's re-checkable
    // model.
    let me = acc(1);
    let chain = Arc::new(chain_for(me.clone()));
    let s = shop(2);
    let a = agent_with(
        me,
        chain.clone(),
        s.clone(),
        Arc::new(FakeEngines::new(Outcome::Pass)),
        Arc::default(),
    );
    assert_eq!(
        a.audit(4, &acc(10)).await,
        Audited::Submitted(VerdictOutcome::Pass)
    );
    let bought = s.bought.lock().unwrap();
    assert_eq!(bought.len(), 1);
    assert_eq!(bought[0].1, acc(10));
    assert!(bought[0].2.contains(&hex::encode(MODEL.0)));
    assert_eq!(chain.sent.lock().unwrap().as_slice(), ["verdict Pass"]);
    let v = chain.verdicts.lock().unwrap()[&(4, acc(10))][0].clone();
    let s = v.stats.unwrap();
    assert_eq!(s.version, CURRENT_STATS.version);
    assert_eq!(
        (
            s.stats.prompt_tokens,
            s.stats.prefill_mean_centi,
            s.stats.decode_mean_centi
        ),
        (PROMPT_TOKENS, 50, 100)
    );
    let c = v.evidence.unwrap();
    assert!(a.store.get(&c).is_some());
}

#[tokio::test]
async fn inconclusive_verdicts_keep_no_evidence() {
    // "无法判定不保存证据".
    let me = acc(1);
    let chain = Arc::new(chain_for(me.clone()));
    let tokens = Outcome::Inconclusive(crate::case::Inconclusive::Tokens);
    let a = agent_with(
        me,
        chain.clone(),
        shop(1),
        Arc::new(FakeEngines::new(tokens)),
        Arc::default(),
    );
    assert!(matches!(
        a.audit(4, &acc(10)).await,
        Audited::Submitted(VerdictOutcome::Inconclusive(_))
    ));
    let v = chain.verdicts.lock().unwrap()[&(4, acc(10))][0].clone();
    assert_eq!((v.evidence, v.stats), (None, None));
    assert!(a.store.list().is_empty());
}

#[tokio::test]
async fn another_statistics_version_stops_the_audit_before_paying() {
    // "统计参数版本不符".
    let me = acc(1);
    let mut c = chain_for(me.clone());
    c.stats = Some(StatsConfig {
        version: CURRENT_STATS.version + 1,
        enabled: true,
    });
    let chain = Arc::new(c);
    let s = shop(1);
    let a = agent_with(
        me,
        chain.clone(),
        s.clone(),
        Arc::new(FakeEngines::new(Outcome::Pass)),
        Arc::default(),
    );
    assert!(matches!(
        a.audit(4, &acc(10)).await,
        Audited::StatsVersion { .. }
    ));
    assert!(s.bought.lock().unwrap().is_empty());
    assert!(chain.sent.lock().unwrap().is_empty());
}

#[tokio::test]
async fn cheats_get_a_failure_whose_evidence_is_stored_first() {
    // "作弊的提供者": the commitment on chain is that of the stored evidence.
    let me = acc(1);
    let chain = Arc::new(chain_for(me.clone()));
    let a = agent_with(
        me.clone(),
        chain.clone(),
        shop(1),
        Arc::new(FakeEngines::new(FAIL)),
        Arc::default(),
    );
    assert!(matches!(
        a.audit(4, &acc(10)).await,
        Audited::Submitted(VerdictOutcome::Fail(ChainFail::Threshold { .. }))
    ));
    let v = chain.verdicts.lock().unwrap()[&(4, acc(10))][0].clone();
    let c = v.evidence.unwrap();
    let stored = a.store.get(&c).unwrap();
    let e = AuditEvidence::open(&stored, &c).unwrap();
    assert_eq!(e.receipt.body.provider, acc(10));
    assert_eq!(
        a.store.list(),
        vec![Stored {
            round: 4,
            provider: Some(acc(10)),
            commitment: c
        }]
    );
}

#[tokio::test]
async fn providers_without_a_recheckable_model_are_skipped() {
    // "没有可复核的模型".
    let me = acc(1);
    let s = shop(1);
    let chain = Arc::new(chain_for(me.clone()));
    let a = agent_with(
        me,
        chain.clone(),
        s.clone(),
        Arc::new(FakeEngines::new(Outcome::Pass)),
        Arc::default(),
    );
    assert_eq!(a.audit(4, &acc(11)).await, Audited::NoModel);
    assert!(s.bought.lock().unwrap().is_empty());
    assert!(chain.sent.lock().unwrap().is_empty());
    assert_eq!(
        a.counters
            .skipped_no_model
            .load(std::sync::atomic::Ordering::Relaxed),
        1
    );
}

#[tokio::test]
async fn another_thresholds_version_stops_the_audit_before_paying() {
    // "阈值版本不符".
    let me = acc(1);
    let mut c = chain_for(me.clone());
    c.version = AUDIT_THRESHOLDS.version + 1;
    let chain = Arc::new(c);
    let s = shop(1);
    let a = agent_with(
        me,
        chain.clone(),
        s.clone(),
        Arc::new(FakeEngines::new(Outcome::Pass)),
        Arc::default(),
    );
    assert!(matches!(
        a.audit(4, &acc(10)).await,
        Audited::ThresholdsVersion { .. }
    ));
    assert!(s.bought.lock().unwrap().is_empty());
    assert!(chain.sent.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_chain_on_an_older_version_gets_no_verdicts() {
    // "阈值版本不符" (m6-toploc-gpu-calibration 5.1, 11.3): a chain that has not adopted the
    // calibrated thresholds of version 4 is not audited by an agent that judges by them.
    assert_eq!(AUDIT_THRESHOLDS.version, 4);
    let me = acc(1);
    let mut c = chain_for(me.clone());
    c.version = 3;
    let chain = Arc::new(c);
    let s = shop(1);
    let a = agent_with(
        me,
        chain.clone(),
        s.clone(),
        Arc::new(FakeEngines::new(Outcome::Pass)),
        Arc::default(),
    );
    assert!(matches!(
        a.audit(4, &acc(10)).await,
        Audited::ThresholdsVersion { chain: 3, ours: 4 }
    ));
    assert!(s.bought.lock().unwrap().is_empty());
    assert!(chain.sent.lock().unwrap().is_empty());
}

/// A bank file of `entries` user prompts, entry `i` of `words(i)` words.
fn bank_file(name: &str, entries: usize, words: impl Fn(usize) -> usize) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("ac-auditor-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("bank.jsonl");
    let lines: Vec<String> = (0..entries)
        .map(|i| {
            let text: Vec<String> = (0..words(i)).map(|w| format!("b{i}w{w}")).collect();
            serde_json::json!([{"role": "user", "content": text.join(" ")}]).to_string()
        })
        .collect();
    std::fs::write(&path, lines.join("\n")).unwrap();
    path
}

// Spec "审计 prompt" / "审计 prompt 落在审计长度区间内": 200 requests mixing the generator and a
// bank (half of whose entries are outside the band) all fall in the band, and the generated ones
// stay diverse (spec "prompt 多样").
#[tokio::test]
async fn audit_prompts_fall_in_the_band() {
    let band = AUDIT_THRESHOLDS.band;
    let lengths = [
        band.min as usize - 60,
        band.min as usize,
        200,
        band.max as usize + 20,
    ];
    let bank = bank_file("mixed-bank", 8, |i| lengths[i % 4]);
    let me = acc(1);
    let engines = Arc::new(FakeEngines::new(Outcome::Pass));
    let mut a = agent_with(
        me.clone(),
        Arc::new(chain_for(me)),
        shop(1),
        engines.clone(),
        Arc::default(),
    );
    a.prompts = Prompts::generator(OutputRange { min: 8, max: 16 })
        .with_bank(&bank, 30)
        .unwrap();
    screen_bank(&mut a.prompts, engines.as_ref(), &[MODEL])
        .await
        .unwrap();
    let mut generated = Vec::new();
    let mut from_bank = 0;
    for _ in 0..200 {
        let (m, _, counted) = a.prompt(MODEL).await.unwrap();
        let n = template_count(&m);
        assert!(band.contains(n), "{n} prompt tokens");
        assert_eq!(counted, n);
        let text: Vec<&str> = m
            .as_array()
            .unwrap()
            .iter()
            .map(|x| x["content"].as_str().unwrap())
            .collect();
        let text = text.join("\n");
        if text.starts_with("b") {
            from_bank += 1;
        } else {
            generated.push(text);
        }
    }
    assert!((30..=100).contains(&from_bank), "{from_bank} from the bank");
    let distinct: std::collections::BTreeSet<&String> = generated.iter().collect();
    assert!(
        distinct.len() * 100 >= generated.len() * 99,
        "{} distinct",
        distinct.len()
    );
    let first: Vec<char> = generated[0].chars().collect();
    for w in first.windows(16) {
        let s: String = w.iter().collect();
        assert!(
            generated.iter().any(|p| !p.contains(&s)),
            "{s:?} is in every prompt"
        );
    }
    assert_eq!(
        a.counters
            .skipped_no_prompt
            .load(std::sync::atomic::Ordering::Relaxed),
        0
    );
}

// Spec "审计 prompt" / "题库条目不在区间内": with no bank entry in the band the agent refuses
// to start.
#[tokio::test]
async fn a_bank_outside_the_band_is_refused() {
    let bank = bank_file("short-bank", 5, |i| 10 + i);
    let mut p = Prompts::generator(OutputRange::default())
        .with_bank(&bank, 20)
        .unwrap();
    let engines = FakeEngines::new(Outcome::Pass);
    let e = screen_bank(&mut p, &engines, &[MODEL]).await.unwrap_err();
    assert!(e.to_string().contains("audit length band"), "{e}");
}

// Design D9: a prompt that never fits is given up after the attempts, before paying.
#[tokio::test]
async fn an_audit_without_a_prompt_in_the_band_is_skipped() {
    let me = acc(1);
    let calls = Arc::new(std::sync::atomic::AtomicU32::new(0));
    let counted = calls.clone();
    let engines = Arc::new(FakeEngines::counting(
        Outcome::Pass,
        Box::new(move |_| {
            counted.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            AUDIT_THRESHOLDS.band.max + 1
        }),
    ));
    let s = shop(1);
    let a = agent_with(
        me.clone(),
        Arc::new(chain_for(me)),
        s.clone(),
        engines,
        Arc::default(),
    );
    assert!(matches!(a.audit(4, &acc(10)).await, Audited::Missed(_)));
    assert_eq!(
        calls.load(std::sync::atomic::Ordering::Relaxed),
        PROMPT_ATTEMPTS
    );
    assert_eq!(
        a.counters
            .skipped_no_prompt
            .load(std::sync::atomic::Ordering::Relaxed),
        1
    );
    assert!(s.bought.lock().unwrap().is_empty());
}

#[tokio::test]
async fn failed_routes_are_retried_through_others() {
    // "神秘顾客请求": another gateway or payment account after a failure.
    let me = acc(1);
    let s = Arc::new(FakeShop {
        routes: 3,
        failing: vec![0, 1],
        exhausted: false,
        bought: StdMutex::new(Vec::new()),
    });
    let a = agent_with(
        me.clone(),
        Arc::new(chain_for(me)),
        s.clone(),
        Arc::new(FakeEngines::new(Outcome::Pass)),
        Arc::default(),
    );
    assert_eq!(
        a.audit(4, &acc(10)).await,
        Audited::Submitted(VerdictOutcome::Pass)
    );
    let routes: Vec<usize> = s.bought.lock().unwrap().iter().map(|b| b.0).collect();
    assert_eq!(*routes.last().unwrap(), 2);
    let mut distinct = routes.clone();
    distinct.dedup();
    assert_eq!(distinct.len(), routes.len());
}

#[tokio::test]
async fn exhausted_payment_accounts_send_nothing() {
    // "审计付款账户" / "付款账户耗尽" (the request side; votes are tested below).
    let me = acc(1);
    let s = Arc::new(FakeShop {
        routes: 2,
        failing: Vec::new(),
        exhausted: true,
        bought: StdMutex::new(Vec::new()),
    });
    let a = agent_with(
        me.clone(),
        Arc::new(chain_for(me)),
        s.clone(),
        Arc::new(FakeEngines::new(Outcome::Pass)),
        Arc::default(),
    );
    assert!(matches!(a.audit(4, &acc(10)).await, Audited::Missed(_)));
    assert!(s.bought.lock().unwrap().is_empty());
}

// ---- 证据保存与交付、升级复核与投票 ----

/// Two accusers (2 and 3) whose failing verdicts on provider 10 opened dispute 1, reviewed by
/// 5, 6 and 7, all on one chain. Returns the chain and the accusers' agents.
async fn disputed(accuser_outcome: Outcome) -> (Arc<FakeChain>, Vec<Arc<Agent>>) {
    let chain = Arc::new(chain_for(acc(2)));
    let mut accusers = Vec::new();
    for n in [2u8, 3] {
        let a = Arc::new(agent_with(
            acc(n),
            chain.clone(),
            shop(1),
            Arc::new(FakeEngines::new(accuser_outcome)),
            Arc::default(),
        ));
        *chain.me.lock().unwrap() = Some(acc(n));
        a.audit(4, &acc(10)).await;
        chain.endpoints.lock().unwrap().insert(acc(n), endpoint(n));
        accusers.push(a);
    }
    chain.disputes.lock().unwrap().insert(
        1,
        Dispute {
            provider: acc(10),
            round: 4,
            accusers: frame_support::BoundedVec::truncate_from(vec![
                Accuser {
                    auditor: acc(2),
                    round: 4,
                },
                Accuser {
                    auditor: acc(3),
                    round: 4,
                },
            ]),
            reviewers: frame_support::BoundedVec::truncate_from(vec![
                (acc(5), None),
                (acc(6), None),
                (acc(7), None),
            ]),
            deadline: 120,
            outcome: None,
            closed_at: None,
            kind: DisputeKind::Fail,
        },
    );
    (chain, accusers)
}

/// A reviewer agent (account `n`) reaching the accusers through their stores.
fn reviewer(
    n: u8,
    chain: &Arc<FakeChain>,
    accusers: &[Arc<Agent>],
    engine: Outcome,
) -> (Agent, Arc<FakeEngines>) {
    let fetch = FakeFetch {
        requester: Some(acc(n)),
        ..FakeFetch::default()
    };
    for a in accusers {
        let e = <AccountId32 as AsRef<[u8]>>::as_ref(&a.me)[0];
        fetch
            .by_endpoint
            .lock()
            .unwrap()
            .insert(endpoint(e).0.to_vec(), Arc::clone(a));
    }
    let engines = Arc::new(FakeEngines::new(engine));
    (
        agent_with(
            acc(n),
            chain.clone(),
            shop(0),
            engines.clone(),
            Arc::new(fetch),
        ),
        engines,
    )
}

#[tokio::test]
async fn reviewers_get_the_evidence_and_others_do_not() {
    // "复核人取得证据", "非复核人被拒绝".
    let (chain, accusers) = disputed(FAIL).await;
    let c = chain.verdicts.lock().unwrap()[&(4, acc(10))][0]
        .evidence
        .unwrap();
    let req = EvidenceRequest {
        dispute: 1,
        commitment: c,
    };
    match accusers[0].answer(&acc(5), req).await {
        EvidenceResponse::Evidence(b) => {
            AuditEvidence::open(&b, &c).unwrap();
        }
        EvidenceResponse::Refused => panic!("a reviewer was refused"),
    }
    assert_eq!(
        accusers[0].answer(&acc(9), req).await,
        EvidenceResponse::Refused
    );
    // Not this accuser's commitment, or an unknown dispute: refused alike.
    let other = chain.verdicts.lock().unwrap()[&(4, acc(10))][1]
        .evidence
        .unwrap();
    assert_eq!(
        accusers[0]
            .answer(
                &acc(5),
                EvidenceRequest {
                    dispute: 1,
                    commitment: other
                }
            )
            .await,
        EvidenceResponse::Refused
    );
    assert_eq!(
        accusers[0]
            .answer(
                &acc(5),
                EvidenceRequest {
                    dispute: 2,
                    commitment: c
                }
            )
            .await,
        EvidenceResponse::Refused
    );
}

#[tokio::test]
async fn two_real_failures_are_confirmed() {
    // "确认作弊": the reviewer re-checks both accusers' evidence (as the accusers sent it).
    let (chain, accusers) = disputed(FAIL).await;
    let (r, engines) = reviewer(5, &chain, &accusers, FAIL);
    r.review_all().await;
    assert!(
        chain
            .sent
            .lock()
            .unwrap()
            .contains(&"vote 1 Confirm".to_owned())
    );
    let seen = engines.seen.lock().unwrap();
    assert_eq!(seen.len(), 2);
    assert!(seen.iter().all(|c| c.engine_model == "mock-model"));
}

#[tokio::test]
async fn evidence_that_passes_is_rejected() {
    // "驳回伪造的不通过".
    let (chain, accusers) = disputed(FAIL).await;
    let (r, _) = reviewer(6, &chain, &accusers, Outcome::Pass);
    r.review_all().await;
    assert!(
        chain
            .sent
            .lock()
            .unwrap()
            .contains(&"vote 1 Reject".to_owned())
    );
}

#[tokio::test]
async fn unreachable_evidence_is_rejected_at_the_deadline() {
    // "拿不到证据".
    let (mut_chain, accusers) = disputed(FAIL).await;
    let (r, _) = reviewer(7, &mut_chain, &accusers[..1], FAIL);
    r.review_all().await;
    assert!(
        !mut_chain
            .sent
            .lock()
            .unwrap()
            .iter()
            .any(|l| l.starts_with("vote"))
    );
    let d = mut_chain.disputes.lock().unwrap()[&1].clone();
    assert_eq!(
        r.review(1, &d, true).await,
        super::review::Decision::Vote(Vote::Reject)
    );
}

#[tokio::test]
async fn an_unavailable_engine_never_votes() {
    // "本机引擎不可用".
    let (chain, accusers) = disputed(FAIL).await;
    let engine_down = Outcome::Inconclusive(crate::case::Inconclusive::Engine);
    let (r, _) = reviewer(5, &chain, &accusers, engine_down);
    let d = chain.disputes.lock().unwrap()[&1].clone();
    assert_eq!(r.review(1, &d, false).await, super::review::Decision::Wait);
    assert!(matches!(
        r.review(1, &d, true).await,
        super::review::Decision::Abstain(_)
    ));
}

#[tokio::test]
async fn payment_trouble_does_not_stop_reviews() {
    // "付款账户耗尽": no requests, yet evidence is served and votes are cast.
    let (chain, accusers) = disputed(FAIL).await;
    let (r, _) = reviewer(5, &chain, &accusers, FAIL);
    assert_eq!(r.shop.routes(), 0);
    r.review_all().await;
    assert!(
        chain
            .sent
            .lock()
            .unwrap()
            .contains(&"vote 1 Confirm".to_owned())
    );
}

#[tokio::test]
async fn evidence_goes_once_no_dispute_can_use_it() {
    // "争议关闭后删除"; a dispute past its deadline that names the auditor is closed by it.
    let (chain, accusers) = disputed(FAIL).await;
    let a = &accusers[0];
    assert_eq!(a.store.list().len(), 1);
    // Round 5: the verdict of round 4 can still open a dispute; kept.
    let mut c = chain_for(acc(2));
    c.round = (5, 101, 121);
    c.block = 105;
    // (The real chain is shared; a view with a later round and block stands in for time.)
    let later = Arc::new(FakeChain {
        verdicts: StdMutex::new(chain.verdicts.lock().unwrap().clone()),
        disputes: StdMutex::new(chain.disputes.lock().unwrap().clone()),
        ..c
    });
    let view = agent_with(
        acc(2),
        later.clone(),
        shop(0),
        Arc::new(FakeEngines::new(FAIL)),
        Arc::default(),
    );
    let stored = a.store.list();
    view.store
        .put(
            stored[0].round,
            stored[0].provider.as_ref().unwrap(),
            &stored[0].commitment,
            b"x",
        )
        .unwrap();
    view.tidy().await;
    assert_eq!(view.store.list().len(), 1);
    // Round 6, the dispute still open and its deadline (120) not passed: kept.
    let mut c6 = chain_for(acc(2));
    c6.round = (6, 121, 141);
    c6.block = 119;
    let r6 = Arc::new(FakeChain {
        disputes: StdMutex::new(later.disputes.lock().unwrap().clone()),
        ..c6
    });
    let v6 = Agent {
        chain: r6.clone(),
        ..view
    };
    v6.tidy().await;
    assert_eq!(v6.store.list().len(), 1);
    // Past the deadline: the agent closes the dispute, then the evidence goes.
    let mut c7 = chain_for(acc(2));
    c7.round = (6, 121, 141);
    c7.block = 125;
    let r7 = Arc::new(FakeChain {
        disputes: StdMutex::new(r6.disputes.lock().unwrap().clone()),
        ..c7
    });
    let v7 = Agent {
        chain: r7.clone(),
        ..v6
    };
    v7.tidy().await;
    assert!(r7.sent.lock().unwrap().contains(&"close 1".to_owned()));
    v7.tidy().await;
    assert!(v7.store.list().is_empty());
}

// ---- 统计争议（m6-audit-sprt）----

/// Nine accusers (20..=28) whose passes on provider 10 in round 4 carry the statistics of
/// `claimed` chunk metrics, and the statistical dispute 2 on them, reviewed by 5, 6 and 7.
async fn statistically_disputed(claimed: (u32, u32)) -> (Arc<FakeChain>, Vec<Arc<Agent>>) {
    let chain = Arc::new(chain_for(acc(20)));
    let mut accusers = Vec::new();
    for n in 20u8..=28 {
        let engines = FakeEngines::new(Outcome::Pass);
        *engines.chunks.lock().unwrap() = chunks(claimed.0, claimed.1);
        let a = Arc::new(agent_with(
            acc(n),
            chain.clone(),
            shop(1),
            Arc::new(engines),
            Arc::default(),
        ));
        *chain.me.lock().unwrap() = Some(acc(n));
        assert!(matches!(a.audit(4, &acc(10)).await, Audited::Submitted(_)));
        chain.endpoints.lock().unwrap().insert(acc(n), endpoint(n));
        accusers.push(a);
    }
    let accused: Vec<Accuser<AccountId32>> = (20u8..=28)
        .map(|n| Accuser {
            auditor: acc(n),
            round: 4,
        })
        .collect();
    chain.disputes.lock().unwrap().insert(
        2,
        Dispute {
            provider: acc(10),
            round: 4,
            accusers: frame_support::BoundedVec::truncate_from(accused),
            reviewers: frame_support::BoundedVec::truncate_from(vec![
                (acc(5), None),
                (acc(6), None),
                (acc(7), None),
            ]),
            deadline: 120,
            outcome: None,
            closed_at: None,
            kind: DisputeKind::Statistical {
                stats_version: CURRENT_STATS.version,
            },
        },
    );
    (chain, accusers)
}

/// int8-looking chunk means: +3.0 nats per verdict under version 1, so nine cross the bound.
const INT8_MEANS: (u32, u32) = (125, 250);
/// Honest-looking chunk means.
const HONEST_MEANS: (u32, u32) = (50, 100);

#[tokio::test]
async fn reviewers_of_a_statistical_dispute_get_the_evidence() {
    // "证据保存与交付" / "统计争议的复核人取得证据".
    let (chain, accusers) = statistically_disputed(INT8_MEANS).await;
    let mine = chain.verdicts.lock().unwrap()[&(4, acc(10))]
        .iter()
        .find(|v| v.auditor == acc(20))
        .unwrap()
        .evidence
        .unwrap();
    let req = EvidenceRequest {
        dispute: 2,
        commitment: mine,
    };
    assert!(matches!(
        accusers[0].answer(&acc(6), req).await,
        EvidenceResponse::Evidence(_)
    ));
    assert_eq!(
        accusers[0].answer(&acc(9), req).await,
        EvidenceResponse::Refused
    );
}

#[tokio::test]
async fn a_statistical_dispute_is_confirmed_when_the_evidence_holds() {
    // "升级复核与投票" / "确认统计争议": the reviewer recomputes every verdict's statistics.
    let (chain, accusers) = statistically_disputed(INT8_MEANS).await;
    let (r, engines) = reviewer(5, &chain, &accusers, Outcome::Pass);
    *engines.chunks.lock().unwrap() = chunks(INT8_MEANS.0, INT8_MEANS.1);
    r.review_all().await;
    assert!(
        chain
            .sent
            .lock()
            .unwrap()
            .contains(&"vote 2 Confirm".to_owned())
    );
    assert_eq!(engines.seen.lock().unwrap().len(), 9);
}

#[tokio::test]
async fn overstated_statistics_are_rejected() {
    // "升级复核与投票" / "驳回虚报统计量": the evidence re-checks honest-looking.
    let (chain, accusers) = statistically_disputed(INT8_MEANS).await;
    let (r, engines) = reviewer(6, &chain, &accusers, Outcome::Pass);
    *engines.chunks.lock().unwrap() = chunks(HONEST_MEANS.0, HONEST_MEANS.1);
    r.review_all().await;
    assert!(
        chain
            .sent
            .lock()
            .unwrap()
            .contains(&"vote 2 Reject".to_owned())
    );
}

#[tokio::test]
async fn missing_evidence_keeps_no_positive_part() {
    // "统计争议中重算不过界": with three accusers unreachable, the six others stay below the
    // bound at the deadline.
    let (chain, accusers) = statistically_disputed(INT8_MEANS).await;
    let (r, engines) = reviewer(7, &chain, &accusers[3..], Outcome::Pass);
    *engines.chunks.lock().unwrap() = chunks(INT8_MEANS.0, INT8_MEANS.1);
    let d = chain.disputes.lock().unwrap()[&2].clone();
    assert_eq!(r.review(2, &d, false).await, super::review::Decision::Wait);
    assert_eq!(
        r.review(2, &d, true).await,
        super::review::Decision::Vote(Vote::Reject)
    );
}

#[tokio::test]
async fn evidence_in_a_statistical_state_is_kept_until_it_leaves() {
    // "证据保存与交付" / "累计归零后删除".
    let me = acc(2);
    let mut c = chain_for(me.clone());
    c.round = (8, 161, 181);
    c.block = 165;
    let chain = Arc::new(c);
    let a = agent_with(
        me.clone(),
        chain.clone(),
        shop(0),
        Arc::new(FakeEngines::new(Outcome::Pass)),
        Arc::default(),
    );
    a.store.put(4, &acc(10), &[7; 32], b"x").unwrap();
    a.store.put(4, &acc(11), &[8; 32], b"y").unwrap();
    let mut state = SprtState::<AccountId32> {
        cumulative: 3_000,
        ..Default::default()
    };
    state
        .entries
        .try_push(SprtEntry {
            auditor: me.clone(),
            round: 4,
            contribution: 3_000,
        })
        .unwrap();
    chain.states.lock().unwrap().insert(acc(10), state);
    a.tidy().await;
    // Provider 10's state holds the round-4 verdict; provider 11 has no state.
    let left: Vec<[u8; 32]> = a.store.list().iter().map(|s| s.commitment).collect();
    assert_eq!(left, vec![[7; 32]]);
    // Back at 0, the state lets it go.
    chain.states.lock().unwrap().clear();
    a.tidy().await;
    assert!(a.store.list().is_empty());
}
