//! AC-BFT finality, double-signing reports and commit–reveal randomness on real `ac-node`
//! processes (tasks 7.4 and 7.6–7.9 of `m2-finality`).

// Test helpers unwrap and index freely; failures should abort the test.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    missing_docs
)]

mod common;

use std::collections::BTreeMap;
use std::time::Duration;

use ac_primitives::ac_bft::{
    Authority, BlockRef, ENGINE_ID, SetId, VersionedFinalityProof, verify_finality_proof,
};
use ac_primitives::offences::OffenceKey;
use common::{
    NodeOpts, block_hash, free_port, heights, start_node_with, state_call, temp_dir, unhex,
    wait_for_finalized, wait_for_height, wait_for_log,
};
use jsonrpsee::{core::client::ClientT, rpc_params};
use parity_scale_codec::{Decode, Encode};
use sp_core::H256;

const START: Duration = Duration::from_secs(120);

fn h256(hex: &str) -> H256 {
    H256::from_slice(&unhex(hex))
}

/// Bytes from a JSON array of numbers or a hex string.
fn bytes(value: &serde_json::Value) -> Vec<u8> {
    match value {
        serde_json::Value::String(s) => unhex(s),
        serde_json::Value::Array(a) => a
            .iter()
            .map(|b| u8::try_from(b.as_u64().unwrap()).unwrap())
            .collect(),
        other => panic!("not bytes: {other}"),
    }
}

/// The AC-BFT justification stored with block `hash`, if any.
async fn stored_proof(node: &common::Node, hash: &str) -> Option<Vec<u8>> {
    let block: serde_json::Value = node
        .rpc
        .request("chain_getBlock", rpc_params![hash])
        .await
        .unwrap();
    block["justifications"]
        .as_array()?
        .iter()
        .find(|j| bytes(&j[0]) == ENGINE_ID)
        .map(|j| bytes(&j[1]))
}

/// Offences recorded in every set up to the current one.
async fn all_offences(node: &common::Node) -> Vec<OffenceKey> {
    let (current, _) = <(SetId, Vec<Authority>)>::decode(
        &mut &state_call(node, "ValidatorSetApi_authority_set", &[], None).await[..],
    )
    .unwrap();
    let mut all = Vec::new();
    for set_id in 0..=current {
        let bytes = state_call(node, "OffencesApi_offences", &set_id.encode(), None).await;
        let offences =
            Vec::<(ac_crypto::PqPublicKey, OffenceKey)>::decode(&mut &bytes[..]).unwrap();
        all.extend(offences.into_iter().map(|(_, key)| key));
    }
    all
}

/// Every `(set, round, kind) -> target` signed according to the `signed …` debug lines of
/// `logs`; panics if one triple was signed for two targets.
fn signed_messages(logs: &[String]) -> BTreeMap<(String, String, String), String> {
    let mut signed = BTreeMap::new();
    for line in logs.iter().flat_map(|l| l.lines()) {
        let Some(rest) = line.split("signed set=").nth(1) else {
            continue;
        };
        let fields: Vec<&str> = rest.split_whitespace().collect();
        let set = fields[0].to_string();
        let round = fields[1].trim_start_matches("round=").to_string();
        let kind = fields[2].trim_start_matches("kind=").to_string();
        let target = fields[3].trim_start_matches("target=").to_string();
        if let Some(previous) =
            signed.insert((set.clone(), round.clone(), kind.clone()), target.clone())
        {
            assert_eq!(
                previous, target,
                "signed {kind} twice in set {set} round {round}"
            );
        }
    }
    signed
}

// Scenario "单节点开发链": on a one-authority `dev` chain the finalized height keeps growing
// for 10 seconds and stays within 3 blocks of the best block.
#[tokio::test(flavor = "multi_thread")]
async fn single_node_dev_chain_finalizes() {
    let node = start_node_with(&["--dev"], &NodeOpts::default());
    wait_for_finalized(&node, 2, START).await;
    let (_, first) = heights(&node).await.unwrap();
    let mut last = first;
    for _ in 0..20 {
        tokio::time::sleep(Duration::from_millis(500)).await;
        let (best, finalized) = heights(&node).await.unwrap();
        assert!(finalized >= last, "finality went backwards");
        assert!(
            best.saturating_sub(finalized) <= 3,
            "finalized #{finalized} lags best #{best}"
        );
        last = finalized;
    }
    assert!(last >= first + 5, "finality stalled: #{first} → #{last}");
}

