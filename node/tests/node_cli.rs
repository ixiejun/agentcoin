//! Node start-up behaviour (node/chain-spec, consensus/aura-pq): tasks 6.2 and 7.1–7.3.

// Test helpers unwrap freely; failures should abort the test.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, missing_docs)]

mod common;

use std::time::{Duration, Instant};

use common::{
    export_spec, run_to_exit, start_dev_node, start_node, temp_dir, wait_for_height, write_spec,
};

const START: Duration = Duration::from_secs(120);

/// The genesis patch of an exported chain spec.
fn patch(spec: &mut serde_json::Value) -> &mut serde_json::Value {
    &mut spec["genesis"]["runtimeGenesis"]["patch"]
}

/// A live chain spec with no balances, whose only authority is `authority` (0x-hex).
fn live_spec(authority: &str) -> serde_json::Value {
    let mut spec = export_spec("dev");
    spec["chainType"] = "Live".into();
    spec["id"] = "agentcoin_live_test".into();
    patch(&mut spec)["balances"]["balances"] = serde_json::json!([]);
    patch(&mut spec)["auraPq"]["authorities"] = serde_json::json!([authority]);
    spec
}

/// Generates an encrypted authority key with `ac-node pq-key generate`; returns
/// (key file, password file, public key).
fn generate_key(dir: &std::path::Path, password: &str) -> (String, String, String) {
    let key = dir.join("authority.json");
    let pw = dir.join("password");
    std::fs::write(&pw, format!("{password}\n")).unwrap();
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_ac-node"))
        .args(["pq-key", "generate", "--output", key.to_str().unwrap()])
        .args(["--password-file", pw.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let public = String::from_utf8(output.stdout).unwrap().trim().to_string();
    assert!(
        public.starts_with("0x02"),
        "ML-DSA-65 public key expected, got {public}"
    );
    (
        key.to_str().unwrap().into(),
        pw.to_str().unwrap().into(),
        public,
    )
}

// consensus/aura-pq + node/chain-spec: Scenario "单节点开发链出块" — at least 5 blocks within
// 10 s once the node is producing.
#[tokio::test(flavor = "multi_thread")]
async fn dev_chain_produces_five_blocks_in_ten_seconds() {
    let node = start_dev_node(&[]);
    let first = wait_for_height(&node, 1, START).await;
    let started = Instant::now();
    let reached = wait_for_height(&node, first + 5, Duration::from_secs(10)).await;
    assert!(reached >= first + 5 && started.elapsed() <= Duration::from_secs(10));
}

// Task 7.1 / design D6: a chain without authorities is refused.
#[test]
fn chain_without_authorities_is_refused() {
    let mut spec = export_spec("dev");
    patch(&mut spec)["auraPq"]["authorities"] = serde_json::json!([]);
    let path = write_spec(&spec, "no-authorities");
    let (ok, output) = run_to_exit(
        &["--chain", path.to_str().unwrap(), "--dev-key", "alice"],
        START,
    );
    assert!(!ok);
    assert!(output.contains("no Aura-PQ authorities"), "{output}");
}

// Requirement "正式链创世零发行" / Scenario "带预挖的正式链规格".
#[test]
fn live_chain_with_premine_is_refused() {
    let mut spec = export_spec("dev");
    spec["chainType"] = "Live".into();
    let path = write_spec(&spec, "live-premine");
    let (ok, output) = run_to_exit(&["--chain", path.to_str().unwrap()], START);
    assert!(!ok);
    assert!(output.contains("genesis issuance"), "{output}");
}

// Requirement "验证人密钥加载" / Scenario "正式链上使用开发密钥".
#[test]
fn dev_key_on_live_chain_is_refused() {
    let dir = temp_dir("live-dev-key");
    let (_, _, public) = generate_key(&dir, "pw");
    let path = write_spec(&live_spec(&public), "live-dev-key");
    let (ok, output) = run_to_exit(
        &[
            "--chain",
            path.to_str().unwrap(),
            "--dev-key",
            "alice",
            "--validator",
            "--unsafe-force-node-key-generation",
        ],
        START,
    );
    assert!(!ok);
    assert!(
        output.contains("only allowed on development and local chains"),
        "{output}"
    );
}

// Requirement "验证人密钥加载" / Scenario "口令错误".
#[test]
fn wrong_password_is_refused() {
    let dir = temp_dir("wrong-password");
    let (key, _, public) = generate_key(&dir, "right");
    let wrong = dir.join("wrong");
    std::fs::write(&wrong, "wrong\n").unwrap();
    let path = write_spec(&live_spec(&public), "wrong-password");
    let (ok, output) = run_to_exit(
        &[
            "--chain",
            path.to_str().unwrap(),
            "--validator",
            "--unsafe-force-node-key-generation",
            "--pq-key-file",
            &key,
            "--pq-password-file",
            wrong.to_str().unwrap(),
        ],
        START,
    );
    assert!(!ok);
    assert!(output.contains("decryption failed"), "{output}");
}

// Scenarios "零发行的正式链规格" and "日志中无私钥": a live, zero-issuance chain passes the
// genesis check and produces blocks with an encrypted key; the log never contains the seed.
#[tokio::test(flavor = "multi_thread")]
async fn live_chain_with_key_file_produces_blocks_without_leaking_the_seed() {
    let dir = temp_dir("live-key-file");
    let (key, pw, public) = generate_key(&dir, "correct horse");
    let path = write_spec(&live_spec(&public), "live-key-file");
    let log = dir.join("node.log");
    let node = start_node(
        &[
            "--chain",
            path.to_str().unwrap(),
            "--validator",
            "--unsafe-force-node-key-generation",
            "-lruntime=debug,aura-pq=trace",
            "--pq-key-file",
            &key,
            "--pq-password-file",
            &pw,
        ],
        Some(&log),
    );
    wait_for_height(&node, 2, START).await;
    // The RPC can report the block before the import is logged: wait for the log line before
    // stopping the node, so the log checks below see a complete log.
    let deadline = Instant::now() + Duration::from_secs(20);
    while !std::fs::read_to_string(&log)
        .unwrap_or_default()
        .contains("Imported #2")
        && Instant::now() < deadline
    {
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    drop(node);

    let json = std::fs::read_to_string(&key).unwrap();
    let seed = ac_crypto::keystore::EncryptedSecret::from_json(&json)
        .unwrap()
        .decrypt(b"correct horse")
        .unwrap();
    let text = std::fs::read_to_string(&log).unwrap();
    assert!(text.contains("Imported #2"), "node did not author blocks");
    let lower = text.to_lowercase();
    assert!(!lower.contains(&hex::encode(*seed)));
    use base64_like::encode as b64;
    assert!(!text.contains(&b64(&*seed)));
}

/// Minimal standard base64 (for the log check only).
mod base64_like {
    pub fn encode(data: &[u8]) -> String {
        const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = String::new();
        for chunk in data.chunks(3) {
            let b = [
                chunk[0],
                *chunk.get(1).unwrap_or(&0),
                *chunk.get(2).unwrap_or(&0),
            ];
            let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
            for i in 0..4 {
                if i <= chunk.len() {
                    out.push(char::from(
                        T[usize::try_from((n >> (18 - 6 * i)) & 63).unwrap()],
                    ));
                } else {
                    out.push('=');
                }
            }
        }
        out
    }
}

// node/invariants Scenario "链规格缺少纪元长度": a chain spec without an emission epoch length
// is refused at start-up, live or not.
#[test]
fn chain_spec_without_epoch_length_is_refused() {
    let mut spec = export_spec("dev");
    patch(&mut spec)["emission"]["epochLength"] = serde_json::json!(0);
    let path = write_spec(&spec, "no-epoch-length");
    let (ok, output) = run_to_exit(
        &["--chain", path.to_str().unwrap(), "--dev-key", "alice"],
        START,
    );
    assert!(!ok);
    assert!(output.contains("emission epoch length"), "{output}");
}

// node/chain-spec: a live chain spec without PoA admin members is refused.
#[test]
fn live_chain_without_admin_members_is_refused() {
    let mut spec = export_spec("dev");
    spec["chainType"] = "Live".into();
    patch(&mut spec)["balances"]["balances"] = serde_json::json!([]);
    patch(&mut spec)["poaCouncil"]["members"] = serde_json::json!([]);
    patch(&mut spec)["poaAdmin"]["threshold"] = serde_json::json!(0);
    let path = write_spec(&spec, "live-no-admins");
    let (ok, output) = run_to_exit(&["--chain", path.to_str().unwrap()], START);
    assert!(!ok);
    assert!(output.contains("PoA admin member"), "{output}");
}
