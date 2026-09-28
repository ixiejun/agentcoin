//! Harness for multi-node end-to-end tests (M1 and M2 acceptance).
//!
//! Tests run only with `AC_E2E=1` (design D12 of `m1-pq-chain`) and use the node binary at
//! `$AC_NODE`, defaulting to `target/debug/ac-node`; build it first with `cargo build -p ac-node`.
//!
//! A [`Testnet`] runs the authorities of a chain (by default the four of `local`: alice, bob,
//! charlie, dave) on this machine. Nodes can be stopped and restarted by name, and extra nodes —
//! full nodes without a key, or a second node with an authority's key — can join. Heights are
//! read through HTTP RPC; [`TestNode::subscribe`] follows imported or finalized heads over
//! WebSocket.

use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use jsonrpsee::core::client::{ClientT, Subscription, SubscriptionClientT};
use jsonrpsee::http_client::{HttpClient, HttpClientBuilder};
use jsonrpsee::rpc_params;
use jsonrpsee::ws_client::{WsClient, WsClientBuilder};

/// Development names of the authorities of generated chains, in order.
pub const NAMES: [&str; 10] = [
    "alice", "bob", "charlie", "dave", "eve", "ferdie", "george", "hannah", "ian", "julia",
];

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

/// Ports handed to test nodes come from this range, below Linux's ephemeral range
/// (32768-60999). A port picked from the ephemeral range and released before the node binds it
/// can meanwhile become the source port of another node's outgoing connection, and the node
/// then fails to start.
const PORT_RANGE: std::ops::Range<u16> = 20_000..32_000;

/// Offset of the next port to try within [`PORT_RANGE`]; 0 until first use.
static NEXT_PORT: AtomicU32 = AtomicU32::new(0);

/// A port in [`PORT_RANGE`] that is free now and was not handed out before by this process.
/// Each process starts at a pid-dependent offset, so concurrent test processes rarely overlap.
///
/// # Errors
///
/// No free port in the range.
pub fn free_port() -> std::io::Result<u16> {
    let span = u32::from(PORT_RANGE.end - PORT_RANGE.start);
    let start = (std::process::id() % 97) * 100 + 1;
    let _ = NEXT_PORT.compare_exchange(0, start, Ordering::SeqCst, Ordering::SeqCst);
    for _ in 0..span {
        let offset = NEXT_PORT.fetch_add(1, Ordering::SeqCst) % span;
        let port = PORT_RANGE.start + u16::try_from(offset).unwrap_or(0);
        if TcpListener::bind(("127.0.0.1", port)).is_ok() {
            return Ok(port);
        }
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::AddrInUse,
        "no free port for a test node",
    ))
}

/// The last `lines` lines of the file at `path` (empty if unreadable).
fn log_tail(path: &Path, lines: usize) -> String {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let all: Vec<&str> = text.lines().collect();
    all.get(all.len().saturating_sub(lines)..)
        .unwrap_or_default()
        .join("\n")
}

fn parse_number(value: &serde_json::Value) -> Option<u64> {
    u64::from_str_radix(value.as_str()?.trim_start_matches("0x"), 16).ok()
}

/// Decodes `0x`-prefixed hex.
///
/// # Errors
///
/// Invalid hex.
pub fn unhex(s: &str) -> Result<Vec<u8>, String> {
    let s = s.trim_start_matches("0x");
    (0..s.len())
        .step_by(2)
        .map(|i| {
            s.get(i..i.saturating_add(2))
                .and_then(|b| u8::from_str_radix(b, 16).ok())
                .ok_or_else(|| format!("invalid hex {s}"))
        })
        .collect()
}

/// Head events a node can be subscribed to.
#[derive(Clone, Copy, Debug)]
pub enum Heads {
    /// Every imported block (`chain_subscribeAllHeads`).
    All,
    /// Finalized heads (`chain_subscribeFinalizedHeads`).
    Finalized,
}

