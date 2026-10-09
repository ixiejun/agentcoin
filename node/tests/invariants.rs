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

/// The code a runtime upgrade submits: the zstd-compressed blob, as real upgrades do. Dev-profile
/// builds embed the uncompressed blob, which no longer fits the 5 MiB block length.
fn upgrade_code(wasm: &[u8]) -> Vec<u8> {
    sp_maybe_compressed_blob::compress_strongly(
        wasm,
        sp_maybe_compressed_blob::CODE_BLOB_BOMB_LIMIT,
    )
    .unwrap()
}

/// Signs `call` as the development account alice and submits it; returns whether it succeeded.
async fn submit_as_alice(client: &ac_wallet::NodeClient, call: ac_runtime::RuntimeCall) -> bool {
    use ac_crypto::SigAlg;
    use ac_crypto::sig::SigningKey;
    use ac_runtime::transaction::{
        TxParams, assemble, authorized_extensions, implicit_from, payload,
    };
    let key =
        SigningKey::from_seed(SigAlg::MlDsa44, &ac_crypto::dev_seed("alice").unwrap()).unwrap();
    let alice = pallet_pq_accounts::derived_account(&key.public_key().unwrap());
    let context = client.chain_context().await.unwrap();
    let params = TxParams {
        nonce: client.nonce(&alice).await.unwrap(),
        tip: 0,
        era: sp_runtime::generic::Era::Immortal,
        era_birth_hash: context.genesis_hash,
    };
    let extensions = authorized_extensions(&params);
    let digest = payload(&call, &extensions, &implicit_from(&context, &params)).unwrap();
    let mut rng = ac_crypto::OsRng::new().unwrap();
    let signature = key
        .sign(&digest, pallet_pq_accounts::TX_SIGNING_CONTEXT, &mut rng)
        .unwrap();
    let first = client.current_key(&alice).await.unwrap().is_none();
    let xt = assemble(
        call,
        alice,
        signature,
        first.then(|| key.public_key().unwrap()),
        extensions,
    );
    client
        .submit_and_watch(&xt, Duration::from_secs(120))
        .await
        .unwrap()
        .success
}

// Task 8.2, Scenarios "超发的 runtime 升级" and "本节点产出违规区块": the dev administration
// (alice, threshold 1) upgrades the runtime to a build that over-mints at settlement. The
// upgrade itself is accepted and the new version runs, but the node refuses the next
// settlement block it authors: the best height stops before it and the log names the rule.
#[tokio::test(flavor = "multi_thread")]
async fn overminting_upgrade_is_rejected() {
    use ac_runtime::RuntimeCall;
    use parity_scale_codec::Encode;

    let dir = temp_dir("overmint-upgrade");
    let log = dir.join("node.log");
    let node = start_node_with(
        &["--dev"],
        &NodeOpts {
            log: Some(&log),
            ..NodeOpts::default()
        },
    );
    wait_for_height(&node, 1, START).await;
    let client = ac_wallet::NodeClient::new(&node.rpc_url).unwrap();

    let code = upgrade_code(ac_overmint_runtime::WASM_BINARY.unwrap());
    let upgrade = RuntimeCall::PoaAdmin(pallet_poa_admin::Call::dispatch_as_root {
        call: Box::new(RuntimeCall::System(frame_system::Call::set_code { code })),
    });
    let length_bound = u32::try_from(upgrade.encoded_size()).unwrap();
    let propose = RuntimeCall::PoaCouncil(pallet_collective::Call::propose {
        threshold: 1,
        proposal: Box::new(upgrade),
        length_bound,
    });
    assert!(
        submit_as_alice(&client, propose).await,
        "upgrade motion failed"
    );

    // The new runtime is live.
    let deadline = std::time::Instant::now() + START;
    loop {
        let version: serde_json::Value = node
            .rpc
            .request("state_getRuntimeVersion", rpc_params![])
            .await
            .unwrap();
        // The faulty build is one `spec_version` above the real runtime.
        if version["specVersion"] == ac_runtime::VERSION.spec_version + 1 {
            break;
        }
        assert!(std::time::Instant::now() < deadline, "upgrade not applied");
        tokio::time::sleep(Duration::from_millis(500)).await;
    }

    // The chain stops right before the next settlement block (10-block emission epochs:
    // settlements at 11, 21, 31, ...).
    let upgraded_at = common::heights(&node).await.unwrap().0;
    let mut settlement = upgraded_at / 10 * 10 + 1;
    if settlement <= upgraded_at {
        settlement += 10;
    }
    assert!(
        common::wait_for_log(&log, "rejecting block", START).await,
        "no block was rejected"
    );
    tokio::time::sleep(Duration::from_secs(5)).await;
    let best = common::heights(&node).await.unwrap().0;
    assert!(
        best < settlement,
        "best {best} reached settlement {settlement}"
    );
    drop(node);
    let text = std::fs::read_to_string(&log).unwrap();
    assert!(
        text.contains(&format!("rejecting block #{settlement}")),
        "rejection of #{settlement} not logged"
    );
    assert!(text.contains("minting bounded by the emission curve"));
}

/// Reads a storage value of the best block through RPC.
async fn storage(node: &common::Node, key: &[u8]) -> Option<Vec<u8>> {
    let value: Option<String> = node
        .rpc
        .request(
            "state_getStorage",
            rpc_params![format!("0x{}", hex::encode(key))],
        )
        .await
        .unwrap();
    value.map(|v| common::unhex(&v))
}

