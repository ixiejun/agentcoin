//! Work settlement end to end (m5-work-settlement task 6.4; spec `engineering/ci-quality-gates`
//! item 11 and `clients/wallet-cli` "工作结算子命令"): on a development chain a user escrows
//! credits and signs a voucher; two providers and a gateway sign receipts with `ac-wallet`; the
//! gateway builds and submits the work report; nothing is claimable during the challenge period;
//! after the maturity epoch is settled the providers receive their shares and market emission,
//! the gateway its fee, the burn is counted and the treasury grows with the work. The node
//! checks every block's issuance throughout; the chain must keep finalizing.
//! Enabled with `AC_E2E=1`; needs `cargo build -p ac-node -p ac-wallet`.

// Test code: failures should abort the test.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, missing_docs)]

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use ac_crypto::{KemAlg, KemPublicKey};
use ac_e2e::{TestNode, Testnet, TestnetConfig, enabled};
use ac_runtime::{AccountId, Balance};
use parity_scale_codec::Decode;

const START: Duration = Duration::from_secs(180);
const DEV_MNEMONIC: &str = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon art";
/// Dev preset emission epoch length.
const EPOCH: u64 = 10;

macro_rules! require_e2e {
    () => {
        if !enabled() {
            eprintln!(
                "end-to-end test disabled; run with AC_E2E=1 after building ac-node and ac-wallet"
            );
            return;
        }
    };
}

