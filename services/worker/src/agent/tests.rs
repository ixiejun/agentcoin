//! The worker's decisions against a stand-in chain and network, with the real executors
//! (m6-public-jobs 7.1, 7.4).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // Test code.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use ac_primitives::emission::EpochIndex;
use ac_primitives::market::audit::RoundIndex;
use ac_primitives::market::public::{
    Assignment, EpochPublic, JobId, JobRecord, JobSpec, PublicParams, RULES_V1, Reveal, UnitIndex,
    UnitRecord, UnitState, Url, WorkerModels, WorkerRecord, commitment,
};
use ac_primitives::market::work::JobKind;
use ac_primitives::market::{MicroUsd, ModelId};
use ac_wallet::public::{Job, Unit, Worker};
use anyhow::{Result, bail};
use async_trait::async_trait;
use sp_runtime::AccountId32;

use super::live::{Engine, Engines};
use super::ports::{Net, WorkerCall, WorkerChain};
use super::store::Store;
use super::{Agent, Ports};
use crate::exec::clean_unit;
use crate::logging::{Sink, init};
use crate::manifest::{Manifest, Shard, blake3};

const MODEL: ModelId = ModelId([7; 32]);
const MANIFEST_URL: &str = "https://data.example/m.json";
const SHARD_URL: &str = "https://data.example/0.jsonl";

static LOGS: Mutex<Vec<String>> = Mutex::new(Vec::new());

fn me() -> AccountId32 {
    AccountId32::new([1; 32])
}

#[derive(Default)]
struct State {
    now: u32,
    worker: Option<Worker>,
    assigned: Vec<Assignment<u32>>,
    jobs: BTreeMap<JobId, Job>,
    units: BTreeMap<(JobId, UnitIndex), Unit>,
    pending: Vec<(EpochIndex, u128)>,
    epochs: BTreeMap<EpochIndex, EpochPublic<u128>>,
    locked: Vec<(u32, u128)>,
    sent: Vec<WorkerCall>,
}

#[derive(Default)]
struct FakeChain(Mutex<State>);

impl FakeChain {
    fn with<R>(&self, f: impl FnOnce(&mut State) -> R) -> R {
        f(&mut self.0.lock().unwrap())
    }

    fn sent(&self) -> Vec<WorkerCall> {
        self.with(|s| s.sent.clone())
    }
}

#[async_trait]
impl WorkerChain for FakeChain {
    async fn best_block(&self) -> Result<u32> {
        Ok(self.with(|s| s.now))
    }
    async fn params(&self) -> Result<Option<PublicParams>> {
        Ok(Some(PublicParams::DEV))
    }
    async fn round(&self) -> Result<Option<(RoundIndex, u32, u32)>> {
        Ok(self.with(|s| {
            let r = s.now / 10;
            Some((r, r * 10, r * 10 + 9))
        }))
    }
    async fn worker(&self, _: &AccountId32) -> Result<Option<Worker>> {
        Ok(self.with(|s| s.worker.clone()))
    }
    async fn assigned(&self, _: &AccountId32) -> Result<Vec<Assignment<u32>>> {
        Ok(self.with(|s| s.assigned.clone()))
    }
    async fn job(&self, job: JobId) -> Result<Option<Job>> {
        Ok(self.with(|s| s.jobs.get(&job).cloned()))
    }
    async fn unit(&self, job: JobId, unit: UnitIndex) -> Result<Option<Unit>> {
        Ok(self.with(|s| s.units.get(&(job, unit)).cloned()))
    }
    async fn pending(&self, _: &AccountId32) -> Result<Vec<(EpochIndex, u128)>> {
        Ok(self.with(|s| s.pending.clone()))
    }
    async fn epoch(&self, epoch: EpochIndex) -> Result<EpochPublic<u128>> {
        Ok(self.with(|s| s.epochs.get(&epoch).copied().unwrap_or_default()))
    }
    async fn locked(&self, _: &AccountId32) -> Result<Vec<(u32, u128)>> {
        Ok(self.with(|s| s.locked.clone()))
    }
    async fn submit(&self, call: WorkerCall) -> Result<bool> {
        self.with(|s| {
            let round = s.now / 10;
            match &call {
                WorkerCall::Ready => {
                    if let Some(w) = s.worker.as_mut() {
                        w.last_ready = Some(round);
                    }
                }
                WorkerCall::SetModels(m) => {
                    if let Some(w) = s.worker.as_mut() {
                        w.models = m.clone();
                    }
                }
                WorkerCall::Commit { job, unit, .. } => {
                    for a in &mut s.assigned {
                        if (a.job, a.unit) == (*job, *unit) {
                            a.committed = true;
                        }
                    }
                }
                WorkerCall::Reveal { job, unit, .. } => {
                    for a in &mut s.assigned {
                        if (a.job, a.unit) == (*job, *unit) {
                            a.revealed = true;
                        }
                    }
                }
                WorkerCall::Claim(epochs) => s.pending.retain(|(e, _)| !epochs.contains(e)),
                WorkerCall::Withdraw => {
                    let now = s.now;
                    s.locked.retain(|(at, _)| *at > now);
                }
            }
            s.sent.push(call);
            Ok(true)
        })
    }
}