/// One node of a test network.
pub struct TestNode {
    /// Node name (a development name, or a label for extra nodes).
    pub name: String,
    /// Development key the node signs with; `None` for full nodes.
    pub key: Option<String>,
    /// HTTP RPC URL.
    pub url: String,
    /// WebSocket RPC URL (same port).
    pub ws_url: String,
    chain: String,
    rpc_port: u16,
    p2p_port: u16,
    base: PathBuf,
    bootnode: Option<String>,
    extra_args: Vec<String>,
    child: Option<Child>,
}

impl TestNode {
    fn new(
        name: &str,
        key: Option<&str>,
        chain: &str,
        base: &Path,
        bootnode: Option<String>,
    ) -> std::io::Result<Self> {
        let rpc_port = free_port()?;
        Ok(Self {
            name: name.to_string(),
            key: key.map(str::to_string),
            url: format!("http://127.0.0.1:{rpc_port}"),
            ws_url: format!("ws://127.0.0.1:{rpc_port}"),
            chain: chain.to_string(),
            rpc_port,
            p2p_port: free_port()?,
            base: base.to_path_buf(),
            bootnode,
            extra_args: Vec::new(),
            child: None,
        })
    }

    /// Path of the node's log file (appended to across restarts).
    #[must_use]
    pub fn log_path(&self) -> PathBuf {
        self.base.join(format!("{}.log", self.name))
    }

    /// Adds command-line arguments for the next start (e.g. log filters).
    pub fn push_args(&mut self, args: &[&str]) {
        self.extra_args
            .extend(args.iter().map(|a| (*a).to_string()));
    }

    /// Starts (or restarts) the node process, keeping its database.
    ///
    /// # Errors
    ///
    /// Spawn failures.
    pub fn start(&mut self) -> std::io::Result<()> {
        let log = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.log_path())?;
        let mut cmd = Command::new(node_binary());
        cmd.args(["--chain", &self.chain])
            .args(["--name", &self.name, "--unsafe-force-node-key-generation"])
            .arg("--base-path")
            .arg(self.base.join(&self.name))
            .args(["--rpc-port", &self.rpc_port.to_string()])
            .args([
                "--listen-addr",
                &format!("/ip4/127.0.0.1/tcp/{}", self.p2p_port),
            ])
            .args(["--no-telemetry", "--no-prometheus", "--no-mdns"])
            // Up to 10 local nodes: allow every RPC connection the tests open.
            .args(["--rpc-max-connections", "1000"]);
        if let Some(key) = &self.key {
            cmd.args(["--validator", "--dev-key", key]);
        }
        if let Some(boot) = &self.bootnode {
            cmd.args(["--bootnodes", boot]);
        }
        cmd.args(&self.extra_args);
        self.child = Some(cmd.stdout(Stdio::null()).stderr(Stdio::from(log)).spawn()?);
        Ok(())
    }

    /// Stops the process with SIGKILL (the database stays).
    pub fn stop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }

    /// Whether the process is running.
    #[must_use]
    pub fn is_running(&self) -> bool {
        self.child.is_some()
    }

    /// HTTP RPC client.
    ///
    /// # Errors
    ///
    /// Invalid URL (never for our URLs).
    pub fn rpc(&self) -> Result<HttpClient, jsonrpsee::core::client::Error> {
        HttpClientBuilder::default().build(&self.url)
    }

    /// WebSocket RPC client.
    ///
    /// # Errors
    ///
    /// Connection failures.
    pub async fn ws(&self) -> Result<WsClient, jsonrpsee::core::client::Error> {
        WsClientBuilder::default().build(&self.ws_url).await
    }

    /// Subscribes to imported or finalized heads; returns the client (keep it alive) and the
    /// header stream.
    ///
    /// # Errors
    ///
    /// Connection or subscription failures.
    pub async fn subscribe(
        &self,
        heads: Heads,
    ) -> Result<(WsClient, Subscription<serde_json::Value>), jsonrpsee::core::client::Error> {
        let client = self.ws().await?;
        let (method, unsubscribe) = match heads {
            Heads::All => ("chain_subscribeAllHeads", "chain_unsubscribeAllHeads"),
            Heads::Finalized => (
                "chain_subscribeFinalizedHeads",
                "chain_unsubscribeFinalizedHeads",
            ),
        };
        let sub = client.subscribe(method, rpc_params![], unsubscribe).await?;
        Ok((client, sub))
    }

    /// Best block number, or `None` if the node does not answer.
    pub async fn height(&self) -> Option<u64> {
        let header: serde_json::Value = self
            .rpc()
            .ok()?
            .request("chain_getHeader", rpc_params![])
            .await
            .ok()?;
        parse_number(&header["number"])
    }

    /// Finalized block number and hash, or `None` if the node does not answer.
    pub async fn finalized(&self) -> Option<(u64, String)> {
        let rpc = self.rpc().ok()?;
        let hash: String = rpc
            .request("chain_getFinalizedHead", rpc_params![])
            .await
            .ok()?;
        let header: serde_json::Value = rpc
            .request("chain_getHeader", rpc_params![&hash])
            .await
            .ok()?;
        Some((parse_number(&header["number"])?, hash))
    }

    /// Hash of block `number` on the best chain, if known.
    pub async fn hash_at(&self, number: u64) -> Option<String> {
        self.rpc()
            .ok()?
            .request("chain_getBlockHash", rpc_params![number])
            .await
            .ok()
    }

    /// The node's libp2p peer ID.
    pub async fn peer_id(&self) -> Option<String> {
        self.rpc()
            .ok()?
            .request("system_localPeerId", rpc_params![])
            .await
            .ok()
    }

    /// Multiaddress under which other nodes reach this one.
    pub async fn multiaddr(&self) -> Option<String> {
        let peer = self.peer_id().await?;
        Some(format!("/ip4/127.0.0.1/tcp/{}/p2p/{peer}", self.p2p_port))
    }

    /// Calls runtime API `method` with SCALE-encoded `args` at block `at` (best if `None`).
    ///
    /// # Errors
    ///
    /// RPC failures.
    pub async fn state_call(
        &self,
        method: &str,
        args: &[u8],
        at: Option<&str>,
    ) -> Result<Vec<u8>, String> {
        let data: String = std::iter::once("0x".to_string())
            .chain(args.iter().map(|b| format!("{b:02x}")))
            .collect();
        let rpc = self.rpc().map_err(|e| e.to_string())?;
        let result: String = match at {
            Some(at) => {
                rpc.request("state_call", rpc_params![method, data, at])
                    .await
            }
            None => rpc.request("state_call", rpc_params![method, data]).await,
        }
        .map_err(|e| e.to_string())?;
        unhex(&result)
    }
}