fn wallet_binary() -> PathBuf {
    std::env::var_os("AC_WALLET").map_or_else(
        || Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/debug/ac-wallet"),
        PathBuf::from,
    )
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

/// "1.5 ATC" → smallest units.
fn atc(text: &str) -> Balance {
    ac_wallet::amount::parse_atc(text.trim_end_matches(" ATC")).unwrap()
}

struct W {
    file: PathBuf,
    password: PathBuf,
    node: String,
}

impl W {
    fn cmd(&self, args: &[&str]) -> Command {
        let mut cmd = Command::new(wallet_binary());
        cmd.args(args)
            .arg("--wallet")
            .arg(&self.file)
            .arg("--password-file")
            .arg(&self.password)
            .args(["--node", &self.node])
            .env("RUST_BACKTRACE", "0");
        cmd
    }

    /// A wallet-only command (no node).
    fn offline(&self, args: &[&str]) -> Command {
        let mut cmd = Command::new(wallet_binary());
        cmd.args(args)
            .arg("--wallet")
            .arg(&self.file)
            .arg("--password-file")
            .arg(&self.password)
            .env("RUST_BACKTRACE", "0");
        cmd
    }

    fn query(&self, args: &[&str]) -> Command {
        let mut cmd = Command::new(wallet_binary());
        cmd.args(args)
            .args(["--node", &self.node])
            .env("RUST_BACKTRACE", "0");
        cmd
    }

    fn address(&self) -> String {
        run(Command::new(wallet_binary())
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
    let mut import = Command::new(wallet_binary())
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
    run(Command::new(wallet_binary())
        .arg("new")
        .arg("--wallet")
        .arg(&w.file)
        .arg("--password-file")
        .arg(&w.password)
        .stderr(Stdio::null()));
    run(&mut from.cmd(&["transfer", "--to", &w.address(), "--amount", funds]));
    w
}

async fn api<T: Decode>(node: &TestNode, method: &str) -> T {
    T::decode(&mut &node.state_call(method, &[], None).await.unwrap()[..]).unwrap()
}

async fn treasury(node: &TestNode) -> Balance {
    let (_, community) = api::<(AccountId, Balance)>(node, "TreasuryApi_community").await;
    let (_, holder) = api::<(AccountId, Balance)>(node, "TreasuryApi_holder").await;
    community + holder
}

async fn wait_for_height(node: &TestNode, height: u64) {
    while node.height().await.unwrap() < height {
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn work_settlement_flow() {
    require_e2e!();
    let config = TestnetConfig {
        label: "settlement".to_string(),
        chain: "dev".to_string(),
        authorities: 1,
        args: Vec::new(),
    };
    let net = Testnet::start_with(config, START).await.unwrap();
    net.wait_all(&[0], 1, START).await.unwrap();
    let node = &net.nodes[0];
    let user = import_dev_wallet(&net.base, &node.url);

    // A model, two providers ($0.1 input, $0.2 output per million tokens) and a gateway (3%).
    let manifest = net.base.join("model.json");
    std::fs::write(
        &manifest,
        format!(
            r#"{{"name":"Qwen2.5-0.5B-Instruct","arch":"qwen2","quant":"int4","shards":["0x{}"],"licenseTag":"apache-2.0"}}"#,
            "01".repeat(32)
        ),
    )
    .unwrap();
    let registered = run(&mut user.cmd(&[
        "market",
        "model",
        "register",
        "--file",
        manifest.to_str().unwrap(),
    ]));
    let model = field(&registered, "model id");
    let kem = format!(
        "0x{}",
        KemPublicKey::new(KemAlg::XWing, &[7; 1216])
            .unwrap()
            .to_canonical()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
    );
    let price = format!("{model}:0.1:0.2");
    let p1 = funded_wallet(&net.base, "p1", &user, "1000");
    let p2 = funded_wallet(&net.base, "p2", &user, "1000");
    let gateway = funded_wallet(&net.base, "gateway", &user, "5000");
    for p in [&p1, &p2] {
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
            &price,
        ]));
    }
    run(&mut gateway.cmd(&[
        "market",
        "gateway",
        "register",
        "--endpoint",
        "https://g.example",
        "--fee-bps",
        "300",
    ]));
    let g = gateway.address();

    // The user escrows 10 ATC and signs a voucher for $0.50.
    run(&mut user.cmd(&[
        "market",
        "escrow",
        "deposit",
        "--gateway",
        &g,
        "--amount",
        "10",
    ]));
    let voucher = field(
        &run(&mut user.cmd(&["market", "voucher", "sign", "--gateway", &g, "--usd", "0.5"])),
        "voucher",
    );

    // Receipts: p1 served 1M + 1M tokens ($0.30), p2 2M prompt tokens ($0.20); each provider
    // signs, the gateway co-signs, anyone checks.
    let mut receipts = Vec::new();
    for (p, name, tokens) in [
        (&p1, "r1.json", ("1000000", "1000000")),
        (&p2, "r2.json", ("2000000", "0")),
    ] {
        let file = net.base.join(name);
        let out = run(&mut p.cmd(&[
            "market",
            "receipt",
            "new",
            "--gateway",
            &g,
            "--provider",
            &p.address(),
            "--model",
            &model,
            "--in-tokens",
            tokens.0,
            "--out-tokens",
            tokens.1,
            "--ttft-ms",
            "120",
            "--total-ms",
            "900",
            // A non-zero TOPLOC commitment: receipts without proofs earn no market work
            // (m6-public-jobs, I-008); the chain only tells zero from non-zero.
            "--toploc",
            &format!("0x{}", "ab".repeat(32)),
            "--out",
            file.to_str().unwrap(),
        ]));
        let expected = if name == "r1.json" { "$0.3" } else { "$0.2" };
        assert_eq!(field(&out, "fee"), expected);
        run(&mut gateway.offline(&[
            "market",
            "receipt",
            "cosign",
            "--file",
            file.to_str().unwrap(),
        ]));
        run(&mut user.query(&[
            "market",
            "receipt",
            "check",
            "--file",
            file.to_str().unwrap(),
        ]));
        receipts.push(file);
    }

    // The gateway submits the report (scenario "从收据到报告"): the root it printed is stored.
    let burned_before: Balance = api(node, "EmissionApi_total_burned").await;
    let treasury_before = treasury(node).await;
    let submitted = run(&mut gateway.cmd(&[
        "market",
        "report",
        "submit",
        "--receipt",
        receipts[0].to_str().unwrap(),
        "--receipt",
        receipts[1].to_str().unwrap(),
        "--voucher",
        &voucher,
    ]));
    let shown = run(&mut user.query(&["market", "report", "show", "--id", "0"]));
    assert_eq!(field(&shown, "root"), field(&submitted, "root"));
    assert_eq!(field(&shown, "settled"), "0.5 ATC");
    assert_eq!(field(&shown, "burned"), "0.1 ATC");
    assert_eq!(field(&shown, "gateway fee"), "0.015 ATC");
    let matures: u64 = field(&shown, "matures in epoch").parse().unwrap();
    let burned: Balance = api(node, "EmissionApi_total_burned").await;
    assert!(
        burned - burned_before >= atc("0.1 ATC"),
        "the burn is counted"
    );

    // Challenge period: nothing to claim.
    let claim = |w: &W, who: &W| run(&mut w.cmd(&["market", "claim", "--account", &who.address()]));
    assert!(claim(&user, &p1).contains("nothing to claim"));

    // Block (matures + 1) × EPOCH + 1 settles the maturity epoch.
    wait_for_height(node, (matures + 1) * EPOCH + 2).await;
    let epoch = run(&mut user.query(&["market", "work", "--epoch", &matures.to_string()]));
    let market = atc(&field(&epoch, "market emission"));
    assert!(market > 0, "market emission minted for the matured work");
    assert!(
        treasury(node).await > treasury_before,
        "the treasury grows with work"
    );

    // Scenario "挑战期后领取": shares (pool 0.385 ATC, 3:2) plus emission shares (3:2).
    let pool = atc("0.385 ATC");
    for (p, share, weight) in [(&p1, pool * 3 / 5, 3u128), (&p2, pool * 2 / 5, 2)] {
        let out = claim(&user, p);
        let received = atc(&field(&out, "received"));
        assert_eq!(received, share + market * weight / 5, "{}", p.address());
    }
    let out = claim(&user, &gateway);
    assert_eq!(atc(&field(&out, "received")), atc("0.015 ATC"));
    assert!(claim(&user, &p1).contains("nothing to claim"));

    // The chain kept finalizing.
    let (finalized, _) = node.finalized().await.unwrap();
    assert!(finalized >= (matures + 1) * EPOCH, "finalized {finalized}");
}
