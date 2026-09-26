//! Harness for multi-node end-to-end tests (spec node/chain-spec, M1 acceptance).
//!
//! Tests run only with `AC_E2E=1` (design D12) and use the node binary at `$AC_NODE`, defaulting
//! to `target/debug/ac-node`; build it first with `cargo build -p ac-node`.

use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use jsonrpsee::core::client::ClientT;
use jsonrpsee::http_client::{HttpClient, HttpClientBuilder};
use jsonrpsee::rpc_params;

/// Whether end-to-end tests are enabled (`AC_E2E=1`).
#[must_use]
pub fn enabled() -> bool {
    std::env::var("AC_E2E").is_ok_and(|v| v == "1")
}

/// Path of the node binary.
#[must_use]
pub fn node_binary() -> PathBuf {
    std::env::var_os("AC_NODE").map_or_else(
        || Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/debug/ac-node"),
        PathBuf::from,
    )
}

fn free_port() -> std::io::Result<u16> {
    Ok(TcpListener::bind("127.0.0.1:0")?.local_addr()?.port())
}

/// One authority of a local testnet.
pub struct TestNode {
    /// Development name (`alice`, `bob`, `charlie`).
    pub name: &'static str,
    /// RPC URL.
    pub url: String,
    rpc_port: u16,
    p2p_port: u16,
    base: PathBuf,
    bootnode: Option<String>,
    child: Option<Child>,
}

impl TestNode {
    /// Starts (or restarts) the node process, keeping its database.
    ///
    /// # Errors
    ///
    /// Spawn failures.
    pub fn start(&mut self) -> std::io::Result<()> {
        let log = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.base.join(format!("{}.log", self.name)))?;
        let mut cmd = Command::new(node_binary());
        cmd.args(["--chain", "local", "--validator", "--dev-key", self.name])
            .args(["--name", self.name, "--unsafe-force-node-key-generation"])
            .arg("--base-path")
            .arg(self.base.join(self.name))
            .args(["--rpc-port", &self.rpc_port.to_string()])
            .args([
                "--listen-addr",
                &format!("/ip4/127.0.0.1/tcp/{}", self.p2p_port),
            ])
            .args(["--no-telemetry", "--no-prometheus", "--no-mdns"]);
        if let Some(boot) = &self.bootnode {
            cmd.args(["--bootnodes", boot]);
        }
        self.child = Some(cmd.stdout(Stdio::null()).stderr(Stdio::from(log)).spawn()?);
        Ok(())
    }

    /// Stops the process (the database stays).
    pub fn stop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }

    /// RPC client.
    ///
    /// # Errors
    ///
    /// Invalid URL (never for our URLs).
    pub fn rpc(&self) -> Result<HttpClient, jsonrpsee::core::client::Error> {
        HttpClientBuilder::default().build(&self.url)
    }

    /// Best block number, or `None` if the node does not answer.
    pub async fn height(&self) -> Option<u64> {
        let header: serde_json::Value = self
            .rpc()
            .ok()?
            .request("chain_getHeader", rpc_params![])
            .await
            .ok()?;
        u64::from_str_radix(header["number"].as_str()?.trim_start_matches("0x"), 16).ok()
    }

    /// Hash of block `number`, if known.
    pub async fn hash_at(&self, number: u64) -> Option<String> {
        self.rpc()
            .ok()?
            .request("chain_getBlockHash", rpc_params![number])
            .await
            .ok()
    }
}

impl Drop for TestNode {
    fn drop(&mut self) {
        self.stop();
    }
}

/// A running three-authority local testnet.
pub struct Testnet {
    /// alice, bob, charlie.
    pub nodes: Vec<TestNode>,
    /// Directory with databases and logs (kept for CI artifacts).
    pub base: PathBuf,
}

impl Testnet {
    /// Starts alice, waits for her peer ID, then starts bob and charlie with alice as bootnode.
    ///
    /// # Errors
    ///
    /// Spawn failures or alice not answering within `timeout`.
    pub async fn start(label: &str, timeout: Duration) -> Result<Self, String> {
        let base = std::env::var_os("AC_E2E_LOGS")
            .map_or_else(std::env::temp_dir, PathBuf::from)
            .join(format!("ac-e2e-{label}-{}", std::process::id()));
        std::fs::create_dir_all(&base).map_err(|e| e.to_string())?;
        let mut nodes = Vec::new();
        for name in ["alice", "bob", "charlie"] {
            let rpc_port = free_port().map_err(|e| e.to_string())?;
            nodes.push(TestNode {
                name,
                url: format!("http://127.0.0.1:{rpc_port}"),
                rpc_port,
                p2p_port: free_port().map_err(|e| e.to_string())?,
                base: base.clone(),
                bootnode: None,
                child: None,
            });
        }
        let mut net = Self { nodes, base };
        let alice = net.nodes.first_mut().ok_or("no nodes")?;
        alice.start().map_err(|e| e.to_string())?;
        let started = Instant::now();
        let peer = loop {
            if let Ok(rpc) = alice.rpc()
                && let Ok(peer) = rpc
                    .request::<String, _>("system_localPeerId", rpc_params![])
                    .await
            {
                break peer;
            }
            if started.elapsed() > timeout {
                return Err(format!(
                    "alice did not start; logs in {}",
                    net.base.display()
                ));
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        };
        let bootnode = format!("/ip4/127.0.0.1/tcp/{}/p2p/{peer}", alice.p2p_port);
        for node in net.nodes.iter_mut().skip(1) {
            node.bootnode = Some(bootnode.clone());
            node.start().map_err(|e| e.to_string())?;
        }
        Ok(net)
    }

    /// Waits until every running node listed in `which` reaches `height`.
    ///
    /// # Errors
    ///
    /// Timeout, naming the node heights.
    pub async fn wait_all(
        &self,
        which: &[usize],
        height: u64,
        timeout: Duration,
    ) -> Result<(), String> {
        let started = Instant::now();
        loop {
            let mut heights = Vec::new();
            for &i in which {
                let node = self.nodes.get(i).ok_or("no such node")?;
                heights.push(node.height().await.unwrap_or(0));
            }
            if heights.iter().all(|h| *h >= height) {
                return Ok(());
            }
            if started.elapsed() > timeout {
                return Err(format!(
                    "heights {heights:?} did not reach {height} within {timeout:?}; logs in {}",
                    self.base.display()
                ));
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }
}