#[derive(Default)]
struct FakeNet {
    files: Mutex<BTreeMap<String, Vec<u8>>>,
    refuse_puts: std::sync::atomic::AtomicBool,
    puts: Mutex<Vec<(String, AccountId32, Vec<u8>)>>,
}

#[async_trait]
impl Net for FakeNet {
    async fn get(&self, url: &str, limit: usize) -> Result<Vec<u8>> {
        match self.files.lock().unwrap().get(url) {
            Some(b) if b.len() <= limit => Ok(b.clone()),
            _ => bail!("not found"),
        }
    }
    async fn put(&self, url: &str, worker: &AccountId32, body: Vec<u8>) -> Result<()> {
        if self.refuse_puts.load(std::sync::atomic::Ordering::SeqCst) {
            bail!("collector down");
        }
        self.puts
            .lock()
            .unwrap()
            .push((url.to_string(), worker.clone(), body));
        Ok(())
    }
}

fn spec(kind: JobKind, shard: &[u8]) -> JobSpec {
    JobSpec {
        kind,
        model: (kind != JobKind::DataClean).then_some(MODEL),
        rules: RULES_V1,
        manifest_hash: blake3(&manifest_of(shard)),
        manifest_url: Url::truncate_from(MANIFEST_URL.as_bytes().to_vec()),
        results_url: Url::truncate_from(b"https://results.example/".to_vec()),
        units: 1,
        price: MicroUsd(10_000),
        canary_root: None,
    }
}

fn manifest_of(shard: &[u8]) -> Vec<u8> {
    serde_json::to_vec(&Manifest {
        units: vec![Shard {
            url: SHARD_URL.into(),
            blake3: hex::encode(blake3(shard)),
            items: 1,
        }],
    })
    .unwrap()
}

struct Setup {
    chain: Arc<FakeChain>,
    net: Arc<FakeNet>,
    agent: Agent,
    dir: std::path::PathBuf,
}

