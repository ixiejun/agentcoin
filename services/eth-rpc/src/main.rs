//! `ac-eth-rpc`: the AgentCoin Ethereum JSON-RPC adapter (m4-evm, spec evm/eth-rpc).
//!
//! Serves Ethereum JSON-RPC over HTTP and WebSocket for Foundry and other tools. It reads the
//! chain through a node's public RPC, holds no keys, signs nothing, and relays only ML-DSA-signed
//! AgentCoin contract transactions. Logs record method names, durations and error classes only.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use clap::Parser;
use jsonrpsee::server::ServerBuilder;

use ac_eth_rpc::chain::Chain;
use ac_eth_rpc::index;
use ac_eth_rpc::logging::StderrLogger;
use ac_eth_rpc::node::RpcNode;
use ac_eth_rpc::rpc::{self, Ctx, Limits};

/// Command-line options.
#[derive(Debug, Parser)]
#[command(name = "ac-eth-rpc", version, about)]
struct Cli {
    /// URL of the node's JSON-RPC.
    #[arg(long, default_value = "http://127.0.0.1:9944")]
    node_url: String,
    /// Address to serve on (loopback only by default).
    #[arg(long, default_value = "127.0.0.1:8545")]
    listen: SocketAddr,
    /// Blocks before start-up to index for receipts and transactions.
    #[arg(long, default_value_t = 10_000)]
    index_depth: u64,
    /// Largest block span of one `eth_getLogs` query.
    #[arg(long, default_value_t = 1_000)]
    max_log_range: u64,
    /// Largest number of simultaneous connections.
    #[arg(long, default_value_t = 100)]
    max_connections: u32,
    /// Log level: error, warn, info or debug.
    #[arg(long, default_value = "info")]
    log_level: log::LevelFilter,
}

static LOGGER: StderrLogger = StderrLogger;

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    log::set_logger(&LOGGER).map_err(|e| anyhow::anyhow!("logger: {e}"))?;
    log::set_max_level(cli.log_level);

    let node = RpcNode::new(&cli.node_url).context("node URL")?;
    let ctx = Arc::new(Ctx {
        chain: Chain::new(node),
        index: Default::default(),
        limits: Limits {
            max_log_range: cli.max_log_range,
        },
    });

    // Follow the chain: backfill, then index every new block.
    let follower = Arc::clone(&ctx);
    let depth = cli.index_depth;
    tokio::spawn(async move {
        loop {
            if index::catch_up(&follower.chain, &follower.index, depth)
                .await
                .is_err()
            {
                log::warn!(target: "eth-rpc", "index: node unavailable, retrying");
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    });

    let server = ServerBuilder::default()
        .max_connections(cli.max_connections)
        .build(cli.listen)
        .await
        .context("bind the listen address")?;
    let address = server.local_addr()?;
    let handle = server.start(rpc::module(ctx)?);
    log::info!(target: "eth-rpc", "serving Ethereum JSON-RPC on {address}");
    tokio::select! {
        () = handle.clone().stopped() => {}
        _ = tokio::signal::ctrl_c() => {
            let _ = handle.stop();
        }
    }
    Ok(())
}
