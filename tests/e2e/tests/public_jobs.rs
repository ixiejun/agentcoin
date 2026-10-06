//! Public jobs end to end (m6-public-jobs 8.3; spec `engineering/ci-quality-gates` "公共任务的
//! 端到端验收测试"): on a development chain (the public job parameters of the `dev` and `local`
//! presets) a model is registered, five workers run `ac-worker run` before deterministic mock
//! engines and the publisher runs `ac-worker collect`. The administration publishes a data
//! cleaning job and an evaluation job whose every unit is a canary. Two of the workers collude:
//! their engines play another model, so they reveal the same forged evaluation summaries.
//!
//! The test checks that units pass and their results are collected, that the colluders are
//! punished (unclaimed work voided, locked rewards burned, suspended) once a canary unit they
//! carried by majority is revealed, that an honest worker's rewards are claimed through public
//! work emission and withdrawn after the lock, that no worker or collector log holds the data,
//! and that burns are counted in the cumulative burned amount; the node checks issuance on every
//! block throughout. If no canary unit falls to the colluders, another evaluation job is
//! published, up to 40 evaluation units in all.
//!
//! Enabled with `AC_E2E=1`; needs `cargo build -p ac-node -p ac-wallet -p ac-worker
//! -p ac-mock-engine`.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    missing_docs
)]

mod common;

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant};

use ac_crypto::SigAlg;
use ac_crypto::sig::SigningKey;
use ac_e2e::{TestNode, Testnet, TestnetConfig};
use ac_primitives::market::public::{UnitState, WorkerRecord};
use ac_runtime::transaction::{TxParams, assemble, authorized_extensions, implicit_from, payload};
use ac_runtime::{AccountId, RuntimeCall, RuntimeEvent};
use ac_wallet::NodeClient;
use ac_wallet::http::{self, Request, Response};
use common::{Proc, W, account, api, bin, field, funded_wallet, import_dev_wallet, run};
use jsonrpsee::core::client::ClientT;
use jsonrpsee::rpc_params;
use parity_scale_codec::{Decode, Encode};
use serde_json::json;
use sp_runtime::generic::Era;

const START: Duration = Duration::from_secs(180);
const MARKER: &str = "quokka-marker-2d71";
const WORKERS: usize = 5;
/// Workers 3 and 4 collude.
const COLLUDERS: [usize; 2] = [3, 4];
const CLEAN_UNITS: u32 = 4;
const EVAL_UNITS: u32 = 20;
const MAX_EVAL_UNITS: u32 = 40;

/// A transaction signed by a development account (the administration on the dev chain is alice
/// alone, threshold 1).
async fn submit_as(client: &NodeClient, name: &str, call: RuntimeCall) -> bool {
    let key = SigningKey::from_seed(SigAlg::MlDsa44, &ac_crypto::dev_seed(name).unwrap()).unwrap();
    let who = pallet_pq_accounts::derived_account(&key.public_key().unwrap());
    let context = client.chain_context().await.unwrap();
    let params = TxParams {
        nonce: client.nonce(&who).await.unwrap(),
        tip: 0,
        era: Era::Immortal,
        era_birth_hash: context.genesis_hash,
    };
    let extensions = authorized_extensions(&params);
    let digest = payload(&call, &extensions, &implicit_from(&context, &params)).unwrap();
    let mut rng = ac_crypto::OsRng::new().unwrap();
    let signature = key
        .sign(&digest, pallet_pq_accounts::TX_SIGNING_CONTEXT, &mut rng)
        .unwrap();
    let first = client.current_key(&who).await.unwrap().is_none();
    let xt = assemble(
        call,
        who,
        signature,
        first.then(|| key.public_key().unwrap()),
        extensions,
    );
    client
        .submit_and_watch(&xt, Duration::from_secs(60))
        .await
        .unwrap()
        .success
}

/// The administration publishes the call `ac-wallet public publish` printed.
async fn admin_publish(client: &NodeClient, printed: &str) {
    let bytes = hex::decode(field(printed, "call").trim_start_matches("0x")).unwrap();
    let call = RuntimeCall::decode(&mut &bytes[..]).unwrap();
    let propose = RuntimeCall::PoaCouncil(pallet_collective::Call::propose {
        threshold: 1,
        length_bound: u32::try_from(bytes.len()).unwrap(),
        proposal: Box::new(call),
    });
    assert!(submit_as(client, "alice", propose).await);
}

