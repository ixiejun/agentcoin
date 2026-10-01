//! Audits end to end (m6-audit-chain 7.1, 7.2; spec `market/audit`, `node/chain-spec` "审计参数"):
//! on a development chain a bfloat16 model, two providers, a gateway and seven auditors are
//! registered with `ac-wallet`, and a user escrows credits. Each provider "serves" requests
//! through an `ac-mock-engine` in prove mode; the test plays the provider's plugin socket, builds
//! the TOPLOC proofs and has provider and gateway sign the receipts with `ac-wallet`, and the
//! gateway reports the work. In an audit round the assigned auditors re-check with `ac-auditor`
//! (a verify-mode mock engine of the registered model) and submit their verdicts with
//! `ac-wallet audit verdict`; reviewers re-check the evidence and vote.
//! - The cheating provider (another model) fails both re-checks, the dispute is confirmed: it is
//!   slashed and jailed, no longer serviceable, and its unsettled work is voided.
//! - The honest provider is falsely failed by two auditors: the reviewers re-check the evidence,
//!   which passes, and reject; both accusers are slashed and made to exit.
//!
//! Enabled with `AC_E2E=1`; needs `cargo build -p ac-node -p ac-wallet -p ac-auditor -p
//! ac-mock-engine`.

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
use std::process::{Command, Stdio};
use std::time::Duration;

use ac_e2e::{TestNode, Testnet, TestnetConfig, free_port};
use ac_market_proto::engine::{
    ENGINE_PROTOCOL_VERSION, EngineMsg, EngineReader, request_id_header,
};
use ac_market_proto::toploc::{MARKET_PARAMS, ToplocProofs};
use ac_primitives::market::audit::{
    AuditMetric, DisputeRecord, FailReason, RoundIndex, VerdictOutcome, VerdictRecord,
};
use ac_primitives::market::work::EpochWork;
use ac_runtime::{AccountId, Balance};
use ac_toploc::{Bf16, Candidate, Segment, build_proofs_from_candidates};
use ac_wallet::http::{Client, join, read_body};
use ac_wallet::work::ReceiptFile;
use common::{Proc, W, account, api, bin, field, funded_wallet, import_dev_wallet, run};
use parity_scale_codec::Encode;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const START: Duration = Duration::from_secs(180);
const MARKER: &str = "pangolin-marker-81d2";
/// The registered model's seed (the verify engines play it).
const HONEST_SEED: u64 = 7;
/// The cheating provider's model.
const OTHER_SEED: u64 = 8;
const WORDS: &str = "the quick brown fox jumps over the lazy dog again and again";

/// A prove-mode receiver on `path`: the segments of each finished request.
async fn prove_receiver(path: &Path) -> tokio::sync::mpsc::UnboundedReceiver<Vec<Segment>> {
    let listener = tokio::net::UnixListener::bind(path).unwrap();
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    tokio::spawn(async move {
        let (mut s, _) = listener.accept().await.unwrap();
        let mut reader = EngineReader::new();
        let mut buf = vec![0u8; 1 << 16];
        let mut current = Vec::new();
        loop {
            let n = s.read(&mut buf).await.unwrap();
            if n == 0 {
                return;
            }
            reader.push(&buf[..n]);
            while let Some(m) = reader.next_msg().unwrap() {
                match m {
                    EngineMsg::Hello { .. } => {
                        let w = EngineMsg::Welcome {
                            version: ENGINE_PROTOCOL_VERSION,
                            topk: 128,
                        };
                        s.write_all(&w.to_frame().unwrap()).await.unwrap();
                    }
                    EngineMsg::Segment {
                        phase,
                        len,
                        candidates,
                        ..
                    } => current.push(Segment {
                        phase,
                        len,
                        candidates: candidates
                            .iter()
                            .map(|c| Candidate {
                                index: c.index,
                                value: Bf16(c.value.0),
                            })
                            .collect(),
                    }),
                    EngineMsg::Finish { .. } => tx.send(std::mem::take(&mut current)).unwrap(),
                    other => panic!("unexpected {other:?}"),
                }
            }
        }
    });
    rx
}

/// A provider's engine (prove mode, model `seed`) with the test as its plugin's receiver.
struct Prover {
    _engine: Proc,
    url: String,
    rx: tokio::sync::mpsc::UnboundedReceiver<Vec<Segment>>,
}