impl Drop for TestNode {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Which chain a [`Testnet`] runs.
pub struct TestnetConfig {
    /// Label used in the log directory name.
    pub label: String,
    /// `local`, or a path to a chain spec whose authorities are the first `authorities` of
    /// [`NAMES`] (see [`write_spec`]).
    pub chain: String,
    /// Number of authorities, started in [`NAMES`] order.
    pub authorities: usize,
    /// Extra command-line arguments of every node.
    pub args: Vec<String>,
}

impl TestnetConfig {
    /// The four-authority `local` chain.
    #[must_use]
    pub fn local(label: &str) -> Self {
        Self {
            label: label.to_string(),
            chain: "local".to_string(),
            authorities: 4,
            args: Vec::new(),
        }
    }
}

/// A running local test network.
pub struct Testnet {
    /// The authorities first, then extra nodes in the order they were added.
    pub nodes: Vec<TestNode>,
    /// Directory with databases and logs (kept for CI artifacts).
    pub base: PathBuf,
    chain: String,
    args: Vec<String>,
    bootnode: String,
}

impl Testnet {
    /// Starts the four authorities of `local`.
    ///
    /// # Errors
    ///
    /// See [`Testnet::start_with`].
    pub async fn start(label: &str, timeout: Duration) -> Result<Self, String> {
        Self::start_with(TestnetConfig::local(label), timeout).await
    }

