//! Market registration end to end (m5-market-registry task 9.1; spec
//! `engineering/ci-quality-gates` item 10 and `clients/wallet-cli` "市场子命令"): on a
//! development chain, the PoA administration sets the reference rate; `ac-wallet market`
//! registers a model, a provider and a gateway, the provider's heartbeats decide whether it is
//! serviceable, and a user escrows credits, signs and checks a voucher and withdraws after the
//! delay. The node checks every block's issuance throughout; the chain must keep finalizing.
//! Enabled with `AC_E2E=1`; needs `cargo build -p ac-node -p ac-wallet`.

// Test code: failures should abort the test.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, missing_docs)]

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use ac_crypto::sig::SigningKey;
use ac_crypto::{KemAlg, KemPublicKey, SigAlg};
use ac_e2e::{TestNode, Testnet, TestnetConfig, enabled};
use ac_primitives::market::AtcPerUsd;
use ac_runtime::transaction::{TxParams, assemble, authorized_extensions, implicit_from, payload};
use ac_runtime::{ATC, RuntimeCall};
use ac_wallet::NodeClient;
use parity_scale_codec::Encode;
use sp_runtime::generic::Era;

const START: Duration = Duration::from_secs(180);
const DEV_MNEMONIC: &str = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon art";
/// Dev preset market delays (runtime `genesis_config_presets`): rate interval, heartbeat
/// interval and escrow withdrawal delay.
const DELAY: u64 = 10;

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

fn run_failing(cmd: &mut Command) -> String {
    let out = cmd.output().unwrap_or_else(|e| panic!("{cmd:?}: {e}"));
    assert!(!out.status.success(), "{cmd:?} unexpectedly succeeded");
    String::from_utf8(out.stderr).unwrap()
}

fn field(output: &str, key: &str) -> String {
    output
        .lines()
        .find_map(|l| l.strip_prefix(&format!("{key}: ")))
        .unwrap_or_else(|| panic!("no {key} in:\n{output}"))
        .trim()
        .to_string()
}

/// A wallet file with its passphrase.
struct W {
    file: PathBuf,
    password: PathBuf,
    node: String,
}

impl W {
    /// `ac-wallet <args…>` with this wallet, passphrase and node.
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

