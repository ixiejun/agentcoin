//! Audit detection latency end to end (m6-auditor-agent 7.2; spec `engineering/ci-quality-gates`
//! "审计检出延迟的验收测试"): on a development chain (20-block rounds, 3 reviewers, quorum 2) a
//! bfloat16 model, two providers (`ac-provider` before an `ac-mock-engine` in prove mode), a
//! gateway (`ac-gateway`) and six auditors are registered. Every auditor runs `ac-auditor run`
//! with its own verify-mode engine of the registered model and its own payment account, and
//! asks only prompts from a bank carrying a marker. Nothing is driven by hand: the agents send
//! pinned requests, re-check, submit verdicts, serve evidence and vote.
//!
//! After a full round of honest service, one provider's engine starts playing another model in
//! the middle of a round. The test checks that this provider is confirmed, slashed and jailed
//! within three rounds of the switch and is no longer serviceable, that the honest provider gets
//! no failing verdict and no dispute, that no auditor is slashed, and that no log holds the
//! marker; the node checks issuance on every block throughout. It prints the blocks from the
//! switch to the first failing verdict, to the dispute and to the jail (and writes them to
//! `$AC_E2E_LATENCY` if set, for the CI summary and the latency report).
//!
//! Enabled with `AC_E2E=1`; needs `cargo build -p ac-node -p ac-wallet -p ac-provider -p
//! ac-gateway -p ac-auditor -p ac-mock-engine`.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    missing_docs
)]

mod common;

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use ac_e2e::{TestNode, Testnet, TestnetConfig, free_port};
use ac_primitives::market::audit::{
    AuditorStats, ProviderAuditStats, RoundIndex, VerdictOutcome, VerdictRecord,
};
use ac_runtime::AccountId;
use common::{Proc, W, account, api, bin, field, funded_wallet, import_dev_wallet, run};
use parity_scale_codec::Encode;
use serde_json::json;

const START: Duration = Duration::from_secs(180);
const MARKER: &str = "okapi-marker-5b19";
/// The registered model's seed (verify engines and the honest provider play it).
const HONEST_SEED: u64 = 7;
/// What the cheating provider switches to.
const OTHER_SEED: u64 = 8;
const AUDITORS: usize = 6;

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
        .arg(&p.toploc);
    cmd
}

async fn round(node: &TestNode) -> (RoundIndex, u32, u32) {
    api::<Option<(RoundIndex, u32, u32)>>(node, "AuditApi_round", &[])
        .await
        .unwrap()
}

async fn block(node: &TestNode) -> u32 {
    u32::try_from(node.height().await.unwrap()).unwrap()
}

async fn stats(node: &TestNode, who: &AccountId) -> ProviderAuditStats {
    api(node, "AuditApi_provider_stats", &who.encode()).await
}

async fn open_dispute(node: &TestNode, who: &AccountId) -> Option<u64> {
    api(node, "AuditApi_open_dispute", &who.encode()).await
}

fn status(user: &W, address: &str) -> String {
    let mut c = Command::new(bin("ac-wallet"));
    c.args(["market", "provider", "show", "--address", address])
        .args(["--node", &user.node]);
    run(&mut c)
}