    /// Starts the first authority, waits for its peer ID, then starts the others with it as
    /// bootnode.
    ///
    /// # Errors
    ///
    /// Spawn failures or the first node not answering within `timeout`.
    pub async fn start_with(config: TestnetConfig, timeout: Duration) -> Result<Self, String> {
        let base = std::env::var_os("AC_E2E_LOGS")
            .map_or_else(std::env::temp_dir, PathBuf::from)
            .join(format!("ac-e2e-{}-{}", config.label, std::process::id()));
        std::fs::create_dir_all(&base).map_err(|e| e.to_string())?;
        let names = NAMES
            .get(..config.authorities)
            .ok_or("too many authorities")?;
        let mut nodes = Vec::new();
        for name in names {
            let mut node = TestNode::new(name, Some(name), &config.chain, &base, None)
                .map_err(|e| e.to_string())?;
            node.extra_args.clone_from(&config.args);
            nodes.push(node);
        }
        let first = nodes.first_mut().ok_or("no nodes")?;
        first.start().map_err(|e| e.to_string())?;
        let started = Instant::now();
        let bootnode = loop {
            if let Some(addr) = first.multiaddr().await {
                break addr;
            }
            if started.elapsed() > timeout {
                return Err(format!(
                    "{} did not start; logs in {}",
                    first.name,
                    base.display()
                ));
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        };
        for node in nodes.iter_mut().skip(1) {
            node.bootnode = Some(bootnode.clone());
            node.start().map_err(|e| e.to_string())?;
        }
        Ok(Self {
            nodes,
            base,
            chain: config.chain,
            args: config.args,
            bootnode,
        })
    }

    /// Index of the node called `name`.
    ///
    /// # Errors
    ///
    /// No such node.
    pub fn index(&self, name: &str) -> Result<usize, String> {
        self.nodes
            .iter()
            .position(|n| n.name == name)
            .ok_or_else(|| format!("no node {name}"))
    }

    /// The node called `name`.
    ///
    /// # Errors
    ///
    /// No such node.
    pub fn node(&self, name: &str) -> Result<&TestNode, String> {
        self.nodes
            .get(self.index(name)?)
            .ok_or_else(|| format!("no node {name}"))
    }

    /// Stops the node called `name` (SIGKILL; the database stays).
    ///
    /// # Errors
    ///
    /// No such node.
    pub fn stop(&mut self, name: &str) -> Result<(), String> {
        let i = self.index(name)?;
        self.nodes.get_mut(i).ok_or("no such node")?.stop();
        Ok(())
    }

    /// Restarts the node called `name` on its database.
    ///
    /// # Errors
    ///
    /// No such node, or a spawn failure.
    pub fn restart(&mut self, name: &str) -> Result<(), String> {
        let i = self.index(name)?;
        self.nodes
            .get_mut(i)
            .ok_or("no such node")?
            .start()
            .map_err(|e| e.to_string())
    }

    /// Starts an extra node called `name`: a full node when `key` is `None`, otherwise a
    /// validator signing with the development key `key` (possibly an authority's key already
    /// in use). Returns its index.
    ///
    /// # Errors
    ///
    /// Spawn failures.
    pub fn add_node(
        &mut self,
        name: &str,
        key: Option<&str>,
        args: &[&str],
    ) -> Result<usize, String> {
        let mut node = TestNode::new(
            name,
            key,
            &self.chain,
            &self.base,
            Some(self.bootnode.clone()),
        )
        .map_err(|e| e.to_string())?;
        node.extra_args.clone_from(&self.args);
        node.push_args(args);
        node.start().map_err(|e| e.to_string())?;
        self.nodes.push(node);
        Ok(self.nodes.len().saturating_sub(1))
    }

    /// Waits until every node listed in `which` reaches best height `height`.
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
        self.wait(which, height, timeout, false).await
    }

    /// Waits until every node listed in `which` has finalized height `height`.
    ///
    /// # Errors
    ///
    /// Timeout, naming the finalized heights.
    pub async fn wait_finalized(
        &self,
        which: &[usize],
        height: u64,
        timeout: Duration,
    ) -> Result<(), String> {
        self.wait(which, height, timeout, true).await
    }

