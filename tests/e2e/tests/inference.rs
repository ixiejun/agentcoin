//! Inference end to end (m5-gateway-provider 7.1, 7.2; spec `engineering/ci-quality-gates`
//! item 13): on a development chain a model, two providers (different prices) and a gateway are
//! registered and a user escrows credits; two `ac-provider` agents (each in front of an
//! `ac-mock-engine`), `ac-gateway` and `ac-wallet market serve` run as processes; the official
//! OpenAI Python SDK calls through the proxy, streamed and not. The cheap provider is stopped to
//! show failover, then restarted. The gateway reports on its own; after the challenge period
//! providers and gateway have claimed. The extra time to first token against the engine
//! directly stays at or below 150 ms (p95), and no prompt or output appears in any service's
//! logs or data. The node checks every block's issuance throughout.
//! Enabled with `AC_E2E=1`; needs `cargo build -p ac-node -p ac-wallet -p ac-provider -p
//! ac-gateway -p ac-mock-engine` and a Python with `tests/e2e/python/requirements.txt`
//! (`AC_E2E_PYTHON`, default `python3`).

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    missing_docs
)]

use std::io::{BufRead, BufReader, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use ac_e2e::{TestNode, Testnet, TestnetConfig, enabled, free_port};
use ac_primitives::market::work::LifetimeWork;
use ac_runtime::{AccountId, Balance};
use parity_scale_codec::{Decode, Encode};

const START: Duration = Duration::from_secs(180);
const DEV_MNEMONIC: &str = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon art";
const MODEL_NAME: &str = "Qwen2.5-0.5B-Instruct";
const MARKER: &str = "narwhal-marker-3c7e";
/// Allowed extra time to first token through proxy, gateway and provider (p95).
const MAX_OVERHEAD_MS: f64 = 150.0;

macro_rules! require_e2e {
    () => {
        if !enabled() {
            eprintln!("end-to-end test disabled; run with AC_E2E=1 after building the binaries");
            return;
        }
    };
}

fn bin(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/debug")
        .join(name)
}

fn run(cmd: &mut Command) -> String {
    let out = cmd.output().unwrap_or_else(|e| panic!("{cmd:?}: {e}"));
    assert!(
        out.status.success(),
        "{cmd:?} failed:\n{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}

fn field(output: &str, key: &str) -> String {
    output
        .lines()
        .find_map(|l| l.strip_prefix(&format!("{key}: ")))
        .unwrap_or_else(|| panic!("no {key} in:\n{output}"))
        .trim()
        .to_string()
}

/// A long-running service process, killed on drop; its stderr goes to a log file.
struct Proc {
    child: Child,
}

impl Proc {
    /// Starts `cmd` and waits for its `listening on <addr>` line.
    fn start(mut cmd: Command, log: PathBuf) -> (Self, String) {
        let err = std::fs::File::create(&log).unwrap();
        let mut child = cmd
            .stdout(Stdio::piped())
            .stderr(err)
            .spawn()
            .unwrap_or_else(|e| panic!("{cmd:?}: {e}"));
        let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
        let deadline = Instant::now() + Duration::from_secs(120);
        let addr = loop {
            assert!(
                Instant::now() < deadline,
                "{cmd:?} did not start; see {}",
                log.display()
            );
            match lines.next() {
                Some(Ok(l)) => {
                    if let Some(a) = l.strip_prefix("listening on ") {
                        break a
                            .trim()
                            .trim_start_matches("http://")
                            .trim_end_matches("/v1")
                            .to_string();
                    }
                }
                _ => panic!(
                    "{cmd:?} exited: {}",
                    std::fs::read_to_string(&log).unwrap_or_default()
                ),
            }
        };
        // Keep draining stdout so the process never blocks on a full pipe.
        std::thread::spawn(move || for _ in lines {});
        (Self { child }, addr)
    }
}

impl Drop for Proc {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

struct W {
    file: PathBuf,
    password: PathBuf,
    node: String,
}

impl W {
    fn cmd(&self, args: &[&str]) -> Command {
        let mut cmd = Command::new(bin("ac-wallet"));
        cmd.args(args)
            .arg("--wallet")
            .arg(&self.file)
            .arg("--password-file")
            .arg(&self.password)
            .args(["--node", &self.node]);
        cmd
    }

    fn address(&self) -> String {
        run(Command::new(bin("ac-wallet"))
            .arg("address")
            .arg("--wallet")
            .arg(&self.file))
        .trim()
        .to_string()
    }
}

fn import_dev_wallet(base: &Path, node: &str) -> W {
    let password = base.join("password");
    std::fs::write(&password, "e2e-password\n").unwrap();
    let file = base.join("user.json");
    let mut import = Command::new(bin("ac-wallet"))
        .arg("import")
        .arg("--wallet")
        .arg(&file)
        .arg("--password-file")
        .arg(&password)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    writeln!(import.stdin.take().unwrap(), "{DEV_MNEMONIC}").unwrap();
    assert!(import.wait().unwrap().success());
    W {
        file,
        password,
        node: node.to_string(),
    }
}

fn funded_wallet(base: &Path, name: &str, from: &W, funds: &str) -> W {
    let w = W {
        file: base.join(format!("{name}.json")),
        password: from.password.clone(),
        node: from.node.clone(),
    };
    run(Command::new(bin("ac-wallet"))
        .arg("new")
        .arg("--wallet")
        .arg(&w.file)
        .arg("--password-file")
        .arg(&w.password)
        .stderr(Stdio::null()));
    run(&mut from.cmd(&["transfer", "--to", &w.address(), "--amount", funds]));
    w
}

async fn api<T: Decode>(node: &TestNode, method: &str, args: &[u8]) -> T {
    T::decode(&mut &node.state_call(method, args, None).await.unwrap()[..]).unwrap()
}

fn account(address: &str) -> AccountId {
    AccountId::new(ac_primitives::decode_address(address).unwrap())
}

fn python() -> String {
    std::env::var("AC_E2E_PYTHON").unwrap_or_else(|_| "python3".into())
}

/// Runs the OpenAI SDK client and returns its JSON report.
fn openai(base_url: &str, model: &str, runs: u32) -> serde_json::Value {
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("python/openai_client.py");
    let out = run(Command::new(python()).arg(script).args([
        "--base-url",
        base_url,
        "--model",
        model,
        "--runs",
        &runs.to_string(),
        "--marker",
        MARKER,
    ]));
    serde_json::from_str(&out).unwrap_or_else(|e| panic!("{e}: {out}"))
}

async fn wait_until<F, Fut>(what: &str, timeout: Duration, mut f: F)
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    let deadline = Instant::now() + timeout;
    while !f().await {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

struct ProviderSetup {
    wallet: W,
    kem_file: PathBuf,
    port: u16,
    engine_addr: String,
    toploc: PathBuf,
}

fn provider_cmd(p: &ProviderSetup, node: &str, model: &str, data: &Path) -> Command {
    let mut cmd = Command::new(bin("ac-provider"));
    cmd.arg("run")
        .arg("--wallet")
        .arg(&p.wallet.file)
        .arg("--password-file")
        .arg(&p.wallet.password)
        .arg("--kem-key")
        .arg(&p.kem_file)
        .args([
            "--node",
            node,
            "--engine",
            &format!("http://{}", p.engine_addr),
        ])
        .args([
            "--model",
            &format!("{model}=mock-model"),
            "--listen",
            &format!("127.0.0.1:{}", p.port),
        ])
        .arg("--data-dir")
        .arg(data)
        .arg("--toploc-socket")
        .arg(&p.toploc)
        .args(["--log-level", "debug"]);
    cmd
}

#[tokio::test(flavor = "multi_thread")]
async fn inference_through_the_market() {
    require_e2e!();
    let config = TestnetConfig {
        label: "inference".to_string(),
        chain: "dev".to_string(),
        authorities: 1,
        args: Vec::new(),
    };
    let net = Testnet::start_with(config, START).await.unwrap();
    net.wait_all(&[0], 1, START).await.unwrap();
    let node = &net.nodes[0];
    let base = net.base.clone();
    let user = import_dev_wallet(&base, &node.url);

    // The model.
    let manifest = base.join("model.json");
    std::fs::write(
        &manifest,
        format!(r#"{{"name":"{MODEL_NAME}","arch":"qwen2","quant":"int4","shards":["0x{}"],"licenseTag":"apache-2.0"}}"#, "01".repeat(32)),
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

    // Two engines and two providers: cheap ($0.1/$0.2) and dear ($0.3/$0.6).
    let mut engines = Vec::new();
    let mut providers = Vec::new();
    for (name, price) in [("cheap", "0.1:0.2"), ("dear", "0.3:0.6")] {
        // The engine plays the vLLM TOPLOC plugin on the provider's socket.
        let toploc = base.join(format!("{name}-toploc.sock"));
        let (engine, engine_addr) = Proc::start(
            {
                let mut c = Command::new(bin("ac-mock-engine"));
                c.args(["--listen", "127.0.0.1:0"])
                    .arg("--toploc-socket")
                    .arg(&toploc);
                c
            },
            base.join(format!("engine-{name}.log")),
        );
        engines.push(engine);
        let wallet = funded_wallet(&base, name, &user, "1000");
        let kem_file = base.join(format!("{name}-kem.json"));
        let kem = field(
            &run(Command::new(bin("ac-provider"))
                .args(["keygen", "--out"])
                .arg(&kem_file)
                .arg("--password-file")
                .arg(&wallet.password)),
            "kem key",
        );
        let port = free_port().unwrap();
        run(&mut wallet.cmd(&[
            "market",
            "provider",
            "register",
            "--tier",
            "t2",
            "--endpoint",
            &format!("http://127.0.0.1:{port}"),
            "--kem-key",
            &kem,
            "--model",
            &format!("{model}:{price}"),
        ]));
        providers.push(ProviderSetup {
            wallet,
            kem_file,
            port,
            engine_addr,
            toploc,
        });
    }
    let data = |n: &str| base.join(format!("data-{n}"));
    let mut running: Vec<Option<Proc>> = providers
        .iter()
        .zip(["cheap", "dear"])
        .map(|(p, n)| {
            Some(
                Proc::start(
                    provider_cmd(p, &node.url, &model, &data(n)),
                    base.join(format!("provider-{n}.log")),
                )
                .0,
            )
        })
        .collect();

    // The gateway (3%), reporting every 5 blocks.
    let gateway = funded_wallet(&base, "gateway", &user, "5000");
    let gw_kem = base.join("gateway-kem.json");
    run(Command::new(bin("ac-gateway"))
        .args(["keygen", "--out"])
        .arg(&gw_kem)
        .arg("--password-file")
        .arg(&gateway.password));
    let gw_port = free_port().unwrap();
    run(&mut gateway.cmd(&[
        "market",
        "gateway",
        "register",
        "--endpoint",
        &format!("http://127.0.0.1:{gw_port}"),
        "--fee-bps",
        "300",
    ]));
    let (gw_proc, _) = Proc::start(
        {
            let mut c = Command::new(bin("ac-gateway"));
            c.arg("run")
                .arg("--wallet")
                .arg(&gateway.file)
                .arg("--password-file")
                .arg(&gateway.password)
                .arg("--kem-key")
                .arg(&gw_kem)
                .args([
                    "--node",
                    &node.url,
                    "--listen",
                    &format!("127.0.0.1:{gw_port}"),
                    "--report-interval",
                    "5",
                    "--log-level",
                    "debug",
                ])
                .arg("--data-dir")
                .arg(data("gateway"));
            c
        },
        base.join("gateway.log"),
    );

    // The user escrows 10 ATC and starts the local proxy.
    let g = gateway.address();
    run(&mut user.cmd(&[
        "market",
        "escrow",
        "deposit",
        "--gateway",
        &g,
        "--amount",
        "10",
    ]));
    let proxy_port = free_port().unwrap();
    let state = base.join("serve-state.json");
    let (proxy, proxy_addr) = Proc::start(
        {
            let mut c = user.cmd(&[
                "market",
                "serve",
                "--gateway",
                &g,
                "--listen",
                &format!("127.0.0.1:{proxy_port}"),
            ]);
            c.arg("--state").arg(&state);
            c
        },
        base.join("proxy.log"),
    );
    let via_proxy = format!("http://{proxy_addr}/v1");

    // Scenario "OpenAI SDK 流式调用": streamed and non-streamed through the proxy, and the
    // latency against the engine directly.
    let direct = openai(
        &format!("http://{}/v1", providers[0].engine_addr),
        "mock-model",
        30,
    );
    let through = openai(&via_proxy, MODEL_NAME, 30);
    for s in through["streamed"].as_array().unwrap() {
        assert!(
            s["text"].as_str().unwrap().contains(MARKER),
            "the answer echoes the prompt"
        );
        assert!(s["usage"]["completion_tokens"].as_u64().unwrap() > 0);
    }
    assert!(
        through["completion"]["usage"]["prompt_tokens"]
            .as_u64()
            .unwrap()
            > 0
    );
    assert!(
        through["models"]
            .as_array()
            .unwrap()
            .iter()
            .any(|m| m.as_str() == Some(&format!("0x{}", model.trim_start_matches("0x"))))
    );
    let p95 = |v: &serde_json::Value| v["ttft_ms"]["p95"].as_f64().unwrap();
    let overhead = p95(&through) - p95(&direct);
    eprintln!(
        "time to first token p95: direct {:.1} ms, through the market {:.1} ms, overhead {overhead:.1} ms",
        p95(&direct),
        p95(&through)
    );
    assert!(
        overhead <= MAX_OVERHEAD_MS,
        "extra time to first token {overhead:.1} ms > {MAX_OVERHEAD_MS} ms"
    );

    // m5-engine-toploc 6.2: every one of the 31 requests got a receipt committing to TOPLOC
    // proofs (the gateway and the proxy checked them before billing and paying); the provider
    // keeps each co-signed receipt with its proofs.
    wait_until(
        "the cheap provider to store 31 co-signed receipts with proofs",
        Duration::from_secs(30),
        || async {
            let files: Vec<String> = std::fs::read_dir(data("cheap").join("receipts"))
                .into_iter()
                .flatten()
                .flatten()
                .filter_map(|e| std::fs::read_to_string(e.path()).ok())
                .collect();
            files.len() == 31 && files.iter().all(|t| t.contains("\"toploc\""))
        },
    )
    .await;

    // Failover: stop the cheap provider; requests are served by the dear one.
    let dear = account(&providers[1].wallet.address());
    running[0] = None;
    let after = openai(&via_proxy, MODEL_NAME, 2);
    assert_eq!(after["streamed"].as_array().unwrap().len(), 2);
    running[0] = Some(
        Proc::start(
            provider_cmd(&providers[0], &node.url, &model, &data("cheap")),
            base.join("provider-cheap-2.log"),
        )
        .0,
    );

    // The gateway reports by itself; the channel's redeemed amount reaches what the proxy paid.
    let paid: u128 =
        serde_json::from_str::<serde_json::Value>(&std::fs::read_to_string(&state).unwrap())
            .unwrap()["paid"]
            .as_str()
            .unwrap()
            .parse()
            .unwrap();
    assert!(paid > 0);
    let user_acct = account(&user.address());
    let gw_acct = account(&g);
    wait_until(
        "the report of every paid request",
        Duration::from_secs(120),
        || async {
            let ch: Option<ac_wallet::market::Channel> = api(
                node,
                "MarketApi_channel",
                &(user_acct.clone(), gw_acct.clone()).encode(),
            )
            .await;
            ch.is_some_and(|c| c.redeemed.0 == paid)
        },
    )
    .await;
    let report: Option<ac_primitives::market::work::ReportRecord<AccountId, Balance>> =
        api(node, "WorkApi_report", &0u64.encode()).await;
    let report = report.expect("report 0");
    assert_eq!(report.gateway, gw_acct);

    // m5-engine-toploc 6.2: receipts commit to TOPLOC proofs, which the gateway keeps with its
    // reports and the providers with their receipts. Before the failover every request had
    // proofs; the gateway and the proxy checked them before billing and paying.
    assert!(
        !std::fs::read_to_string(base.join("provider-cheap.log"))
            .unwrap()
            .contains("toploc_missing"),
        "every request before the failover has proofs"
    );
    let mut proven = 0usize;
    for entry in std::fs::read_dir(data("gateway").join("reports"))
        .unwrap()
        .flatten()
    {
        let v: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(entry.path()).unwrap()).unwrap();
        let proofs = v["toploc"].as_object().cloned().unwrap_or_default();
        for r in v["receipts"].as_array().unwrap() {
            let bytes = hex::decode(r.as_str().unwrap()).unwrap();
            let receipt = ac_primitives::market::SignedReceipt::decode(&mut &bytes[..]).unwrap();
            let kept = proofs.get(&hex::encode(receipt.body.request_id)).map(|h| {
                let b = hex::decode(h.as_str().unwrap()).unwrap();
                ac_market_proto::toploc::ToplocProofs::decode(&mut &b[..]).unwrap()
            });
            ac_market_proto::toploc::check(&receipt.body, kept.as_ref()).unwrap();
            if kept.is_some() {
                proven += 1;
            }
        }
    }
    assert!(
        proven > 0,
        "the gateway keeps the proofs of reported receipts"
    );

    // After the challenge period the dear provider (which served since the failover) and the
    // gateway have claimed, and the gateway deleted the receipts of matured reports.
    wait_until(
        "claims after the challenge period",
        Duration::from_secs(180),
        || async {
            let lifetime: LifetimeWork<Balance> =
                api(node, "WorkApi_lifetime", &dear.encode()).await;
            lifetime.claimed > 0
        },
    )
    .await;
    wait_until(
        "the gateway's matured receipts to be deleted",
        Duration::from_secs(120),
        || async {
            std::fs::read_dir(data("gateway").join("reports"))
                .map(|d| d.count() == 0)
                .unwrap_or(true)
        },
    )
    .await;

    // No request content anywhere in the services' logs and data.
    drop(proxy);
    drop(gw_proc);
    running.clear();
    let mut paths = vec![
        base.join("gateway.log"),
        base.join("proxy.log"),
        base.join("provider-cheap.log"),
        base.join("provider-dear.log"),
        base.join("provider-cheap-2.log"),
    ];
    let mut stack = vec![data("gateway"), data("cheap"), data("dear")];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
            if e.path().is_dir() {
                stack.push(e.path());
            } else {
                paths.push(e.path());
            }
        }
    }
    for p in paths {
        let text = String::from_utf8_lossy(&std::fs::read(&p).unwrap_or_default()).into_owned();
        assert!(
            !text.contains(MARKER),
            "{} contains request content",
            p.display()
        );
    }
    assert!(
        std::fs::read_to_string(base.join("gateway.log"))
            .unwrap()
            .contains("report"),
        "the gateway logged its reports"
    );

    // The chain kept finalizing.
    let (finalized, _) = node.finalized().await.unwrap();
    assert!(finalized > 20, "finalized {finalized}");
}
