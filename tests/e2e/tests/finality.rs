//! M2 acceptance on the four-authority local testnet (tasks 8.3–8.5 of `m2-finality`): fault
//! tolerance, double signing with one key on two nodes, full nodes and late joiners, and
//! independently recomputed randomness. Enabled with `AC_E2E=1`.

// Test code: failures should abort the test.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    missing_docs
)]

use std::time::{Duration, Instant};

use ac_crypto::{PqPublicKey, SigAlg};
use ac_e2e::{TestNode, Testnet, enabled};
use ac_primitives::ac_bft::{Authority, SetId};
use ac_primitives::offences::OffenceKey;
use ac_runtime::genesis_config_presets::dev_public_key;
use parity_scale_codec::{Decode, Encode};
use sp_core::H256;

const START: Duration = Duration::from_secs(120);
/// `local` epoch length.
const EPOCH: u64 = 20;

macro_rules! require_e2e {
    () => {
        if !enabled() {
            eprintln!("end-to-end test disabled; run with AC_E2E=1 after `cargo build -p ac-node`");
            return;
        }
    };
}

async fn finalized(node: &TestNode) -> u64 {
    node.finalized().await.map_or(0, |f| f.0)
}

async fn authority_set(node: &TestNode) -> Result<(SetId, Vec<Authority>), String> {
    let bytes = node
        .state_call("ValidatorSetApi_authority_set", &[], None)
        .await?;
    Decode::decode(&mut &bytes[..]).map_err(|e| e.to_string())
}

/// Every offence recorded in any set so far, at the best block. While two nodes sign with one
/// key the best block can be on a fork whose state is discarded before the call reaches it;
/// that is an error the caller polls past.
async fn offences(node: &TestNode) -> Result<Vec<(PqPublicKey, OffenceKey)>, String> {
    let (current, _) = authority_set(node).await?;
    let mut all = Vec::new();
    for set_id in 0..=current {
        let bytes = node
            .state_call("OffencesApi_offences", &set_id.encode(), None)
            .await?;
        all.extend(
            Vec::<(PqPublicKey, OffenceKey)>::decode(&mut &bytes[..]).map_err(|e| e.to_string())?,
        );
    }
    Ok(all)
}

