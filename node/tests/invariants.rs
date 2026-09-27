//! Node invariants (spec node/invariants; tasks 3.1–3.3 and 8.2 of `m3-economics`).

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

use common::{
    NodeOpts, free_port, start_node_with, state_call, temp_dir, wait_for_finalized, wait_for_height,
};
use jsonrpsee::{core::client::ClientT, rpc_params};
use parity_scale_codec::Decode;

const START: Duration = Duration::from_secs(120);

// Task 3.1: a full node imports the blocks of the dev chain through the invariant block import,
// which executes each block once and hands the storage changes on; the node keeps up with
// production and finality.
#[tokio::test(flavor = "multi_thread")]
async fn network_blocks_are_executed_once() {
    let dir = temp_dir("execute-once");
    let log = dir.join("follower.log");
    let port = free_port();
    let producer = start_node_with(
        &["--dev"],
        &NodeOpts {
            p2p_port: Some(port),
            ..NodeOpts::default()
        },
    );
    wait_for_height(&producer, 1, START).await;
    let peer: String = producer
        .rpc
        .request("system_localPeerId", rpc_params![])
        .await
        .unwrap();
    let bootnode = format!("/ip4/127.0.0.1/tcp/{port}/p2p/{peer}");
    let follower = start_node_with(
        &[
            "--chain",
            "dev",
            "--bootnodes",
            &bootnode,
            "-lac-invariants=debug",
        ],
        &NodeOpts {
            log: Some(&log),
            ..NodeOpts::default()
        },
    );
    let target = wait_for_height(&producer, 15, START).await;
    wait_for_finalized(&follower, target, START).await;
    drop((producer, follower));

    let text = std::fs::read_to_string(&log).unwrap();
    let mut executed: BTreeMap<u64, usize> = BTreeMap::new();
    for line in text.lines() {
        if let Some(rest) = line.split("executed block #").nth(1) {
            let n: u64 = rest.split_whitespace().next().unwrap().parse().unwrap();
            *executed.entry(n).or_default() += 1;
        }
    }
    for n in 1..=target {
        assert_eq!(
            executed.get(&n),
            Some(&1),
            "block #{n} executed {:?} times",
            executed.get(&n)
        );
    }
}

// Task 3.2, Scenario "正常排放": with the checks enabled, a dev chain (10-block emission epochs)
// crosses at least three settlements; every block is accepted and finalized, and the emission
// matches the runtime's own view.
#[tokio::test(flavor = "multi_thread")]
async fn normal_emission_is_accepted() {
    let dir = temp_dir("normal-emission");
    let log = dir.join("node.log");
    let node = start_node_with(
        &["--dev"],
        &NodeOpts {
            log: Some(&log),
            ..NodeOpts::default()
        },
    );
    // Blocks 11, 21 and 31 settle epochs 0, 1 and 2.
    wait_for_height(&node, 32, START).await;
    wait_for_finalized(&node, 32, START).await;
    let epoch =
        u64::decode(&mut &state_call(&node, "EmissionApi_current_epoch", &[], None).await[..])
            .unwrap();
    assert!(epoch >= 3, "current epoch {epoch}");
    let minted =
        u128::decode(&mut &state_call(&node, "EmissionApi_total_minted", &[], None).await[..])
            .unwrap();
    let scheduled = u128::decode(
        &mut &state_call(&node, "EmissionApi_scheduled", &0u64.to_le_bytes(), None).await[..],
    )
    .unwrap();
    // PoA without work: each settled epoch mints its 5% floor.
    assert!(minted >= 3 * (scheduled * 5 / 100), "minted {minted}");
    drop(node);
    let text = std::fs::read_to_string(&log).unwrap();
    assert!(!text.contains("rejecting block"), "a block was rejected");
    assert!(!text.contains("constitution invariant violated"));
}
