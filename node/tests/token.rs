//! Requirement "ATC 精度与余额" / Scenario "代币元数据" (chain/native-token).

// Test helpers unwrap freely; failures should abort the test.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, missing_docs)]

mod common;

use std::time::Duration;

use common::{start_dev_node, wait_for_height};
use jsonrpsee::{core::client::ClientT, rpc_params};

#[tokio::test(flavor = "multi_thread")]
async fn token_symbol_and_decimals() {
    let node = start_dev_node(&[]);
    wait_for_height(&node, 0, Duration::from_secs(120)).await;
    let properties: serde_json::Value = node
        .rpc
        .request("system_properties", rpc_params![])
        .await
        .unwrap();
    assert_eq!(properties["tokenSymbol"], "ATC");
    assert_eq!(properties["tokenDecimals"], 18);
}