/// A worker assigned unit 0 of job 0 (`kind`), with `shard` served and `served` as the bytes
/// actually downloaded; evaluation and embedding use an engine at `engine`.
fn setup(name: &str, kind: JobKind, shard: &[u8], served: &[u8], engine: &str) -> Setup {
    init(log::LevelFilter::Info, Sink::Buffer(&LOGS));
    let chain = Arc::new(FakeChain::default());
    let net = Arc::new(FakeNet::default());
    let spec = spec(kind, shard);
    net.files
        .lock()
        .unwrap()
        .insert(MANIFEST_URL.into(), manifest_of(shard));
    net.files
        .lock()
        .unwrap()
        .insert(SHARD_URL.into(), served.to_vec());
    chain.with(|s| {
        s.now = 5;
        s.worker = Some(WorkerRecord {
            models: WorkerModels::truncate_from(vec![MODEL]),
            last_ready: None,
            misses: 0,
            suspended_until: None,
            accepted: 0,
            missed: 0,
        });
        s.jobs.insert(
            0,
            JobRecord {
                spec,
                published_at: 1,
                opened: 1,
                accepted: 0,
                failed: 0,
                cancelled: false,
            },
        );
        s.assigned.push(Assignment {
            job: 0,
            unit: 0,
            attempt: 1,
            commit_by: 25,
            reveal_by: 35,
            committed: false,
            revealed: false,
        });
    });
    let mut engines = BTreeMap::new();
    engines.insert(
        MODEL,
        Engine {
            client: crate::EngineClient::new(engine).unwrap(),
            model: "mock-model".into(),
            busy: tokio::sync::Mutex::new(()),
        },
    );
    let dir = std::env::temp_dir().join(format!("ac-worker-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let agent = Agent::new(
        Ports {
            chain: Arc::clone(&chain) as Arc<dyn WorkerChain>,
            net: Arc::clone(&net) as Arc<dyn Net>,
            exec: Arc::new(Engines { engines }),
        },
        me(),
        Store::open(&dir).unwrap(),
        Box::new(|| Ok([9; 32])),
    );
    Setup {
        chain,
        net,
        agent,
        dir,
    }
}

/// Ticks until an execution has finished (it runs in the background).
async fn tick_until_settled(s: &mut Setup) {
    for _ in 0..200 {
        s.agent.tick().await.unwrap();
        let c = &s.agent.counters;
        if c.executed + c.data_errors + c.execution_errors > 0 {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!("the unit never finished");
}

fn marker_shard(marker: &str) -> Vec<u8> {
    format!(
        "{{\"text\":\"A document about {marker} that is long enough to be kept by the rules.\"}}\n"
    )
    .into_bytes()
}

// Spec "全自动完成一个单元" (with the stand-in chain): ready, execution, commitment, reveal after
// the deadline, upload once accepted in the majority, claim and withdrawal. Spec "工作者服务"
// (7.4): no log line holds the data.
#[tokio::test]
async fn a_clean_unit_goes_from_assignment_to_withdrawal() {
    let marker = "zq-marker-7f3a";
    let shard = marker_shard(marker);
    let mut s = setup(
        "full",
        JobKind::DataClean,
        &shard,
        &shard,
        "http://127.0.0.1:1",
    );
    tick_until_settled(&mut s).await;

    let expected = clean_unit(&crate::manifest::parse_texts(&shard, 10).unwrap()).unwrap();
    let hash = commitment(&Reveal {
        job: 0,
        unit: 0,
        attempt: 1,
        worker: &me(),
        summary: &expected.summary,
        result_hash: &expected.result_hash,
        salt: &[9; 32],
    });
    let sent = s.chain.sent();
    assert_eq!(sent.first(), Some(&WorkerCall::Ready));
    assert!(sent.contains(&WorkerCall::Commit {
        job: 0,
        unit: 0,
        hash
    }));

    // Nothing is revealed before the deadline; after it, the reveal opens the commitment.
    s.agent.tick().await.unwrap();
    assert_eq!(s.agent.counters.reveals, 0);
    s.chain.with(|st| st.now = 26);
    s.agent.tick().await.unwrap();
    let reveal = s
        .chain
        .sent()
        .into_iter()
        .find_map(|c| match c {
            WorkerCall::Reveal { reveal, .. } => Some(reveal),
            _ => None,
        })
        .unwrap();
    assert_eq!(reveal.summary.to_vec(), expected.summary);
    assert_eq!(reveal.result_hash, expected.result_hash);
    assert_eq!(reveal.salt, [9; 32]);

    // The unit passes with this worker in the majority: the full result is uploaded once, after
    // a retry while the collector is down.
    s.chain.with(|st| {
        st.now = 30;
        st.assigned.clear();
        st.units.insert(
            (0, 0),
            UnitRecord {
                attempt: 1,
                opened_at: 5,
                commit_by: 25,
                reveal_by: 35,
                assigned: [AccountId32::new([2; 32]), me(), AccountId32::new([3; 32])],
                tried: Default::default(),
                commits: [None; 3],
                reveals: Default::default(),
                state: UnitState::Accepted {
                    reference: 0,
                    majority: [true, true, false],
                },
                settled_at: Some(30),
                canary_revealed: false,
            },
        );
    });
    s.net
        .refuse_puts
        .store(true, std::sync::atomic::Ordering::SeqCst);
    s.agent.tick().await.unwrap();
    assert_eq!(s.agent.counters.uploads, 0);
    s.net
        .refuse_puts
        .store(false, std::sync::atomic::Ordering::SeqCst);
    s.agent.tick().await.unwrap();
    s.agent.tick().await.unwrap();
    {
        let puts = s.net.puts.lock().unwrap();
        assert_eq!(puts.len(), 1);
        let (url, who, body) = &puts[0];
        assert_eq!(url, "https://results.example/0/0");
        assert_eq!(who, &me());
        assert_eq!(body, &expected.result);
    }

    // The epoch settles: claim; the lock ends: withdraw.
    s.chain.with(|st| {
        st.pending = vec![(3, 100)];
        st.epochs.insert(
            3,
            EpochPublic {
                verified: 100,
                emission: Some(1_000),
            },
        );
        st.locked = vec![(40, 1_000)];
    });
    s.agent.tick().await.unwrap();
    assert!(s.chain.sent().contains(&WorkerCall::Claim(vec![3])));
    assert_eq!(s.agent.counters.withdrawals, 0);
    s.chain.with(|st| st.now = 40);
    s.agent.tick().await.unwrap();
    assert_eq!(s.chain.sent().last(), Some(&WorkerCall::Withdraw));

    // Pruned: the local copy goes.
    s.chain.with(|st| st.units.clear());
    s.agent.tick().await.unwrap();
    assert!(s.agent.store.keys().is_empty());

    let logs = LOGS.lock().unwrap();
    assert!(!logs.is_empty());
    assert!(logs.iter().all(|l| !l.contains(marker)), "{logs:?}");
    let _ = std::fs::remove_dir_all(&s.dir);
}

// Spec "引擎不可用不提交".
#[tokio::test]
async fn a_down_engine_commits_nothing() {
    let shard = b"{\"context\":\"zq-eval-marker 1 + 1 =\",\"choices\":[\" 2\",\" 3\"]}\n";
    let mut s = setup("down", JobKind::Eval, shard, shard, "http://127.0.0.1:1");
    tick_until_settled(&mut s).await;
    assert_eq!(s.agent.counters.execution_errors, 1);
    s.agent.tick().await.unwrap();
    assert!(
        !s.chain
            .sent()
            .iter()
            .any(|c| matches!(c, WorkerCall::Commit { .. }))
    );
    let logs = LOGS.lock().unwrap();
    assert!(logs.iter().all(|l| !l.contains("zq-eval-marker")));
    let _ = std::fs::remove_dir_all(&s.dir);
}

// Spec "分片被篡改".
#[tokio::test]
async fn a_tampered_shard_commits_nothing() {
    let shard = marker_shard("original");
    let served = marker_shard("tampered");
    let mut s = setup(
        "tampered",
        JobKind::DataClean,
        &shard,
        &served,
        "http://127.0.0.1:1",
    );
    tick_until_settled(&mut s).await;
    assert_eq!(s.agent.counters.data_errors, 1);
    assert_eq!(s.agent.counters.executed, 0);
    assert!(
        !s.chain
            .sent()
            .iter()
            .any(|c| matches!(c, WorkerCall::Commit { .. }))
    );
    let _ = std::fs::remove_dir_all(&s.dir);
}

// An evaluation unit through the mock engine, and the start-up checks.
#[tokio::test]
async fn an_eval_unit_runs_on_the_engine_and_preflight_syncs_models() {
    let engine = ac_mock_engine::spawn("127.0.0.1:0", ac_mock_engine::Config::default())
        .await
        .unwrap();
    let shard = b"{\"context\":\"Which is larger:\",\"choices\":[\" one\",\" two\",\" three\"]}\n";
    let mut s = setup("eval", JobKind::Eval, shard, shard, &engine.url());
    s.agent
        .preflight(&WorkerModels::truncate_from(vec![MODEL]))
        .await
        .unwrap();
    assert!(s.chain.sent().is_empty());
    let other = WorkerModels::truncate_from(vec![MODEL, ModelId([8; 32])]);
    s.agent.preflight(&other).await.unwrap();
    assert_eq!(s.chain.sent(), vec![WorkerCall::SetModels(other)]);
    tick_until_settled(&mut s).await;
    assert_eq!(s.agent.counters.executed, 1);
    assert_eq!(s.agent.counters.commits, 1);

    s.chain.with(|st| st.worker = None);
    assert!(s.agent.preflight(&WorkerModels::default()).await.is_err());
    let _ = std::fs::remove_dir_all(&s.dir);
}