// Scenario "独立验证证明": a finality proof stored by the node verifies with nothing but the
// genesis hash and the set's public keys, and names the finalized block at its height.
#[tokio::test(flavor = "multi_thread")]
async fn stored_proof_verifies_independently() {
    let node = start_node_with(&["--dev"], &NodeOpts::default());
    // Without set changes a proof is stored every 64 blocks.
    let finalized = wait_for_finalized(&node, 66, Duration::from_secs(240)).await;
    let genesis = block_hash(&node, 0).await;
    let (set_id, authorities) = <(SetId, Vec<Authority>)>::decode(
        &mut &state_call(&node, "ValidatorSetApi_authority_set", &[], Some(&genesis)).await[..],
    )
    .unwrap();
    let mut found = None;
    for number in 64..=finalized {
        let hash = block_hash(&node, number).await;
        if let Some(proof) = stored_proof(&node, &hash).await {
            found = Some((number, hash, proof));
            break;
        }
    }
    let (number, hash, proof) = found.expect("no stored proof from #64 on");
    let proof = VersionedFinalityProof::decode(&mut &proof[..]).unwrap();
    let target = verify_finality_proof(&h256(&genesis), set_id, &authorities, &proof).unwrap();
    assert_eq!(
        target,
        BlockRef {
            hash: h256(&hash),
            number: u32::try_from(number).unwrap(),
        }
    );
}

// Task 7.8: the three AC-BFT metrics are exported and the finalized height is above zero.
#[tokio::test(flavor = "multi_thread")]
async fn metrics_are_exported() {
    let port = free_port();
    let node = start_node_with(
        &["--dev"],
        &NodeOpts {
            prometheus_port: Some(port),
            ..NodeOpts::default()
        },
    );
    wait_for_finalized(&node, 3, START).await;
    let body = reqwest_like::get(port, "/metrics").await;
    for name in [
        "acbft_round",
        "acbft_finalized_number",
        "acbft_finality_latency_seconds",
    ] {
        assert!(body.contains(name), "missing metric {name}");
    }
    let finalized: u64 = body
        .lines()
        .find(|l| l.starts_with("acbft_finalized_number"))
        .and_then(|l| l.rsplit(' ').next())
        .unwrap()
        .parse()
        .unwrap();
    assert!(finalized > 0);
    drop(node);
}

// Scenario "重启后仍能揭示" (and no secret in the logs): a validator commits in epoch 0,
// restarts, and its reveal in epoch 1 matches the commitment, so R(0) is published and equals
// the independent recomputation.
#[tokio::test(flavor = "multi_thread")]
async fn randomness_reveal_survives_restart() {
    let dir = temp_dir("randomness-restart");
    let base = dir.join("db");
    let logs = [dir.join("first.log"), dir.join("second.log")];
    let args = ["--dev", "-lruntime=debug,ac-bft=debug,aura-pq=debug"];
    let node = start_node_with(
        &args,
        &NodeOpts {
            base_path: Some(&base),
            log: Some(&logs[0]),
            ..NodeOpts::default()
        },
    );
    // Epoch 0 of `dev` is blocks 1–10; the commitment is in block 1.
    wait_for_height(&node, 3, START).await;
    drop(node);
    let node = start_node_with(
        &args,
        &NodeOpts {
            base_path: Some(&base),
            log: Some(&logs[1]),
            ..NodeOpts::default()
        },
    );
    // R(0) is published at the boundary of epoch 2 (block 21).
    wait_for_height(&node, 22, START).await;
    let genesis = h256(&block_hash(&node, 0).await);
    let at = block_hash(&node, 22).await;
    let latest = Option::<(u64, H256)>::decode(
        &mut &state_call(&node, "RandomnessApi_latest", &[], Some(&at)).await[..],
    )
    .unwrap();

    let seed = ac_crypto::dev_seed("alice").unwrap();
    let public = ac_crypto::sig::SigningKey::from_seed(ac_crypto::SigAlg::MlDsa65, &seed)
        .unwrap()
        .public_key()
        .unwrap();
    let account = *ac_crypto::account_id(&public).as_bytes();
    let secrets: Vec<[u8; 32]> = (0..3)
        .map(|epoch| {
            *ac_crypto::randomness_secret(&seed, &genesis.to_fixed_bytes(), epoch)
                .unwrap()
                .expose()
        })
        .collect();
    let expected = ac_primitives::randomness::epoch_randomness(0, &[(account, secrets[0])])
        .unwrap()
        .unwrap();
    assert_eq!(latest, Some((0, expected)));
    drop(node);

    // Revealed secrets are public on chain; the not-yet-revealed ones must never be logged.
    for log in &logs {
        let text = std::fs::read_to_string(log).unwrap().to_lowercase();
        assert!(text.contains("imported"), "empty log {}", log.display());
        assert!(!text.contains(&hex::encode(seed.expose())));
        assert!(!text.contains(&hex::encode(secrets[2])));
    }
}