/// Serves the job data (manifests and shards) over HTTP on `listener`.
fn serve_data(listener: tokio::net::TcpListener, files: Arc<BTreeMap<String, Vec<u8>>>) {
    tokio::spawn(http::serve(listener, move |req: Request| {
        let files = Arc::clone(&files);
        async move {
            let path = req.uri().path().trim_start_matches('/').to_string();
            let resp: Response = match files.get(&path) {
                Some(b) => http::full(200, "application/octet-stream", b.clone()),
                None => http::full(404, "text/plain", ""),
            };
            resp
        }
    }));
}

/// A read-only wallet command against the node (no password needed).
fn query(w: &W, args: &[&str]) -> Command {
    let mut cmd = Command::new(bin("ac-wallet"));
    cmd.args(args).args(["--node", &w.node]);
    cmd
}

/// A job's shards and manifest, named `<prefix>-<unit>.jsonl` and `<prefix>.json`.
fn job_data(
    files: &mut BTreeMap<String, Vec<u8>>,
    base_url: &str,
    prefix: &str,
    shards: Vec<Vec<u8>>,
) -> Vec<u8> {
    let units: Vec<_> = shards
        .into_iter()
        .enumerate()
        .map(|(i, shard)| {
            let name = format!("{prefix}-{i}.jsonl");
            let entry = json!({
                "url": format!("{base_url}/{name}"),
                "blake3": hex::encode(ac_crypto::hash::blake3_256(&shard)),
                "items": shard.iter().filter(|b| **b == b'\n').count(),
            });
            files.insert(name, shard);
            entry
        })
        .collect();
    let manifest = serde_json::to_vec(&json!({ "units": units })).unwrap();
    files.insert(format!("{prefix}.json"), manifest.clone());
    manifest
}

fn clean_shard(unit: u32) -> Vec<u8> {
    (0..3)
        .map(|d| {
            json!({"text": format!(
                "Document {d} of unit {unit} mentions {MARKER} and the tides of a northern bay."
            )})
            .to_string()
                + "\n"
        })
        .collect::<String>()
        .into_bytes()
}

fn eval_shard(job: u32, unit: u32) -> Vec<u8> {
    (0..20)
        .map(|i| {
            json!({
                "context": format!("Job {job} unit {unit} question {i} about {MARKER}: the answer is"),
                "choices": [" alpha", " beta", " gamma", " delta"],
            })
            .to_string()
                + "\n"
        })
        .collect::<String>()
        .into_bytes()
}

/// Events of block `hash`.
async fn events(node: &TestNode, hash: &str) -> Vec<RuntimeEvent> {
    let key = frame_support::storage::storage_prefix(b"System", b"Events");
    let key = format!("0x{}", hex::encode(key));
    let raw: Option<String> = node
        .rpc()
        .unwrap()
        .request("state_getStorage", rpc_params![key, hash])
        .await
        .unwrap();
    let Some(raw) = raw else { return Vec::new() };
    let bytes = ac_e2e::unhex(&raw).unwrap();
    Vec::<frame_system::EventRecord<RuntimeEvent, sp_core::H256>>::decode(&mut &bytes[..])
        .unwrap()
        .into_iter()
        .map(|r| r.event)
        .collect()
}

/// What the test watches for in the blocks' events.
#[derive(Default)]
struct Seen {
    scanned: u64,
    /// (who, burned, voided, suspended in that block).
    punished: Vec<(AccountId, u128, u128, bool)>,
    canary_failed: BTreeSet<(u32, u32)>,
    accepted: BTreeSet<(u32, u32)>,
}