#[tokio::test(flavor = "multi_thread")]
async fn agents_find_a_provider_that_starts_cheating() {
    require_e2e!();
    let config = TestnetConfig {
        label: "auditor-agent".to_string(),
        chain: "dev".to_string(),
        authorities: 1,
        args: Vec::new(),
    };
    let net = Testnet::start_with(config, START).await.unwrap();
    net.wait_all(&[0], 1, START).await.unwrap();
    let node = &net.nodes[0];
    let base = net.base.clone();
    let user = import_dev_wallet(&base, &node.url);

    // A bfloat16 model (TOPLOC re-checks need it).
    let manifest = base.join("model.json");
    std::fs::write(
        &manifest,
        format!(r#"{{"name":"mock","arch":"qwen2","quant":"bf16","shards":["0x{}"],"licenseTag":"apache-2.0"}}"#, "05".repeat(32)),
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

    // Two providers at the same price; the cheater's engine switches models on a file.
    let switch_file = base.join("start-cheating");
    let mut engines = Vec::new();
    let mut providers = Vec::new();
    for name in ["cheater", "honest"] {
        let toploc = base.join(format!("{name}-toploc.sock"));
        let mut c = Command::new(bin("ac-mock-engine"));
        c.args(["--listen", "127.0.0.1:0", "--toploc-mode", "prove"])
            .args(["--model-seed", &HONEST_SEED.to_string()])
            .arg("--toploc-socket")
            .arg(&toploc);
        if name == "cheater" {
            c.args(["--switch-seed", &OTHER_SEED.to_string()])
                .arg("--switch-file")
                .arg(&switch_file);
        }
        let (engine, engine_addr) = Proc::start(c, base.join(format!("engine-{name}.log")));
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
            &format!("{model}:0.1:0.2"),
        ]));
        providers.push(ProviderSetup {
            wallet,
            kem_file,
            port,
            engine_addr,
            toploc,
        });
    }
    let _running: Vec<Proc> = providers
        .iter()
        .zip(["cheater", "honest"])
        .map(|(p, n)| {
            Proc::start(
                provider_cmd(p, &node.url, &model, &base.join(format!("data-{n}"))),
                base.join(format!("provider-{n}.log")),
            )
            .0
        })
        .collect();
    let cheater = account(&providers[0].wallet.address());
    let honest = account(&providers[1].wallet.address());

    // The gateway.
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
    let (_gw, _) = Proc::start(
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
                ])
                .arg("--data-dir")
                .arg(base.join("data-gateway"));
            c
        },
        base.join("gateway.log"),
    );
    let g = gateway.address();

    // A prompt bank whose every prompt carries the marker (spec "日志中没有审计 prompt").
    let bank = base.join("bank.jsonl");
    let lines: Vec<String> = (0..20)
        .map(|i| {
            json!([{"role": "user", "content": format!("question {i} about {MARKER} and tides")}])
                .to_string()
        })
        .collect();
    std::fs::write(&bank, lines.join("\n")).unwrap();

    // Six auditors, each with a payment account (escrow at the gateway), a verify engine and an
    // agent. The pot pays them.
    let pot = field(&run(&mut user.query(&["audit", "pot"])), "pot");
    run(&mut user.cmd(&["transfer", "--to", &pot, "--amount", "100"]));
    let mut agents = Vec::new();
    let mut auditors = Vec::new();
    for i in 0..AUDITORS {
        let a = funded_wallet(&base, &format!("auditor{i}"), &user, "2000");
        run(&mut a.cmd(&["audit", "register"]));
        let payer = funded_wallet(&base, &format!("payer{i}"), &user, "50");
        run(&mut payer.cmd(&[
            "market",
            "escrow",
            "deposit",
            "--gateway",
            &g,
            "--amount",
            "20",
        ]));
        let kem = base.join(format!("auditor{i}-kem.json"));
        run(Command::new(bin("ac-auditor"))
            .args(["keygen", "--out"])
            .arg(&kem)
            .arg("--password-file")
            .arg(&a.password));
        let socket = base.join(format!("auditor{i}-verify.sock"));
        let engine_port = free_port().unwrap();
        let listen = free_port().unwrap();
        let cfg = json!({
            "node": node.url,
            "wallet": a.file, "password_file": a.password, "kem_key": kem,
            "listen": format!("127.0.0.1:{listen}"),
            "public_endpoint": format!("http://127.0.0.1:{listen}"),
            "data_dir": base.join(format!("auditor{i}-data")),
            "engines": [{"model": model, "engine": format!("http://127.0.0.1:{engine_port}"),
                         "engine_model": "mock-model", "socket": socket}],
            "payers": [{"wallet": payer.file, "password_file": payer.password, "gateways": [g]}],
            "prompts": {"bank": bank, "bank_percent": 100, "min_tokens": 8, "max_tokens": 24},
        });
        let cfg_file = base.join(format!("agent{i}-config.json"));
        std::fs::write(&cfg_file, cfg.to_string()).unwrap();
        let mut agent = Command::new(bin("ac-auditor"));
        agent
            .args(["run", "--config"])
            .arg(&cfg_file)
            .args(["--log-level", "debug"]);
        // The agent waits for its engine's plugin; the engine retries until the socket exists.
        let mut engine = Command::new(bin("ac-mock-engine"));
        engine
            .args([
                "--listen",
                &format!("127.0.0.1:{engine_port}"),
                "--toploc-mode",
                "verify",
            ])
            .args(["--model-seed", &HONEST_SEED.to_string()])
            .arg("--toploc-socket")
            .arg(&socket);
        let verify = Proc::spawn(engine, &base.join(format!("auditor{i}-engine.log")));
        let (agent, _) = Proc::start(agent, base.join(format!("auditor{i}.log")));
        agents.push((agent, verify));
        auditors.push(account(&a.address()));
    }

    // A full round of honest service: verdicts on both providers, all passing.
    let r0 = round(node).await.0;
    while round(node).await.0 < r0 + 2 {
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    for p in [&cheater, &honest] {
        let s = stats(node, p).await;
        assert!(s.pass > 0, "no passing verdict before the switch: {s:?}");
        assert_eq!(s.fail, 0, "{s:?}");
    }

    // The switch, in the middle of a round.
    let (k, start, next) = round(node).await;
    while block(node).await < start + (next - start) / 2 {
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    std::fs::write(&switch_file, b"").unwrap();
    let switched = block(node).await;
    let length = next - start;
    let limit = start + 4 * length; // the end of round k + 3

    // The agents find it: failing verdicts, a dispute, a confirmation, a jail.
    let mut first_fail = None;
    let mut disputed = None;
    let mut jailed = None;
    while jailed.is_none() {
        let now = block(node).await;
        assert!(
            now < limit,
            "not jailed within three rounds of the switch (block {switched}, now {now})"
        );
        if first_fail.is_none() && stats(node, &cheater).await.fail > 0 {
            first_fail = Some(now);
        }
        if disputed.is_none() && open_dispute(node, &cheater).await.is_some() {
            disputed = Some(now);
        }
        if field(&status(&user, &providers[0].wallet.address()), "status") == "Jailed" {
            jailed = Some(now);
            disputed.get_or_insert(now);
            first_fail.get_or_insert(now);
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    let shown = status(&user, &providers[0].wallet.address());
    assert_eq!(field(&shown, "serviceable"), "false");
    assert_eq!(stats(node, &cheater).await.confirmed, 1);

    // The honest provider: never failed, never disputed. No auditor was slashed.
    let s = stats(node, &honest).await;
    assert_eq!(s.fail, 0, "{s:?}");
    assert!(open_dispute(node, &honest).await.is_none());
    for a in &auditors {
        let shown = run(&mut user.query(&[
            "audit",
            "status",
            "--address",
            &ac_primitives::encode_address(a.as_ref()),
        ]));
        assert_eq!(field(&shown, "status"), "Active");
        assert_eq!(field(&shown, "stake"), "1000 ATC");
        let activity: AuditorStats = api(node, "AuditApi_auditor_stats", &a.encode()).await;
        assert!(activity.verdicts > 0);
    }
    // Some reviewers re-checked the evidence their accusers served and confirmed.
    let r = round(node).await.0;
    let mut fails = Vec::<VerdictRecord<AccountId>>::new();
    for back in 0..=4 {
        let v: Vec<VerdictRecord<AccountId>> = api(
            node,
            "AuditApi_verdicts",
            &(r.saturating_sub(back), cheater.clone()).encode(),
        )
        .await;
        fails.extend(
            v.into_iter()
                .filter(|v| matches!(v.outcome, VerdictOutcome::Fail(_))),
        );
    }
    assert!(fails.len() >= 2 && fails.iter().all(|v| v.evidence.is_some()));

    // Nothing of the prompts in any log.
    tokio::time::sleep(Duration::from_secs(2)).await;
    for entry in std::fs::read_dir(&base).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|e| e == "log") {
            let text = std::fs::read_to_string(&path).unwrap_or_default();
            assert!(
                !text.contains(MARKER),
                "{} contains request content",
                path.display()
            );
        }
    }

    let latency = json!({
        "round_blocks": length,
        "switch_block": switched,
        "switch_round": k,
        "first_fail_blocks": first_fail.unwrap() - switched,
        "dispute_blocks": disputed.unwrap() - switched,
        "jail_blocks": jailed.unwrap() - switched,
    });
    eprintln!("audit detection latency: {latency}");
    if let Some(path) = std::env::var_os("AC_E2E_LATENCY") {
        std::fs::write(path, latency.to_string()).unwrap();
    }
}

trait Query {
    fn query(&self, args: &[&str]) -> Command;
}

impl Query for W {
    /// A read-only wallet command against the node (no password needed).
    fn query(&self, args: &[&str]) -> Command {
        let mut cmd = Command::new(bin("ac-wallet"));
        cmd.args(args).args(["--node", &self.node]);
        cmd
    }
}