// Scenario "双签被记录": two nodes seal different blocks with the same development key in the
// same slots; a third node receives both headers, logs the double signing and reports it, and
// the offence is recorded on chain.
#[tokio::test(flavor = "multi_thread")]
async fn double_signing_is_recorded() {
    let dir = temp_dir("double-signing");
    let observer_log = dir.join("observer.log");
    let ports = [free_port(), free_port()];
    let alice = |port| {
        start_node_with(
            &["--dev"],
            &NodeOpts {
                p2p_port: Some(port),
                ..NodeOpts::default()
            },
        )
    };
    let first = alice(ports[0]);
    let second = alice(ports[1]);
    let mut bootnodes = Vec::new();
    for (node, port) in [(&first, ports[0]), (&second, ports[1])] {
        wait_for_height(node, 1, START).await;
        let peer: String = node
            .rpc
            .request("system_localPeerId", rpc_params![])
            .await
            .unwrap();
        bootnodes.push(format!("/ip4/127.0.0.1/tcp/{port}/p2p/{peer}"));
    }
    // A full node of the same chain without a key.
    let mut args = vec!["--chain", "dev", "-laura-pq=debug,ac-offences=debug"];
    for b in &bootnodes {
        args.extend(["--bootnodes", b.as_str()]);
    }
    let observer = start_node_with(
        &args,
        &NodeOpts {
            log: Some(&observer_log),
            ..NodeOpts::default()
        },
    );
    assert!(
        wait_for_log(
            &observer_log,
            "equivocation: authority",
            Duration::from_secs(90)
        )
        .await,
        "the observer saw no double signing"
    );
    let deadline = std::time::Instant::now() + Duration::from_secs(90);
    loop {
        let offences = all_offences(&observer).await;
        if offences
            .iter()
            .any(|o| matches!(o, OffenceKey::Aura { .. }))
        {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "no block-seal offence recorded; offences: {offences:?}"
        );
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    drop((first, second, observer));
}

// Scenario "重启后不双签": a validator killed with SIGKILL right after voting and restarted on
// the same database never signs two targets for one round and message type, finality resumes,
// and no offence of it is recorded.
#[tokio::test(flavor = "multi_thread")]
async fn no_double_vote_after_kill_and_restart() {
    let dir = temp_dir("kill-restart");
    let base = dir.join("db");
    let logs = [dir.join("first.log"), dir.join("second.log")];
    let args = ["--dev", "-lac-bft=debug"];
    let opts = |log| NodeOpts {
        base_path: Some(&base),
        log: Some(log),
        ..NodeOpts::default()
    };
    let node = start_node_with(&args, &opts(&logs[0]));
    wait_for_finalized(&node, 5, START).await;
    assert!(wait_for_log(&logs[0], "kind=Prepare", Duration::from_secs(10)).await);
    // `Node::drop` sends SIGKILL.
    drop(node);
    let before = std::fs::read_to_string(&logs[0]).unwrap();

    let node = start_node_with(&args, &opts(&logs[1]));
    let (_, resumed) = loop {
        if let Some(h) = heights(&node).await {
            break h;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    };
    wait_for_finalized(&node, resumed + 5, START).await;
    let after = std::fs::read_to_string(&logs[1]).unwrap();
    let signed = signed_messages(&[before.clone(), after.clone()]);
    assert!(
        signed.len() > 10,
        "too few signed messages: {}",
        signed.len()
    );
    assert!(
        after.contains("signed set="),
        "nothing signed after the restart"
    );
    assert!(all_offences(&node).await.is_empty());
}

/// A minimal HTTP GET for the metrics endpoint.
mod reqwest_like {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    pub async fn get(port: u16, path: &str) -> String {
        let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .unwrap();
        stream
            .write_all(
                format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n")
                    .as_bytes(),
            )
            .await
            .unwrap();
        let mut body = String::new();
        stream.read_to_string(&mut body).await.unwrap();
        body
    }
}
