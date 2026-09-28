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
    /// URL of the node's RPC endpoint.
    pub rpc_url: String,
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
    let mut args = vec!["--dev"];
    args.extend_from_slice(extra);
    start_node(&args, None)
}

/// Starts `ac-node --tmp` with fresh ports, `args` and optionally a log file for stderr.
pub fn start_node(args: &[&str], log: Option<&std::path::Path>) -> Node {
    start_node_with(
        args,
        &NodeOpts {
            log,
            ..NodeOpts::default()
        },
    )
}

/// Options of [`start_node_with`].
#[derive(Default)]
pub struct NodeOpts<'a> {
    /// File receiving stderr.
    pub log: Option<&'a std::path::Path>,
    /// Persistent database directory instead of `--tmp` (for restart tests).
    pub base_path: Option<&'a std::path::Path>,
    /// Prometheus port instead of `--no-prometheus`.
    pub prometheus_port: Option<u16>,
    /// P2P port instead of a fresh one.
    pub p2p_port: Option<u16>,
}

/// Starts `ac-node` with fresh ports, `args` and `opts`.
pub fn start_node_with(args: &[&str], opts: &NodeOpts<'_>) -> Node {
    let rpc_port = free_port();
    let stderr = match opts.log {
        Some(path) => Stdio::from(std::fs::File::create(path).unwrap()),
        None => Stdio::null(),
    };
    let mut command = Command::new(env!("CARGO_BIN_EXE_ac-node"));
    match opts.base_path {
        Some(path) => command.arg("--base-path").arg(path),
        None => command.arg("--tmp"),
    };
    match opts.prometheus_port {
        Some(port) => command.args(["--prometheus-port", &port.to_string()]),
        None => command.arg("--no-prometheus"),
    };
    let p2p_port = opts.p2p_port.unwrap_or_else(free_port);
    let child = command
        .args(["--no-telemetry", "--no-mdns"])
        .args(["--rpc-port", &rpc_port.to_string()])
        .args(["--listen-addr", &format!("/ip4/127.0.0.1/tcp/{p2p_port}")])
        .args(args)
        .stdout(Stdio::null())
        .stderr(stderr)
        .spawn()
        .unwrap();
    let rpc_url = format!("http://127.0.0.1:{rpc_port}");
    // The genesis runtime code alone can exceed the 10 MiB default (benchmark builds).
    let rpc = HttpClientBuilder::default()
        .max_response_size(64 * 1024 * 1024)
        .build(&rpc_url)
        .unwrap();
    Node {
        child,
        rpc,
        rpc_url,
    }
}

/// Runs `ac-node` with `args` until it exits (or `timeout` passes) and returns whether it
/// succeeded plus its combined output.
pub fn run_to_exit(args: &[&str], timeout: Duration) -> (bool, String) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_ac-node"))
        .args(["--tmp", "--no-telemetry", "--no-prometheus", "--no-mdns"])
        .args(["--rpc-port", &free_port().to_string()])
        .args([
            "--listen-addr",
            &format!("/ip4/127.0.0.1/tcp/{}", free_port()),
        ])
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let start = Instant::now();
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            let output = child.wait_with_output().unwrap();
            let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
            text.push_str(&String::from_utf8_lossy(&output.stderr));
            return (status.success(), text);
        }
        if start.elapsed() > timeout {
            let _ = child.kill();
            panic!("ac-node did not exit within {timeout:?}");
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// The JSON chain specification of a built-in chain (`dev` or `local`).
pub fn export_spec(chain: &str) -> serde_json::Value {
    let output = Command::new(env!("CARGO_BIN_EXE_ac-node"))
        .args(["export-chain-spec", "--chain", chain])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

/// A per-test temporary directory.
pub fn temp_dir(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("ac-node-test-{}-{name}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Writes a chain spec to a fresh file.
pub fn write_spec(spec: &serde_json::Value, name: &str) -> std::path::PathBuf {
    let path = temp_dir(name).join("spec.json");
    std::fs::write(&path, serde_json::to_vec_pretty(spec).unwrap()).unwrap();
    path
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

/// Parses a `0x`-prefixed hex block number.
fn parse_number(value: &serde_json::Value) -> u64 {
    u64::from_str_radix(value.as_str().unwrap().trim_start_matches("0x"), 16).unwrap()
}

/// Best and finalized block numbers, or `None` while the RPC is not up.
pub async fn heights(node: &Node) -> Option<(u64, u64)> {
    let best: serde_json::Value = node
        .rpc
        .request("chain_getHeader", rpc_params![])
        .await
        .ok()?;
    let finalized_hash: String = node
        .rpc
        .request("chain_getFinalizedHead", rpc_params![])
        .await
        .ok()?;
    let finalized: serde_json::Value = node
        .rpc
        .request("chain_getHeader", rpc_params![finalized_hash])
        .await
        .ok()?;
    Some((
        parse_number(&best["number"]),
        parse_number(&finalized["number"]),
    ))
}

/// Waits until the node's finalized block number is at least `height`.
pub async fn wait_for_finalized(node: &Node, height: u64, timeout: Duration) -> u64 {
    let start = Instant::now();
    loop {
        if let Some((_, finalized)) = heights(node).await
            && finalized >= height
        {
            return finalized;
        }
        assert!(
            start.elapsed() < timeout,
            "node did not finalize height {height} within {timeout:?}"
        );
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

/// Hash of block `number` on the node's best chain.
pub async fn block_hash(node: &Node, number: u64) -> String {
    node.rpc
        .request("chain_getBlockHash", rpc_params![number])
        .await
        .unwrap()
}

/// Calls runtime API `method` with SCALE-encoded `args` at block `at` (best if `None`).
pub async fn state_call(node: &Node, method: &str, args: &[u8], at: Option<&str>) -> Vec<u8> {
    let data = format!("0x{}", hex::encode(args));
    let result: String = match at {
        Some(at) => node
            .rpc
            .request("state_call", rpc_params![method, data, at])
            .await
            .unwrap(),
        None => node
            .rpc
            .request("state_call", rpc_params![method, data])
            .await
            .unwrap(),
    };
    unhex(&result)
}

/// Waits until `path` contains `needle` or `timeout` passes; returns whether it did.
pub async fn wait_for_log(path: &std::path::Path, needle: &str, timeout: Duration) -> bool {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if std::fs::read_to_string(path)
            .unwrap_or_default()
            .contains(needle)
        {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    false
}
