//! Requirement "区块与状态承诺使用 BLAKE3-256" (chain/state-hashing): recompute the block hash,
//! extrinsics root and state root of a running dev chain with an independent BLAKE3.

// Test helpers unwrap freely; failures should abort the test.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, missing_docs)]

mod common;

use std::time::Duration;

use ac_runtime::Header;
use hash256_std_hasher::Hash256StdHasher;
use jsonrpsee::{core::client::ClientT, rpc_params};
use parity_scale_codec::Encode;
use sp_core::H256;
use sp_trie::{LayoutV1, TrieConfiguration};

use common::{start_dev_node, unhex, wait_for_height};

/// Reference hasher on the `blake3` crate, independent of `ac-crypto` and `ac-primitives`.
#[derive(Debug)]
struct RefBlake3;

impl hash_db::Hasher for RefBlake3 {
    type Out = H256;
    type StdHasher = Hash256StdHasher;
    const LENGTH: usize = 32;
    fn hash(data: &[u8]) -> H256 {
        H256(*blake3::hash(data).as_bytes())
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn block_hashes_and_roots_are_blake3() {
    let node = start_dev_node(&[]);
    wait_for_height(&node, 3, Duration::from_secs(120)).await;

    for number in 0u32..=3 {
        let hash: String = node
            .rpc
            .request("chain_getBlockHash", rpc_params![number])
            .await
            .unwrap();
        let block: serde_json::Value = node
            .rpc
            .request("chain_getBlock", rpc_params![&hash])
            .await
            .unwrap();
        let header: Header = serde_json::from_value(block["block"]["header"].clone()).unwrap();

        // Scenario "区块哈希可独立复算".
        let recomputed = blake3::hash(&header.encode());
        assert_eq!(
            recomputed.as_bytes()[..],
            unhex(&hash)[..],
            "block {number}"
        );

        // Scenario "外部交易根可独立复算".
        let extrinsics: Vec<Vec<u8>> = block["block"]["extrinsics"]
            .as_array()
            .unwrap()
            .iter()
            .map(|x| unhex(x.as_str().unwrap()))
            .collect();
        let root = LayoutV1::<RefBlake3>::ordered_trie_root(extrinsics);
        assert_eq!(
            root, header.extrinsics_root,
            "extrinsics root of block {number}"
        );
    }
}

// Scenario "状态根可独立复算": the genesis state root equals a BLAKE3 trie over genesis storage.
#[tokio::test(flavor = "multi_thread")]
async fn genesis_state_root_is_blake3() {
    let node = start_dev_node(&[]);
    wait_for_height(&node, 0, Duration::from_secs(120)).await;

    let genesis: String = node
        .rpc
        .request("chain_getBlockHash", rpc_params![0u32])
        .await
        .unwrap();
    let header: Header = serde_json::from_value(
        node.rpc
            .request::<serde_json::Value, _>("chain_getHeader", rpc_params![&genesis])
            .await
            .unwrap(),
    )
    .unwrap();

    let keys: Vec<String> = node
        .rpc
        .request(
            "state_getKeysPaged",
            rpc_params!["0x", 1000u32, Option::<String>::None, &genesis],
        )
        .await
        .unwrap();
    assert!(!keys.is_empty() && keys.len() < 1000);
    let mut entries = Vec::new();
    for key in keys {
        let value: String = node
            .rpc
            .request("state_getStorage", rpc_params![&key, &genesis])
            .await
            .unwrap();
        entries.push((unhex(&key), unhex(&value)));
    }
    let root = LayoutV1::<RefBlake3>::trie_root(entries);
    assert_eq!(root, header.state_root);
}

// Scenario "哈希方案标识可查询".
#[tokio::test(flavor = "multi_thread")]
async fn chain_profile_reports_blake3() {
    let node = start_dev_node(&[]);
    wait_for_height(&node, 0, Duration::from_secs(120)).await;
    let result: String = node
        .rpc
        .request("state_call", rpc_params!["ChainProfileApi_profile", "0x"])
        .await
        .unwrap();
    let profile: ac_primitives::ChainProfile =
        parity_scale_codec::Decode::decode(&mut &unhex(&result)[..]).unwrap();
    assert_eq!(profile.hashing, ac_primitives::ChainHashing::Blake3_256);
}