/// Waits until `node` is within 3 blocks of `reference` in both best and finalized height.
async fn wait_caught_up(node: &TestNode, reference: &TestNode, timeout: Duration, base: &str) {
    let started = Instant::now();
    loop {
        let (best, fin) = (
            reference.height().await.unwrap_or(0),
            finalized(reference).await,
        );
        if let (Some(h), Some((f, _))) = (node.height().await, node.finalized().await)
            && best.saturating_sub(h) <= 3
            && fin.saturating_sub(f) <= 3
            && f > 0
        {
            return;
        }
        assert!(
            started.elapsed() < timeout,
            "{} did not catch up within {timeout:?}; logs in {base}",
            node.name
        );
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

// Scenario "停止 1/4 节点": with dave stopped, the other nodes' finalized height grows by at
// least 15 in 30 s.
#[tokio::test(flavor = "multi_thread")]
async fn finality_survives_one_stopped_node() {
    require_e2e!();
    let mut net = Testnet::start("stop-one", START).await.unwrap();
    net.wait_finalized(&[0, 1, 2, 3], 5, START).await.unwrap();
    net.stop("dave").unwrap();
    let mut before = Vec::new();
    for node in &net.nodes[..3] {
        before.push(finalized(node).await);
    }
    tokio::time::sleep(Duration::from_secs(30)).await;
    for (node, before) in net.nodes[..3].iter().zip(before) {
        let after = finalized(node).await;
        assert!(
            after >= before + 15,
            "{} finalized {before} → {after} in 30 s; logs in {}",
            node.name,
            net.base.display()
        );
    }
}

// Scenario "停止一半节点后恢复": with charlie and dave stopped for 20 s blocks keep coming but
// finality stops; after they restart every node is within 3 blocks of its best block in
// finalized height within 30 s. The 30 s count from when both restarted nodes serve RPC again:
// a debug node takes over 20 s to open its database and load the runtime on a CI runner (I-021),
// which says nothing about AC-BFT's recovery.
#[tokio::test(flavor = "multi_thread")]
async fn finality_pauses_without_quorum_and_recovers() {
    require_e2e!();
    let mut net = Testnet::start("stop-half", START).await.unwrap();
    net.wait_finalized(&[0, 1, 2, 3], 5, START).await.unwrap();
    net.stop("charlie").unwrap();
    net.stop("dave").unwrap();
    // Let votes already in flight settle.
    tokio::time::sleep(Duration::from_secs(3)).await;
    let (best_before, fin_before) = (
        net.nodes[0].height().await.unwrap(),
        finalized(&net.nodes[0]).await,
    );
    tokio::time::sleep(Duration::from_secs(17)).await;
    let (best_after, fin_after) = (
        net.nodes[0].height().await.unwrap(),
        finalized(&net.nodes[0]).await,
    );
    assert_eq!(fin_after, fin_before, "finality advanced without a quorum");
    assert!(
        best_after >= best_before + 5,
        "blocks stopped: #{best_before} → #{best_after}"
    );

    net.restart("charlie").unwrap();
    net.restart("dave").unwrap();
    let spawned = Instant::now();
    for node in &net.nodes[2..4] {
        while node.height().await.is_none() {
            assert!(
                spawned.elapsed() < START,
                "{} did not come back; logs in {}",
                node.name,
                net.base.display()
            );
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }
    eprintln!("restarted nodes up after {:?}", spawned.elapsed());
    let restarted = Instant::now();
    loop {
        let mut lagging = Vec::new();
        for node in &net.nodes {
            let (best, fin) = (node.height().await.unwrap_or(0), finalized(node).await);
            if best.saturating_sub(fin) > 3 || fin <= fin_before {
                lagging.push((node.name.clone(), best, fin));
            }
        }
        if lagging.is_empty() {
            break;
        }
        assert!(
            restarted.elapsed() < Duration::from_secs(30),
            "finality did not recover within 30 s: {lagging:?}; logs in {}",
            net.base.display()
        );
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

// Scenarios "同一密钥运行两个节点" (an offence of alice within 60 s), "下一纪元生效" (from the
// next epoch alice is out of the set and the other three keep producing and finalizing) and
// "纪元边界后按新列表出块" (the Aura-PQ list follows the new set); then a new full node syncs
// across the set change, verifying the change block's finality proof, and catches up.
#[tokio::test(flavor = "multi_thread")]
async fn same_key_on_two_nodes_is_reported_and_disabled() {
    require_e2e!();
    let mut net = Testnet::start("same-key", START).await.unwrap();
    net.wait_finalized(&[0, 1, 2, 3], 3, START).await.unwrap();
    let alice = dev_public_key("alice", SigAlg::MlDsa65).unwrap();
    net.add_node("alice-twin", Some("alice"), &[]).unwrap();
    let bob = net.node("bob").unwrap();

    let started = Instant::now();
    let recorded_at = loop {
        if offences(bob)
            .await
            .is_ok_and(|all| all.iter().any(|(who, _)| *who == alice))
        {
            break bob.height().await.unwrap();
        }
        assert!(
            started.elapsed() < Duration::from_secs(60),
            "no offence of alice within 60 s; logs in {}",
            net.base.display()
        );
        tokio::time::sleep(Duration::from_millis(500)).await;
    };
    // The boundary after the epoch of the recording block enacts the removal.
    let boundary = (recorded_at.saturating_sub(1) / EPOCH + 1) * EPOCH + 1;
    let others = [
        net.index("bob").unwrap(),
        net.index("charlie").unwrap(),
        net.index("dave").unwrap(),
    ];
    net.wait_all(&others, boundary + 1, Duration::from_secs(3 * EPOCH))
        .await
        .unwrap();
    let bob = net.node("bob").unwrap();
    let at = bob.hash_at(boundary).await.unwrap();
    let (_, set) = <(SetId, Vec<Authority>)>::decode(
        &mut &bob
            .state_call("ValidatorSetApi_authority_set", &[], Some(&at))
            .await
            .unwrap()[..],
    )
    .unwrap();
    assert_eq!(set.len(), 3, "alice still in the set at #{boundary}");
    assert!(set.iter().all(|a| a.key != alice));
    let aura = Vec::<PqPublicKey>::decode(
        &mut &bob
            .state_call("AuraPqApi_authorities", &[], Some(&at))
            .await
            .unwrap()[..],
    )
    .unwrap();
    assert!(
        !aura.contains(&alice) && aura.len() == 3,
        "Aura-PQ list {aura:?}"
    );
    // The other three keep producing and finalizing with the new set.
    net.wait_finalized(&others, boundary + 10, Duration::from_secs(60))
        .await
        .unwrap();

    // A new full node syncs from genesis across the set change.
    let late = net.add_node("latecomer", None, &[]).unwrap();
    wait_caught_up(
        &net.nodes[late],
        net.node("bob").unwrap(),
        Duration::from_secs(60),
        &net.base.display().to_string(),
    )
    .await;
}

// Scenarios "全节点跟随" (a full node stays within 3 finalized blocks of the validators and
// signs nothing), "新节点通过证明跟上终局性" (a node started after two epochs catches up within
// 60 s) and "独立复算" (R(0) recomputed from the published reveals).
#[tokio::test(flavor = "multi_thread")]
async fn full_nodes_follow_and_randomness_recomputes() {
    require_e2e!();
    let mut net = Testnet::start("full-node", START).await.unwrap();
    let observer = net.add_node("observer", None, &["-lac-bft=debug"]).unwrap();
    net.wait_finalized(&[0, 1, 2, 3, observer], 3, START)
        .await
        .unwrap();
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(30) {
        let validator = finalized(&net.nodes[0]).await;
        let follower = finalized(&net.nodes[observer]).await;
        assert!(
            validator.saturating_sub(follower) <= 3,
            "observer finalized #{follower}, alice #{validator}"
        );
        tokio::time::sleep(Duration::from_secs(1)).await;
    }

    // Two epochs and a bit: R(0) is published at the first block of epoch 2.
    let published = 2 * EPOCH + 1;
    net.wait_all(&[0], published + 4, Duration::from_secs(90))
        .await
        .unwrap();
    let late = net.add_node("latecomer", None, &[]).unwrap();

    let alice = &net.nodes[0];
    let before = alice.hash_at(published - 1).await.unwrap();
    let after = alice.hash_at(published).await.unwrap();
    let reveals = Vec::<([u8; 32], [u8; 32])>::decode(
        &mut &alice
            .state_call("RandomnessApi_reveals", &0u64.encode(), Some(&before))
            .await
            .unwrap()[..],
    )
    .unwrap();
    assert_eq!(reveals.len(), 4, "every validator reveals");
    let published_r = Option::<H256>::decode(
        &mut &alice
            .state_call(
                "RandomnessApi_epoch_randomness",
                &0u64.encode(),
                Some(&after),
            )
            .await
            .unwrap()[..],
    )
    .unwrap();
    let recomputed = ac_primitives::randomness::epoch_randomness(0, &reveals).unwrap();
    assert!(published_r.is_some());
    assert_eq!(published_r, recomputed);

    wait_caught_up(
        &net.nodes[late],
        &net.nodes[0],
        Duration::from_secs(60),
        &net.base.display().to_string(),
    )
    .await;

    // The observer never signed a consensus message.
    let log = std::fs::read_to_string(net.nodes[observer].log_path()).unwrap();
    assert!(log.contains("AC-BFT started"), "observer log incomplete");
    assert!(
        log.contains("following"),
        "observer did not start as a follower"
    );
    assert!(
        !log.contains("signed set="),
        "the full node signed a message"
    );
}