impl Seen {
    async fn scan(&mut self, node: &TestNode) {
        let height = node.height().await.unwrap_or(0);
        while self.scanned < height {
            self.scanned += 1;
            let Some(hash) = node.hash_at(self.scanned).await else {
                return;
            };
            for e in events(node, &hash).await {
                if let RuntimeEvent::PublicJobs(e) = e {
                    match e {
                        pallet_public_jobs::Event::Punished {
                            who,
                            burned,
                            voided,
                        } => {
                            let record: Option<WorkerRecord<u32>> = Decode::decode(
                                &mut &node
                                    .state_call("PublicJobsApi_worker", &who.encode(), Some(&hash))
                                    .await
                                    .unwrap()[..],
                            )
                            .unwrap();
                            let suspended = record.is_some_and(|w| w.suspended_until.is_some());
                            self.punished.push((who, burned, voided, suspended));
                        }
                        pallet_public_jobs::Event::CanaryFailed { job, unit } => {
                            self.canary_failed.insert((job, unit));
                        }
                        pallet_public_jobs::Event::UnitAccepted { job, unit, .. } => {
                            self.accepted.insert((job, unit));
                        }
                        _ => {}
                    }
                }
            }
        }
    }
}

async fn total_burned(node: &TestNode) -> u128 {
    api(node, "EmissionApi_total_burned", &[]).await
}

async fn worker_record(node: &TestNode, who: &AccountId) -> Option<WorkerRecord<u32>> {
    api(node, "PublicJobsApi_worker", &who.encode()).await
}

async fn free(client: &NodeClient, who: &AccountId) -> u128 {
    client.free_balance(who).await.unwrap()
}

fn worker_config(base: &Path, i: usize, w: &W, model: &str, engine: &str) -> PathBuf {
    let cfg = json!({
        "node": w.node, "wallet": w.file, "password_file": w.password,
        "data_dir": base.join(format!("worker{i}-data")),
        "engines": [{"model": model, "engine": engine, "engine_model": "mock-model"}],
        "poll_ms": 500,
    });
    let file = base.join(format!("worker{i}-config.json"));
    std::fs::write(&file, cfg.to_string()).unwrap();
    file
}