async fn prover(base: &Path, name: &str, seed: u64) -> Prover {
    let socket = base.join(format!("{name}-prove.sock"));
    let rx = prove_receiver(&socket).await;
    let mut c = Command::new(bin("ac-mock-engine"));
    c.args(["--listen", "127.0.0.1:0", "--toploc-mode", "prove"])
        .args(["--model-seed", &seed.to_string()])
        .arg("--toploc-socket")
        .arg(&socket);
    let (engine, addr) = Proc::start(c, base.join(format!("engine-{name}.log")));
    Prover {
        _engine: engine,
        url: format!("http://{addr}"),
        rx,
    }
}

struct Market {
    base: PathBuf,
    model: String,
    gateway: W,
}

/// One served request: the provider's engine answers, the provider and the gateway sign the
/// receipt (with the proofs' commitment). Returns the case file and the receipt file.
async fn served(
    m: &Market,
    provider: &W,
    engine: &mut Prover,
    label: &str,
    id: u8,
) -> (PathBuf, PathBuf) {
    let base = &m.base;
    let messages = json!([{"role": "user", "content": format!("{WORDS} {MARKER} {id}")}]);
    let resp = Client::new()
        .unwrap()
        .post_with(
            &join(&engine.url, "/v1/chat/completions"),
            "application/json",
            &[("x-request-id", &request_id_header(&[id; 32]))],
            json!({"model": "mock-model", "messages": messages, "max_tokens": 40}).to_string(),
        )
        .await
        .unwrap();
    let reply: Value =
        serde_json::from_slice(&read_body(resp.into_body(), 1 << 20).await.unwrap()).unwrap();
    let segments = engine.rx.recv().await.unwrap();
    let proofs = ToplocProofs::new(
        &MARKET_PARAMS,
        &build_proofs_from_candidates(&segments, &MARKET_PARAMS).unwrap(),
    );
    let commitment = proofs.commitment().unwrap();
    let (pt, ct) = (
        reply["usage"]["prompt_tokens"].as_u64().unwrap(),
        reply["usage"]["completion_tokens"].as_u64().unwrap(),
    );
    let receipt = base.join(format!("{label}-receipt.json"));
    run(&mut provider.cmd(&[
        "market",
        "receipt",
        "new",
        "--gateway",
        &m.gateway.address(),
        "--provider",
        &provider.address(),
        "--model",
        &m.model,
        "--in-tokens",
        &pt.to_string(),
        "--out-tokens",
        &ct.to_string(),
        "--toploc",
        &hex::encode(commitment),
        "--request-id",
        &hex::encode([id; 32]),
        "--out",
        receipt.to_str().unwrap(),
    ]));
    run(&mut offline(
        &m.gateway,
        &[
            "market",
            "receipt",
            "cosign",
            "--file",
            receipt.to_str().unwrap(),
        ],
    ));
    let signed = ReceiptFile::load(&receipt).unwrap().signed().unwrap();
    let case = json!({
        "model": m.model,
        "engine_model": "mock-model",
        "messages": messages,
        "output": reply["choices"][0]["message"]["content"],
        "finish_reason": reply["choices"][0]["finish_reason"],
        "usage": {"prompt_tokens": pt, "completion_tokens": ct},
        "receipt": hex::encode(signed.encode()),
        "toploc": hex::encode(proofs.encode()),
    });
    let file = base.join(format!("{label}-case.json"));
    std::fs::write(&file, case.to_string()).unwrap();
    (file, receipt)
}

fn offline(w: &W, args: &[&str]) -> Command {
    let mut cmd = Command::new(bin("ac-wallet"));
    cmd.args(args)
        .arg("--wallet")
        .arg(&w.file)
        .arg("--password-file")
        .arg(&w.password);
    cmd
}

/// `ac-auditor recheck` of a case or of evidence against a fresh verify-mode engine of the
/// registered model; returns the report line.
fn recheck(base: &Path, label: &str, input: &[&str]) -> Value {
    let port = free_port().unwrap();
    let socket = base.join(format!("{label}-verify.sock"));
    let _ = std::fs::remove_file(&socket);
    let auditor = Command::new(bin("ac-auditor"))
        .arg("recheck")
        .args(input)
        .args(["--engine", &format!("http://127.0.0.1:{port}")])
        .args(["--engine-model", "mock-model", "--quant", "bf16"])
        .arg("--socket")
        .arg(&socket)
        .stdout(Stdio::piped())
        .stderr(std::fs::File::create(base.join(format!("{label}-auditor.log"))).unwrap())
        .spawn()
        .unwrap();
    // The verify engine connects to the auditor's socket as soon as it starts.
    let mut c = Command::new(bin("ac-mock-engine"));
    c.args([
        "--listen",
        &format!("127.0.0.1:{port}"),
        "--toploc-mode",
        "verify",
    ])
    .args(["--model-seed", &HONEST_SEED.to_string()])
    .arg("--toploc-socket")
    .arg(&socket);
    let (_engine, _) = Proc::start(c, base.join(format!("{label}-verify-engine.log")));
    let out = auditor.wait_with_output().unwrap();
    assert!(
        out.status.success(),
        "ac-auditor failed; see {label}-auditor.log"
    );
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(!text.contains(MARKER));
    serde_json::from_str(text.trim()).unwrap_or_else(|e| panic!("{e}: {text}"))
}