    /// `ac-wallet <args…>` with only the node (queries).
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

/// A new funded wallet (`funds` ATC from `from`).
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

/// Signs `call` with the development account `name` and submits it (the PoA administration).
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

async fn wait_blocks(node: &TestNode, blocks: u64) {
    let target = node.height().await.unwrap() + blocks;
    while node.height().await.unwrap() < target {
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn market_registration_flow() {
    require_e2e!();
    let config = TestnetConfig {
        label: "market".to_string(),
        chain: "dev".to_string(),
        authorities: 1,
        args: Vec::new(),
    };
    let net = Testnet::start_with(config, START).await.unwrap();
    net.wait_all(&[0], 1, START).await.unwrap();
    let node = &net.nodes[0];
    let client = NodeClient::new(&node.url).unwrap();
    let user = import_dev_wallet(&net.base, &node.url);

    // The reference rate: 1 ATC = 1 USD at genesis; the administration (alice, threshold 1)
    // moves it by +10% once the minimum interval passed.
    let rate = user.query(&["market", "rate"]);
    assert_eq!(field(&run(&mut { rate }), "rate"), "1 ATC per USD");
    wait_blocks(node, DELAY).await;
    let set = RuntimeCall::RefRate(pallet_ref_rate::Call::set_rate {
        rate: AtcPerUsd(ATC / 10 * 11),
    });
    let propose = RuntimeCall::PoaCouncil(pallet_collective::Call::propose {
        threshold: 1,
        length_bound: u32::try_from(set.encoded_size()).unwrap(),
        proposal: Box::new(set),
    });
    assert!(submit_as(&client, "alice", propose).await);
    assert_eq!(
        field(&run(&mut user.query(&["market", "rate"])), "rate"),
        "1.1 ATC per USD"
    );

    // A model: the ID the wallet computes is the one the chain stores.
    let manifest = net.base.join("model.json");
    std::fs::write(
        &manifest,
        format!(
            r#"{{"name":"Qwen2.5-0.5B-Instruct","arch":"qwen2","quant":"int4","shards":["0x{}","0x{}"],"licenseTag":"apache-2.0"}}"#,
            "01".repeat(32),
            "02".repeat(32)
        ),
    )
    .unwrap();
    let manifest = manifest.to_str().unwrap();
    let registered = run(&mut user.cmd(&["market", "model", "register", "--file", manifest]));
    let id = field(&registered, "model id");
    assert_eq!(
        id,
        "0x226bb1bf7ef7963e7bc8f666002192b287323e1343ab59f9f261cafa0fa8a1ab"
    );
    let shown = run(&mut user.query(&["market", "model", "show", "--id", &id]));
    assert_eq!(field(&shown, "model id"), id);
    assert_eq!(field(&shown, "quant"), "Int4");

    // A provider (T2, $100 = 110 ATC now) and a gateway ($1,000 = 1,100 ATC).
    let provider = funded_wallet(&net.base, "provider", &user, "1000");
    let gateway = funded_wallet(&net.base, "gateway", &user, "5000");
    let kem = format!(
        "0x{}",
        hex_of(
            &KemPublicKey::new(KemAlg::XWing, &[7; 1216])
                .unwrap()
                .to_canonical()
        )
    );
    let entry = format!("{id}:0.1:0.2");
    // Too little stake: refused with the chain's error name.
    let err = run_failing(&mut provider.cmd(&[
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
        &entry,
        "--stake",
        "100",
    ]));
    assert!(err.contains("Providers(BelowThreshold)"), "{err}");
    run(&mut provider.cmd(&[
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
        &entry,
    ]));
    let listed = run(&mut user.query(&["market", "providers", "--model", &id]));
    assert_eq!(field(&listed, "serviceable providers"), "1");
    assert_eq!(field(&listed, "provider"), provider.address());
    assert_eq!(field(&listed, "stake"), "110 ATC");

    // Two silent heartbeat intervals make it unserviceable; a heartbeat brings it back.
    wait_blocks(node, 2 * DELAY + 1).await;
    let listed = run(&mut user.query(&["market", "providers", "--model", &id]));
    assert_eq!(field(&listed, "serviceable providers"), "0");
    run(&mut provider.cmd(&["market", "provider", "heartbeat"]));
    let listed = run(&mut user.query(&["market", "providers", "--model", &id]));
    assert_eq!(field(&listed, "serviceable providers"), "1");

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
    let shown = run(&mut gateway.query(&["market", "gateway", "show", "--address", &g]));
    assert_eq!(field(&shown, "fee"), "300 bps");

    // Escrow, a voucher for $0.50 checked against the chain, then withdrawal after the delay.
    run(&mut user.cmd(&[
        "market",
        "escrow",
        "deposit",
        "--gateway",
        &g,
        "--amount",
        "5",
    ]));
    let signed =
        run(&mut user.cmd(&["market", "voucher", "sign", "--gateway", &g, "--usd", "0.5"]));
    let voucher = field(&signed, "voucher");
    let checked = run(&mut user.query(&["market", "voucher", "check", "--voucher", &voucher]));
    assert_eq!(field(&checked, "valid"), "yes");
    assert!(field(&checked, "increment").ends_with("(500000 micro-dollars)"));
    assert_eq!(field(&checked, "increment in ATC"), "0.55 ATC");
    assert_eq!(field(&checked, "covered by the escrow"), "true");
    // Spec "美元格式错误": refused before anything is signed.
    let err = run_failing(&mut user.cmd(&[
        "market",
        "voucher",
        "sign",
        "--gateway",
        &g,
        "--usd",
        "0.0000001",
    ]));
    assert!(err.contains("decimal places"), "{err}");

    run(&mut user.cmd(&[
        "market",
        "escrow",
        "request-withdrawal",
        "--gateway",
        &g,
        "--amount",
        "5",
    ]));
    let err = run_failing(&mut user.cmd(&["market", "escrow", "withdraw", "--gateway", &g]));
    assert!(err.contains("Credits(TooEarly)"), "{err}");
    wait_blocks(node, DELAY + 1).await;
    run(&mut user.cmd(&["market", "escrow", "withdraw", "--gateway", &g]));
    let channel = run(Command::new(wallet_binary())
        .args([
            "market",
            "channel",
            "--gateway",
            &g,
            "--node",
            &node.url,
            "--wallet",
        ])
        .arg(&user.file));
    assert_eq!(field(&channel, "escrow"), "0 ATC");
    assert_eq!(field(&channel, "channel"), "1");

    // The node checked every block's issuance on import; the chain still grows and finalizes.
    let (finalized, _) = node.finalized().await.unwrap();
    wait_blocks(node, 3).await;
    assert!(node.finalized().await.unwrap().0 > finalized);
}

fn hex_of(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
