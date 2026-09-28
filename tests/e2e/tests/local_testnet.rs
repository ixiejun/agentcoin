//! Acceptance on the four-authority local testnet: block production and finality (M2 task 8.2;
//! spec node/chain-spec "四节点本地网络", consensus/ac-bft "四节点最终确定"), and the M1
//! transfer and key rotation (chain/pq-accounts "密钥轮换"). Enabled with `AC_E2E=1`.

// Test code: failures should abort the test.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, missing_docs)]

use std::time::{Duration, Instant};

use ac_crypto::SigAlg;
use ac_e2e::{Testnet, enabled};
use ac_runtime::transaction::{TxParams, assemble, authorized_extensions, implicit_from, payload};
use ac_runtime::{ATC, RuntimeCall};
use ac_wallet::{NodeClient, Wallet, ops};
use sp_runtime::generic::Era;

const START: Duration = Duration::from_secs(120);
const DEV_MNEMONIC: &str = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon art";

macro_rules! require_e2e {
    () => {
        if !enabled() {
            eprintln!("end-to-end test disabled; run with AC_E2E=1 after `cargo build -p ac-node`");
            return;
        }
    };
}

// Scenario "四节点持续出块": within 60 s every node is at height ≥ 40 and all agree on the chain
// except for the latest 3 blocks. Scenario "四节点最终确定": meanwhile every node's finalized
// height keeps growing and all finalize the same block at each height.
#[tokio::test(flavor = "multi_thread")]
async fn four_nodes_produce_and_finalize() {
    require_e2e!();
    let net = Testnet::start("blocks", START).await.unwrap();
    let all = [0, 1, 2, 3];
    net.wait_all(&all, 1, START).await.unwrap();
    let started = Instant::now();
    let mut finalized = vec![0; 4];
    let mut increases = vec![0; 4];
    while started.elapsed() < Duration::from_secs(60) {
        for (i, node) in net.nodes.iter().enumerate() {
            let f = node.finalized().await.map_or(0, |f| f.0);
            assert!(f >= finalized[i], "{} finality went backwards", node.name);
            if f > finalized[i] {
                increases[i] += 1;
            }
            finalized[i] = f;
        }
        let mut heights = Vec::new();
        for node in &net.nodes {
            heights.push(node.height().await.unwrap_or(0));
        }
        if heights.iter().all(|h| *h >= 40) {
            break;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    net.wait_all(
        &all,
        40,
        Duration::from_secs(60).saturating_sub(started.elapsed()),
    )
    .await
    .unwrap();
    assert!(
        increases.iter().all(|n| *n >= 10),
        "finality did not keep growing: {increases:?} increases, finalized {finalized:?}"
    );

    let mut lowest = u64::MAX;
    for node in &net.nodes {
        lowest = lowest.min(node.height().await.unwrap());
    }
    let settled = lowest.saturating_sub(3);
    for n in [1, settled / 2, settled] {
        let hashes: Vec<_> = futures_join(net.nodes.iter().map(|node| node.hash_at(n))).await;
        assert!(
            hashes.windows(2).all(|w| w[0] == w[1]),
            "fork at height {n}: {hashes:?}"
        );
    }
    // Finalized blocks agree: every node's finalized block is on every other node's chain.
    let lowest_finalized = *finalized.iter().min().unwrap();
    assert!(lowest_finalized >= 30, "finalized heights {finalized:?}");
    let hashes: Vec<_> =
        futures_join(net.nodes.iter().map(|node| node.hash_at(lowest_finalized))).await;
    assert!(
        hashes.windows(2).all(|w| w[0] == w[1]),
        "different blocks finalized at {lowest_finalized}: {hashes:?}"
    );
}

async fn futures_join<F: std::future::Future<Output = Option<String>>>(
    futures: impl Iterator<Item = F>,
) -> Vec<Option<String>> {
    let mut out = Vec::new();
    for f in futures {
        out.push(f.await);
    }
    out
}

// M1 acceptance: an ML-DSA-44 account sends its first transfer, rotates to ML-DSA-65, the old
// key is then rejected by another node and the new key works through a third node; the account
// ID never changes.
#[tokio::test(flavor = "multi_thread")]
async fn transfer_and_rotation_across_nodes() {
    require_e2e!();
    let net = Testnet::start("rotation", START).await.unwrap();
    net.wait_all(&[0, 1, 2, 3], 2, START).await.unwrap();
    let alice = NodeClient::new(&net.nodes[0].url).unwrap();
    let bob = NodeClient::new(&net.nodes[1].url).unwrap();
    let charlie = NodeClient::new(&net.nodes[2].url).unwrap();

    // Other nodes must have imported what alice's node included before they are read from.
    let synced = || async {
        let height = net.nodes[0].height().await.unwrap();
        net.wait_all(&[1, 2], height, START).await.unwrap();
    };

    let dev = Wallet::import(DEV_MNEMONIC, SigAlg::MlDsa44, b"pw").unwrap();
    let mut fresh = Wallet::create(SigAlg::MlDsa44, b"pw").unwrap().wallet;
    let me = fresh.account().unwrap();
    let address = fresh.address().to_string();
    assert!(
        ops::transfer(&alice, &dev, b"pw", &me, 10 * ATC)
            .await
            .unwrap()
            .success
    );
    synced().await;

    // First transfer from the fresh ML-DSA-44 account.
    let sink = dev.account().unwrap();
    assert!(
        ops::transfer(&bob, &fresh, b"pw", &sink, ATC)
            .await
            .unwrap()
            .success
    );
    let old_key = fresh.current_key(b"pw").unwrap();

    // Rotate to ML-DSA-65.
    ops::rotate(&alice, &mut fresh, b"pw", SigAlg::MlDsa65)
        .await
        .unwrap();
    assert_eq!(fresh.address(), address);
    assert_eq!(fresh.account().unwrap(), me);
    synced().await;
    let (key, rotations) = charlie.current_key(&me).await.unwrap().unwrap();
    assert_eq!((key.alg(), rotations), (SigAlg::MlDsa65, 1));

    // A transfer signed with the old key is rejected by bob.
    let context = bob.chain_context().await.unwrap();
    let params = TxParams {
        nonce: bob.nonce(&me).await.unwrap(),
        tip: 0,
        era: Era::Immortal,
        era_birth_hash: context.genesis_hash,
    };
    let call = RuntimeCall::Balances(pallet_balances::Call::transfer_allow_death {
        dest: sink.clone(),
        value: ATC,
    });
    let extensions = authorized_extensions(&params);
    let digest = payload(&call, &extensions, &implicit_from(&context, &params)).unwrap();
    let mut rng = ac_crypto::OsRng::new().unwrap();
    let signature = old_key
        .sign(&digest, pallet_pq_accounts::TX_SIGNING_CONTEXT, &mut rng)
        .unwrap();
    let stale = assemble(call, me.clone(), signature, None, extensions);
    assert!(
        bob.submit_and_watch(&stale, Duration::from_secs(10))
            .await
            .is_err()
    );

    // The new key works through charlie.
    let before = charlie.free_balance(&sink).await.unwrap();
    assert!(
        ops::transfer(&charlie, &fresh, b"pw", &sink, ATC)
            .await
            .unwrap()
            .success
    );
    assert_eq!(charlie.free_balance(&sink).await.unwrap(), before + ATC);
}

// Scenario "节点重启后追上": dave stops for 20 s, restarts, and within 30 s is within 3 blocks
// of the others in both best and finalized height.
#[tokio::test(flavor = "multi_thread")]
async fn restarted_node_catches_up() {
    require_e2e!();
    let mut net = Testnet::start("restart", START).await.unwrap();
    net.wait_finalized(&[0, 1, 2, 3], 5, START).await.unwrap();
    net.stop("dave").unwrap();
    tokio::time::sleep(Duration::from_secs(20)).await;
    net.restart("dave").unwrap();
    let restarted = Instant::now();
    loop {
        let leader = net.nodes[0].height().await.unwrap_or(0);
        let leader_finalized = net.nodes[0].finalized().await.map_or(0, |f| f.0);
        let dave = net.node("dave").unwrap();
        if let (Some(h), Some((f, _))) = (dave.height().await, dave.finalized().await)
            && leader.saturating_sub(h) <= 3
            && leader_finalized.saturating_sub(f) <= 3
        {
            break;
        }
        assert!(
            restarted.elapsed() < Duration::from_secs(30),
            "dave did not catch up within 30 s; logs in {}",
            net.base.display()
        );
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}