    async fn wait(
        &self,
        which: &[usize],
        height: u64,
        timeout: Duration,
        finalized: bool,
    ) -> Result<(), String> {
        let started = Instant::now();
        loop {
            let mut heights = Vec::new();
            let mut unreachable = Vec::new();
            for &i in which {
                let node = self.nodes.get(i).ok_or("no such node")?;
                let h = if finalized {
                    node.finalized().await.map(|f| f.0)
                } else {
                    node.height().await
                };
                if h.is_none() {
                    unreachable.push(node);
                }
                heights.push(h.unwrap_or(0));
            }
            if heights.iter().all(|h| *h >= height) {
                return Ok(());
            }
            if started.elapsed() > timeout {
                // A node whose RPC does not answer has usually exited: show why.
                let tails: String = unreachable
                    .iter()
                    .map(|n| {
                        format!(
                            "\n--- {} (RPC unreachable), last log lines:\n{}",
                            n.name,
                            log_tail(&n.log_path(), 40)
                        )
                    })
                    .collect();
                return Err(format!(
                    "{} heights {heights:?} did not reach {height} within {timeout:?}; logs in {}{tails}",
                    if finalized { "finalized" } else { "best" },
                    self.base.display()
                ));
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }

    /// Indices of all running nodes.
    #[must_use]
    pub fn running(&self) -> Vec<usize> {
        (0..self.nodes.len())
            .filter(|i| self.nodes.get(*i).is_some_and(TestNode::is_running))
            .collect()
    }
}

/// Writes a chain spec with the first `authorities` of [`NAMES`] as authorities (and an epoch
/// length of at least twice their number) to `dir/spec-<n>.json`, starting from the `local`
/// spec exported by the node binary.
///
/// # Errors
///
/// Export, key derivation or file errors.
pub fn write_spec(dir: &Path, authorities: usize) -> Result<PathBuf, String> {
    let output = Command::new(node_binary())
        .args(["export-chain-spec", "--chain", "local"])
        .output()
        .map_err(|e| e.to_string())?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).into_owned());
    }
    let mut spec: serde_json::Value =
        serde_json::from_slice(&output.stdout).map_err(|e| e.to_string())?;
    let keys = NAMES
        .get(..authorities)
        .ok_or("too many authorities")?
        .iter()
        .map(|name| {
            ac_runtime::genesis_config_presets::dev_public_key(name, ac_crypto::SigAlg::MlDsa65)
                .map_err(|e| e.to_string())
                .and_then(|k| serde_json::to_value(k).map_err(|e| e.to_string()))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let patch = spec
        .pointer_mut("/genesis/runtimeGenesis/patch")
        .ok_or("spec without a genesis patch")?;
    patch["auraPq"]["authorities"] = serde_json::Value::Array(keys);
    let length = u64::try_from(authorities)
        .map_err(|e| e.to_string())?
        .saturating_mul(2)
        .max(20);
    patch["validatorSet"]["epochLength"] = length.into();
    let path = dir.join(format!("spec-{authorities}.json"));
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    std::fs::write(
        &path,
        serde_json::to_vec_pretty(&spec).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    Ok(path)
}

#[cfg(test)]
mod port_tests {
    #![allow(clippy::unwrap_used)]

    use super::{PORT_RANGE, free_port, log_tail};

    // Ports come from outside the ephemeral range and are never handed out twice.
    #[test]
    fn ports_are_distinct_and_outside_the_ephemeral_range() {
        let ports: Vec<u16> = (0..50).map(|_| free_port().unwrap()).collect();
        assert!(ports.iter().all(|p| PORT_RANGE.contains(p)));
        let mut unique = ports.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), ports.len());
    }

    #[test]
    fn log_tail_keeps_the_last_lines() {
        let path = std::env::temp_dir().join(format!("ac-e2e-tail-{}", std::process::id()));
        std::fs::write(&path, "a\nb\nc\nd\n").unwrap();
        assert_eq!(log_tail(&path, 2), "c\nd");
        assert_eq!(log_tail(&path, 10), "a\nb\nc\nd");
        std::fs::remove_file(&path).unwrap();
        assert_eq!(log_tail(&path, 3), "");
    }
}