async fn round(node: &TestNode) -> (RoundIndex, u32, u32) {
    api::<Option<(RoundIndex, u32, u32)>>(node, "AuditApi_round", &[])
        .await
        .unwrap()
}

async fn block(node: &TestNode) -> u32 {
    u32::try_from(node.height().await.unwrap()).unwrap()
}

/// Waits for the first block of a round that has a seed; returns the round.
async fn fresh_round(node: &TestNode) -> RoundIndex {
    loop {
        let (r, start, _) = round(node).await;
        let seeded: Option<sp_core::H256> = api(node, "AuditApi_seed", &r.encode()).await;
        if seeded.is_some() && block(node).await <= start + 2 {
            return r;
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
}

async fn assignment(node: &TestNode, r: RoundIndex, provider: &W) -> Vec<AccountId> {
    api(
        node,
        "AuditApi_assignment",
        &(r, account(&provider.address())).encode(),
    )
    .await
}

fn auditor_of<'a>(auditors: &'a [W], who: &AccountId) -> &'a W {
    auditors
        .iter()
        .find(|a| account(&a.address()) == *who)
        .unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn audits_confirm_and_reject() {
    require_e2e!();
    let config = TestnetConfig {
        label: "audit".to_string(),
        chain: "dev".to_string(),
        authorities: 1,
        args: Vec::new(),
    };
    let net = Testnet::start_with(config, START).await.unwrap();
    net.wait_all(&[0], 1, START).await.unwrap();
    let node = &net.nodes[0];
    let base = net.base.clone();
    let user = import_dev_wallet(&base, &node.url);

    // A bfloat16 model, a gateway, a cheating and an honest provider.
    let manifest = base.join("model.json");
    std::fs::write(
        &manifest,
        format!(r#"{{"name":"mock","arch":"qwen2","quant":"bf16","shards":["0x{}"],"licenseTag":"apache-2.0"}}"#, "03".repeat(32)),
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
    let gateway = funded_wallet(&base, "gateway", &user, "5000");
    run(&mut gateway.cmd(&[
        "market",
        "gateway",
        "register",
        "--endpoint",
        "https://g.example",
        "--fee-bps",
        "300",
    ]));
    let m = Market {
        base: base.clone(),
        model,
        gateway,
    };
    let mut providers = Vec::new();
    for name in ["cheater", "honest"] {
        let p = funded_wallet(&base, name, &user, "1000");
        let kem_file = base.join(format!("{name}-kem.json"));
        let kem = field(
            &run(Command::new(bin("ac-provider"))
                .args(["keygen", "--out"])
                .arg(&kem_file)
                .arg("--password-file")
                .arg(&p.password)),
            "kem key",
        );
        run(&mut p.cmd(&[
            "market",
            "provider",
            "register",
            "--tier",
            "t2",
            "--endpoint",
            "https://p.example",
            "--kem-key",
            &kem,
            "--model",
            &format!("{}:0.1:0.2", m.model),
        ]));
        providers.push(p);
    }
    let (cheater, honest) = (&providers[0], &providers[1]);

    // Seven auditors (stake: the current $1,000 threshold) and a funded pot.
    let auditors: Vec<W> = (0..7)
        .map(|i| {
            let a = funded_wallet(&base, &format!("auditor{i}"), &user, "2000");
            run(&mut a.cmd(&["audit", "register"]));
            a
        })
        .collect();
    let pot = field(&run(&mut user.query(&["audit", "pot"])), "pot");
    run(&mut user.cmd(&["transfer", "--to", &pot, "--amount", "100"]));

    // Two served requests per provider, re-checked ahead of the round (re-checks do not depend
    // on who is assigned).
    let mut cheat_engine = prover(&base, "cheater", OTHER_SEED).await;
    let mut honest_engine = prover(&base, "honest", HONEST_SEED).await;
    let mut cheat_cases = Vec::new();
    let mut receipts = Vec::new();
    for i in 0..2u8 {
        let (case, receipt) =
            served(&m, cheater, &mut cheat_engine, &format!("cheat{i}"), 10 + i).await;
        let report = recheck(
            &base,
            &format!("cheat{i}"),
            &["--case", case.to_str().unwrap()],
        );
        assert_eq!(report["outcome"], "fail", "{report}");
        let report_file = base.join(format!("cheat{i}-report.json"));
        std::fs::write(&report_file, report.to_string()).unwrap();
        cheat_cases.push((case, report_file));
        receipts.push(receipt);
    }
    let mut honest_cases = Vec::new();
    for i in 0..2u8 {
        let (case, _) = served(
            &m,
            honest,
            &mut honest_engine,
            &format!("honest{i}"),
            20 + i,
        )
        .await;
        let mut report = recheck(
            &base,
            &format!("honest{i}"),
            &["--case", case.to_str().unwrap()],
        );
        assert_eq!(report["outcome"], "pass", "{report}");
        // The accusers lie: they turn the pass into a failure.
        let lie = VerdictOutcome::Fail(FailReason::Threshold {
            chunk: 1,
            metric: AuditMetric::MantissaMean,
        });
        report["onchain"] = json!(hex::encode(lie.encode()));
        let report_file = base.join(format!("honest{i}-report.json"));
        std::fs::write(&report_file, report.to_string()).unwrap();
        honest_cases.push((case, report_file));
    }

    // The cheater's work is reported (its receipts, paid by the user's voucher).
    let g = m.gateway.address();
    run(&mut user.cmd(&[
        "market",
        "escrow",
        "deposit",
        "--gateway",
        &g,
        "--amount",
        "10",
    ]));
    let total: u128 = receipts
        .iter()
        .map(|r| ReceiptFile::load(r).unwrap().body().unwrap().fee.0)
        .sum();
    let usd = format!("{}.{:06}", total / 1_000_000, total % 1_000_000);
    let voucher = field(
        &run(&mut user.cmd(&["market", "voucher", "sign", "--gateway", &g, "--usd", &usd])),
        "voucher",
    );
    let mut submit = vec!["market", "report", "submit", "--voucher", &voucher];
    let paths: Vec<String> = receipts
        .iter()
        .map(|r| r.to_str().unwrap().to_string())
        .collect();
    for p in &paths {
        submit.extend(["--receipt", p]);
    }
    run(&mut m.gateway.cmd(&submit));
    let shown = run(&mut user.query(&["market", "report", "show", "--id", "0"]));
    let matures: u64 = field(&shown, "matures in epoch").parse().unwrap();
    let verified = |node| async move {
        api::<EpochWork<Balance>>(node, "WorkApi_epoch_work", &matures.encode())
            .await
            .verified
    };
    assert!(verified(node).await > 0);

    // ---- Confirmation (7.1) ----
    let r = fresh_round(node).await;
    let assigned = assignment(node, r, cheater).await;
    assert_eq!(assigned.len(), 2);
    // Spec clients/wallet-cli "提交前发现未被分配": an unassigned auditor is stopped locally.
    let outsider = auditors
        .iter()
        .find(|a| !assigned.contains(&account(&a.address())))
        .unwrap();
    let refused = outsider
        .cmd(&[
            "audit",
            "verdict",
            "--report",
            cheat_cases[0].1.to_str().unwrap(),
            "--case",
            cheat_cases[0].0.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(!refused.status.success());
    assert!(String::from_utf8_lossy(&refused.stderr).contains("not assigned"));
    for (who, (case, report)) in assigned.iter().zip(&cheat_cases) {
        let out = run(&mut auditor_of(&auditors, who).cmd(&[
            "audit",
            "verdict",
            "--report",
            report.to_str().unwrap(),
            "--case",
            case.to_str().unwrap(),
        ]));
        // Spec "从复核结果提交不通过": the commitment equals the exported evidence's.
        let evidence = base.join(format!(
            "{}.bin",
            case.file_stem().unwrap().to_str().unwrap()
        ));
        let exported = run(Command::new(bin("ac-auditor"))
            .args(["evidence", "--case", case.to_str().unwrap(), "--out"])
            .arg(&evidence));
        let c: Value = serde_json::from_str(exported.trim()).unwrap();
        assert_eq!(
            field(&out, "evidence commitment"),
            c["commitment"].as_str().unwrap()
        );
    }
    let id: Option<u64> = api(
        node,
        "AuditApi_open_dispute",
        &account(&cheater.address()).encode(),
    )
    .await;
    let id = id.expect("two failures open a dispute");
    let dispute: Option<DisputeRecord<AccountId, u32>> =
        api(node, "AuditApi_dispute", &id.encode()).await;
    let dispute = dispute.unwrap();
    let verdicts: Vec<VerdictRecord<AccountId>> = api(
        node,
        "AuditApi_verdicts",
        &(r, account(&cheater.address())).encode(),
    )
    .await;
    // Two reviewers re-check the evidence the accusers hand over and confirm.
    for (k, (reviewer, _)) in dispute.reviewers.iter().take(2).enumerate() {
        let v = &verdicts[k];
        let case = &cheat_cases[assigned.iter().position(|a| *a == v.auditor).unwrap()].0;
        let evidence = base.join(format!(
            "{}.bin",
            case.file_stem().unwrap().to_str().unwrap()
        ));
        let report = recheck(
            &base,
            &format!("review{k}"),
            &[
                "--evidence",
                evidence.to_str().unwrap(),
                "--commitment",
                &hex::encode(v.evidence.unwrap()),
            ],
        );
        assert_eq!(report["outcome"], "fail", "{report}");
        run(&mut auditor_of(&auditors, reviewer).cmd(&[
            "audit",
            "vote",
            "--provider",
            &cheater.address(),
            "--id",
            &id.to_string(),
            "--vote",
            "confirm",
        ]));
    }
    let shown = run(&mut user.query(&[
        "market",
        "provider",
        "show",
        "--address",
        &cheater.address(),
    ]));
    assert_eq!(field(&shown, "status"), "Jailed");
    assert_eq!(field(&shown, "serviceable"), "false");
    assert_eq!(field(&shown, "stake"), "90 ATC");
    assert_eq!(
        verified(node).await,
        0,
        "the jailed provider's unsettled work is voided"
    );

    // ---- Rejection (7.2) ----
    let r = fresh_round(node).await;
    let assigned = assignment(node, r, honest).await;
    for (who, (case, report)) in assigned.iter().zip(&honest_cases) {
        run(&mut auditor_of(&auditors, who).cmd(&[
            "audit",
            "verdict",
            "--report",
            report.to_str().unwrap(),
            "--case",
            case.to_str().unwrap(),
        ]));
    }
    let id: Option<u64> = api(
        node,
        "AuditApi_open_dispute",
        &account(&honest.address()).encode(),
    )
    .await;
    let id = id.unwrap();
    let dispute: DisputeRecord<AccountId, u32> =
        api::<Option<_>>(node, "AuditApi_dispute", &id.encode())
            .await
            .unwrap();
    let verdicts: Vec<VerdictRecord<AccountId>> = api(
        node,
        "AuditApi_verdicts",
        &(r, account(&honest.address())).encode(),
    )
    .await;
    for (k, (reviewer, _)) in dispute.reviewers.iter().take(2).enumerate() {
        let v = &verdicts[k];
        let case = &honest_cases[assigned.iter().position(|a| *a == v.auditor).unwrap()].0;
        let evidence = base.join(format!(
            "{}.bin",
            case.file_stem().unwrap().to_str().unwrap()
        ));
        run(Command::new(bin("ac-auditor"))
            .args(["evidence", "--case", case.to_str().unwrap(), "--out"])
            .arg(&evidence));
        let report = recheck(
            &base,
            &format!("review-honest{k}"),
            &[
                "--evidence",
                evidence.to_str().unwrap(),
                "--commitment",
                &hex::encode(v.evidence.unwrap()),
            ],
        );
        assert_eq!(report["outcome"], "pass", "{report}");
        run(&mut auditor_of(&auditors, reviewer).cmd(&[
            "audit",
            "vote",
            "--provider",
            &honest.address(),
            "--id",
            &id.to_string(),
            "--vote",
            "reject",
        ]));
    }
    for who in &assigned {
        let status = run(&mut user.query(&[
            "audit",
            "status",
            "--address",
            &ac_primitives::encode_address(who.as_ref()),
        ]));
        assert_eq!(field(&status, "status"), "Exiting");
        assert_eq!(
            field(&status, "unbonding").split(' ').next().unwrap(),
            "900"
        );
    }
    let shown =
        run(&mut user.query(&["market", "provider", "show", "--address", &honest.address()]));
    assert_eq!(field(&shown, "status"), "Active");

    // Payments came from the pot; no request content in any service's output.
    let left = field(&run(&mut user.query(&["audit", "pot"])), "balance");
    assert_ne!(left, "100 ATC");
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