// m3-pos task 7.3, node/invariants Scenario "正常切换": alice registers her authority key as a
// candidate with 12% of the issuance. The dev chain (one candidate, conditions from block 20,
// held for 20 blocks) switches to PoS at an epoch boundary; the node accepts every block
// through its independent check, and the chain keeps producing and finalizing blocks.
#[tokio::test(flavor = "multi_thread")]
async fn dev_chain_switches_to_pos() {
    use ac_invariants::keys;

    let dir = temp_dir("switch-to-pos");
    let log = dir.join("node.log");
    let node = start_node_with(
        &["--dev"],
        &NodeOpts {
            log: Some(&log),
            ..NodeOpts::default()
        },
    );
    wait_for_height(&node, 1, START).await;
    let client = ac_wallet::NodeClient::new(&node.rpc_url).unwrap();
    assert!(
        register_alice(&client, 600_000 * ac_runtime::ATC).await,
        "registration failed"
    );

    let deadline = std::time::Instant::now() + START;
    loop {
        if storage(&node, &keys::PHASE).await == Some(vec![1]) {
            break;
        }
        assert!(std::time::Instant::now() < deadline, "no switch to PoS");
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    assert_eq!(storage(&node, &keys::POA_AUTHORITIES).await, Some(vec![0]));
    let switched = common::heights(&node).await.unwrap().0;
    wait_for_height(&node, switched + 15, START).await;
    wait_for_finalized(&node, switched + 10, START).await;
    drop(node);
    let text = std::fs::read_to_string(&log).unwrap();
    assert!(!text.contains("rejecting block"), "a block was rejected");
    assert!(!text.contains("constitution invariant violated"));
}

/// Signs alice's registration of her authority key as a candidate with `value`.
async fn register_alice(client: &ac_wallet::NodeClient, value: u128) -> bool {
    use ac_crypto::SigAlg;
    use ac_crypto::sig::SigningKey;
    use ac_primitives::staking::{VALIDATOR_POP_CONTEXT, pop_statement};
    let context = client.chain_context().await.unwrap();
    let alice = pallet_pq_accounts::derived_account(
        &SigningKey::from_seed(SigAlg::MlDsa44, &ac_crypto::dev_seed("alice").unwrap())
            .unwrap()
            .public_key()
            .unwrap(),
    );
    let authority =
        SigningKey::from_seed(SigAlg::MlDsa65, &ac_crypto::dev_seed("alice").unwrap()).unwrap();
    let key = authority.public_key().unwrap();
    let proof = authority
        .sign_deterministic(
            &pop_statement(&context.genesis_hash, &alice, &key),
            VALIDATOR_POP_CONTEXT,
        )
        .unwrap();
    let register =
        ac_runtime::RuntimeCall::StakingPos(pallet_staking_pos::Call::register_candidate {
            key,
            proof,
            value,
            commission_bps: 1_000,
        });
    submit_as_alice(client, register).await
}

// m3-pos task 9.2, node/invariants Scenario "提前切换被拒绝": the dev administration upgrades the
// runtime to a build whose checkpoints ignore the minimum height and the sustain period, and
// alice stakes enough to qualify. The faulty runtime switches to PoS at the next checkpoint;
// the node refuses that block, and the log names the rule.
#[tokio::test(flavor = "multi_thread")]
async fn early_switch_upgrade_is_rejected() {
    use ac_runtime::RuntimeCall;
    use parity_scale_codec::Encode;

    let dir = temp_dir("early-switch-upgrade");
    let log = dir.join("node.log");
    let node = start_node_with(
        &["--dev"],
        &NodeOpts {
            log: Some(&log),
            ..NodeOpts::default()
        },
    );
    wait_for_height(&node, 1, START).await;
    let client = ac_wallet::NodeClient::new(&node.rpc_url).unwrap();

    let code = upgrade_code(ac_early_switch_runtime::WASM_BINARY.unwrap());
    let upgrade = RuntimeCall::PoaAdmin(pallet_poa_admin::Call::dispatch_as_root {
        call: Box::new(RuntimeCall::System(frame_system::Call::set_code { code })),
    });
    let length_bound = u32::try_from(upgrade.encoded_size()).unwrap();
    let propose = RuntimeCall::PoaCouncil(pallet_collective::Call::propose {
        threshold: 1,
        proposal: Box::new(upgrade),
        length_bound,
    });
    assert!(
        submit_as_alice(&client, propose).await,
        "upgrade motion failed"
    );
    assert!(
        register_alice(&client, 600_000 * ac_runtime::ATC).await,
        "registration failed"
    );

    assert!(
        common::wait_for_log(&log, "rejecting block", START).await,
        "no block was rejected"
    );
    drop(node);
    let text = std::fs::read_to_string(&log).unwrap();
    assert!(text.contains("PoA → PoS transition"), "rule not named");
    // The node rejects it as an early switch. The faulty runtime switches at the first boundary
    // after alice qualifies, which depends on when her registration lands; an honest runtime
    // would only start the qualified run there, so any such boundary is early.
    assert!(
        text.contains("switched to PoS before the conditions held"),
        "not rejected as an early switch"
    );
    // The rejected block is an epoch boundary (10-block epochs).
    let rejected: u64 = text
        .split("rejecting block #")
        .nth(1)
        .and_then(|rest| rest.split(|c: char| !c.is_ascii_digit()).next())
        .and_then(|n| n.parse().ok())
        .unwrap();
    assert_eq!(rejected % 10, 1, "rejected block #{rejected}");
}