#[tokio::test(flavor = "multi_thread")]
async fn colluders_are_caught_and_honest_workers_paid() {
    require_e2e!();
    let config = TestnetConfig {
        label: "public-jobs".to_string(),
        chain: "dev".to_string(),
        authorities: 1,
        args: Vec::new(),
    };
    let net = Testnet::start_with(config, START).await.unwrap();
    net.wait_all(&[0], 1, START).await.unwrap();
    let node = &net.nodes[0];
    let base = net.base.clone();
    let client = NodeClient::new(&node.url).unwrap();
    let user = import_dev_wallet(&base, &node.url);
    let burned_at_start = total_burned(node).await;

    // The evaluation model.
    let manifest = base.join("model.json");
    std::fs::write(
        &manifest,
        format!(r#"{{"name":"mock","arch":"qwen2","quant":"bf16","shards":["0x{}"],"licenseTag":"apache-2.0"}}"#, "06".repeat(32)),
    )
    .unwrap();
    let model = field(
        &run(&mut user.cmd(&[
            "market",
            "model",
            "register",
            "--file",
            manifest.to_str().unwrap(),
        ])),
        "model id",
    );

    // Engines: the registered model, and the other model the colluders run.
    let (_honest_engine, honest_engine) = Proc::start(
        {
            let mut c = Command::new(bin("ac-mock-engine"));
            c.args(["--listen", "127.0.0.1:0", "--model-seed", "0"]);
            c
        },
        base.join("engine-honest.log"),
    );
    let (_other_engine, other_engine) = Proc::start(
        {
            let mut c = Command::new(bin("ac-mock-engine"));
            c.args(["--listen", "127.0.0.1:0", "--model-seed", "1"]);
            c
        },
        base.join("engine-colluders.log"),
    );
    let honest_engine = format!("http://{honest_engine}");
    let other_engine = format!("http://{other_engine}");

    // The data: a cleaning job (job 0) and up to two evaluation jobs (jobs 1 and 2). The
    // manifests carry the server's URL, so the listener is bound first.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let data_url = format!("http://{}", listener.local_addr().unwrap());
    let mut files = BTreeMap::new();
    let clean_manifest = job_data(
        &mut files,
        &data_url,
        "clean",
        (0..CLEAN_UNITS).map(clean_shard).collect(),
    );
    let eval_manifests: Vec<Vec<u8>> = [1u32, 2]
        .iter()
        .map(|job| {
            job_data(
                &mut files,
                &data_url,
                &format!("eval{job}"),
                (0..EVAL_UNITS).map(|u| eval_shard(*job, u)).collect(),
            )
        })
        .collect();
    serve_data(listener, Arc::new(files));
    for (name, bytes) in [("clean.json", &clean_manifest)].into_iter().chain(
        eval_manifests
            .iter()
            .enumerate()
            .map(|(i, m)| (["eval1.json", "eval2.json"][i], m)),
    ) {
        std::fs::write(base.join(name), bytes).unwrap();
    }

    // Canaries for every evaluation unit, computed with the registered model.
    let units = (0..EVAL_UNITS)
        .map(|u| u.to_string())
        .collect::<Vec<_>>()
        .join(",");
    let mut roots = Vec::new();
    for job in [1u32, 2] {
        let out = run(Command::new(bin("ac-worker"))
            .args(["canary", "--job", &job.to_string(), "--kind", "eval"])
            .arg("--manifest")
            .arg(base.join(format!("eval{job}.json")))
            .args(["--units", &units, "--engine", &honest_engine])
            .args(["--model", "mock-model", "--out"])
            .arg(base.join(format!("canaries{job}.json"))));
        roots.push(field(&out, "canary root"));
    }

    // The collector, revealing canaries with the dev wallet.
    let collect_port = ac_e2e::free_port().unwrap();
    let collect_cfg = base.join("collect.json");
    std::fs::write(
        &collect_cfg,
        json!({
            "node": node.url, "listen": format!("127.0.0.1:{collect_port}"),
            "dir": base.join("results"),
            "wallet": user.file, "password_file": user.password,
            "canaries": [base.join("canaries1.json"), base.join("canaries2.json")],
            "poll_ms": 500,
        })
        .to_string(),
    )
    .unwrap();
    let (_collector, _) = Proc::start(
        {
            let mut c = Command::new(bin("ac-worker"));
            c.args(["collect", "--config"]).arg(&collect_cfg);
            c
        },
        base.join("collector.log"),
    );
    let results_url = format!("http://127.0.0.1:{collect_port}");

    // Five workers.
    let mut wallets = Vec::new();
    let mut workers = Vec::new();
    for i in 0..WORKERS {
        let w = funded_wallet(&base, &format!("worker{i}"), &user, "100");
        run(&mut w.cmd(&["public", "worker", "register", "--model", &model]));
        let engine = if COLLUDERS.contains(&i) {
            &other_engine
        } else {
            &honest_engine
        };
        let cfg = worker_config(&base, i, &w, &model, engine);
        let mut c = Command::new(bin("ac-worker"));
        c.args(["run", "--config"]).arg(&cfg);
        workers.push(Proc::spawn(c, &base.join(format!("worker{i}.log"))));
        wallets.push(w);
    }
    let accounts: Vec<AccountId> = wallets.iter().map(|w| account(&w.address())).collect();
    let honest = accounts[0].clone();
    let baseline = free(&client, &honest).await;

    // The administration publishes the cleaning job.
    let published = Instant::now();
    let printed = run(&mut query(
        &user,
        &[
            "public",
            "publish",
            "--kind",
            "clean",
            "--manifest",
            base.join("clean.json").to_str().unwrap(),
            "--manifest-url",
            &format!("{data_url}/clean.json"),
            "--results-url",
            &results_url,
            "--units",
            &CLEAN_UNITS.to_string(),
            "--price",
            "0.05",
        ],
    ));
    admin_publish(&client, &printed).await;
    let publish_eval = |job: u32| {
        run(&mut query(
            &user,
            &[
                "public",
                "publish",
                "--kind",
                "eval",
                "--model",
                &model,
                "--manifest",
                base.join(format!("eval{job}.json")).to_str().unwrap(),
                "--manifest-url",
                &format!("{data_url}/eval{job}.json"),
                "--results-url",
                &results_url,
                "--units",
                &EVAL_UNITS.to_string(),
                "--price",
                "0.05",
                "--canary-root",
                &roots[usize::try_from(job - 1).unwrap()],
            ],
        ))
    };
    admin_publish(&client, &publish_eval(1)).await;
    let mut eval_jobs = 1u32;

    // Watch the chain until both colluders are punished, publishing a second evaluation job if
    // the first ends without a canary unit carried by them.
    let mut seen = Seen::default();
    let mut first_accepted = None;
    let deadline = Instant::now() + Duration::from_secs(900);
    let colluders: Vec<AccountId> = COLLUDERS.iter().map(|i| accounts[*i].clone()).collect();
    loop {
        assert!(
            Instant::now() < deadline,
            "colluders not caught in time; logs in {}",
            base.display()
        );
        seen.scan(node).await;
        if first_accepted.is_none() && seen.accepted.iter().any(|(j, _)| *j == 0) {
            first_accepted = Some(published.elapsed());
        }
        let caught: BTreeSet<_> = seen.punished.iter().map(|p| p.0.clone()).collect();
        if colluders.iter().all(|c| caught.contains(c)) {
            break;
        }
        let job = client.public_job(eval_jobs).await.unwrap();
        let finished = job
            .as_ref()
            .is_some_and(|j| j.accepted + j.failed >= j.spec.units);
        if finished && eval_jobs * EVAL_UNITS < MAX_EVAL_UNITS {
            eval_jobs += 1;
            admin_publish(&client, &publish_eval(eval_jobs)).await;
        }
        tokio::time::sleep(Duration::from_millis(1_000)).await;
    }

    // Spec "本地链几分钟内完成一个单元": the first cleaning unit passed within minutes.
    let first = first_accepted.expect("no cleaning unit passed");
    assert!(
        first < Duration::from_secs(300),
        "first unit after {first:?}"
    );

    // Spec "串通被金丝雀查出": both colluders punished (their unclaimed work for the unit they
    // carried is voided at least) and suspended; no honest worker is.
    for c in &colluders {
        let (_, _, voided, suspended) = seen.punished.iter().find(|p| p.0 == *c).unwrap();
        assert!(*voided > 0, "nothing voided for a colluder");
        assert!(*suspended, "a punished colluder is not suspended");
        assert!(worker_record(node, c).await.is_some());
    }
    assert!(!seen.canary_failed.is_empty());
    for (who, _, _, _) in &seen.punished {
        assert!(colluders.contains(who), "an honest worker was punished");
    }

    // Every passed unit's result is collected; the cleaning job finishes.
    wait_for("the cleaning job", Duration::from_secs(300), || async {
        client
            .public_job(0)
            .await
            .unwrap()
            .is_some_and(|j| j.accepted == CLEAN_UNITS)
    })
    .await;
    wait_for("collected results", Duration::from_secs(120), || async {
        let mut all = true;
        for (job, unit) in &seen.accepted {
            let failed = matches!(
                client
                    .public_unit(*job, *unit)
                    .await
                    .unwrap()
                    .map(|u| u.state),
                Some(UnitState::Failed)
            );
            if !failed && !base.join(format!("results/{job}/{unit}")).exists() {
                all = false;
            }
        }
        all
    })
    .await;

    // An honest worker's rewards: claimed through public work emission, then withdrawn.
    wait_for(
        "an honest worker's withdrawn rewards",
        Duration::from_secs(300),
        || async {
            client.public_locked(&honest).await.unwrap().is_empty()
                && client.public_pending(&honest).await.unwrap().is_empty()
                && free(&client, &honest).await > baseline
        },
    )
    .await;

    // Burns are in the cumulative burned amount.
    seen.scan(node).await;
    let burned: u128 = seen.punished.iter().map(|p| p.1).sum();
    assert!(total_burned(node).await - burned_at_start >= burned);

    // No log holds the data.
    for entry in std::fs::read_dir(&base).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        if (name.starts_with("worker") || name.starts_with("collector")) && name.ends_with(".log") {
            let text = std::fs::read_to_string(&path).unwrap();
            assert!(!text.contains(MARKER), "{} holds the data", path.display());
            assert!(!text.is_empty() || name.starts_with("collector"));
        }
    }
    // The node accepted every block: its issuance check never rejected one.
    let node_log = std::fs::read_to_string(node.log_path()).unwrap_or_default();
    assert!(!node_log.contains("rejecting block"));
    drop(workers);
}

async fn wait_for<F, Fut>(what: &str, timeout: Duration, mut f: F)
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    let deadline = Instant::now() + timeout;
    while !f().await {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(1_000)).await;
    }
}
