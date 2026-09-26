//! Helpers for tests that run the `ac-node` binary.
// Each integration-test binary uses a different subset of these helpers.
#![allow(dead_code)]

use std::{
    net::TcpListener,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

use jsonrpsee::{
    core::client::ClientT,
    http_client::{HttpClient, HttpClientBuilder},
    rpc_params,
};

/// A node process that is killed when dropped.
pub struct Node {
    child: Child,
    /// HTTP RPC client connected to the node.
    pub rpc: HttpClient,
}

impl Drop for Node {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Returns a TCP port that was free a moment ago.
pub fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// Starts `ac-node --dev --tmp` with fresh ports and extra arguments.
pub fn start_dev_node(extra: &[&str]) -> Node {
    let rpc_port = free_port();
    let child = Command::new(env!("CARGO_BIN_EXE_ac-node"))
        .args([
            "--dev",
            "--tmp",
            "--no-telemetry",
            "--no-prometheus",
            "--no-mdns",
        ])
        .args(["--rpc-port", &rpc_port.to_string()])
        .args(["--port", &free_port().to_string()])
        .args(extra)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let rpc = HttpClientBuilder::default()
        .build(format!("http://127.0.0.1:{rpc_port}"))
        .unwrap();
    Node { child, rpc }
}

/// Waits until the node's best block number is at least `height`.
pub async fn wait_for_height(node: &Node, height: u64, timeout: Duration) -> u64 {
    let start = Instant::now();
    loop {
        let header: Result<serde_json::Value, _> =
            node.rpc.request("chain_getHeader", rpc_params![]).await;
        if let Ok(header) = header {
            let number = header["number"].as_str().unwrap();
            let number = u64::from_str_radix(number.trim_start_matches("0x"), 16).unwrap();
            if number >= height {
                return number;
            }
        }
        assert!(
            start.elapsed() < timeout,
            "node did not reach height {height} within {timeout:?}"
        );
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

/// Decodes a `0x`-prefixed hex string.
pub fn unhex(s: &str) -> Vec<u8> {
    hex::decode(s.trim_start_matches("0x")).unwrap()
}
