//! `ac-gateway`: the inference gateway.

use std::path::PathBuf;
use std::sync::Arc;

use ac_gateway::chain_tasks::{self, Schedule};
use ac_gateway::logging::{self, Sink, TARGET};
use ac_gateway::routing::Router;
use ac_gateway::{Config, Gateway};
use ac_primitives::market::records::GatewayStatus;
use ac_wallet::kem_key;
use ac_wallet::ops::Signer;
use ac_wallet::wallet::read_password_file;
use ac_wallet::{NodeClient, Wallet};
use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "ac-gateway", about = "AgentCoin inference gateway")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Generate the gateway's X-Wing key.
    Keygen {
        /// Key file to create.
        #[arg(long)]
        out: PathBuf,
        /// File holding the key file's passphrase.
        #[arg(long)]
        password_file: PathBuf,
    },
    /// Serve users.
    Run(RunArgs),
}

#[derive(clap::Args)]
struct RunArgs {
    /// The gateway's wallet file.
    #[arg(long)]
    wallet: PathBuf,
    /// File holding the wallet passphrase.
    #[arg(long)]
    password_file: PathBuf,
    /// The X-Wing key file (from `keygen`).
    #[arg(long)]
    kem_key: PathBuf,
    /// File holding the key file's passphrase (default: the wallet's).
    #[arg(long)]
    kem_password_file: Option<PathBuf>,
    /// Node RPC URL.
    #[arg(long, default_value = "http://127.0.0.1:9944")]
    node: String,
    /// Address to listen on (the registered endpoint should reach it).
    #[arg(long, default_value = "127.0.0.1:8431")]
    listen: String,
    /// Directory for the channel ledger and reported receipts.
    #[arg(long, default_value = "ac-gateway-data")]
    data_dir: PathBuf,
    /// Blocks between report rounds (default: one live emission epoch; at most an epoch).
    #[arg(long, default_value_t = 3_600)]
    report_interval: u64,
    /// Output-token limit applied when a request gives none.
    #[arg(long, default_value_t = 4_096)]
    max_output_tokens: u32,
    /// Log level (error, warn, info, debug, trace). Lines never contain request content.
    #[arg(long, default_value = "info")]
    log_level: log::LevelFilter,
}

#[tokio::main]
async fn main() -> Result<()> {
    match Cli::parse().command {
        Command::Keygen { out, password_file } => {
            let key = kem_key::generate(&out, &read_password_file(&password_file)?)?;
            println!("kem key: {}", kem_key::encode(&key));
            Ok(())
        }
        Command::Run(args) => run(args).await,
    }
}

async fn run(a: RunArgs) -> Result<()> {
    logging::init(a.log_level, Sink::Stderr);
    let password = read_password_file(&a.password_file)?;
    let wallet = Wallet::load(&a.wallet)?;
    let signer = Signer::from_wallet(&wallet, &password)?;
    let kem_password = match &a.kem_password_file {
        Some(p) => read_password_file(p)?,
        None => password.clone(),
    };
    let (kem, kem_public) = kem_key::load(&a.kem_key, &kem_password)?;
    let node = NodeClient::new(&a.node)?;
    let genesis = node.chain_context().await?.genesis_hash;
    let record = node
        .market_gateway(&signer.account)
        .await?
        .context("this account is not a registered gateway (register with ac-wallet market gateway register)")?;
    if record.status != GatewayStatus::Active {
        bail!("the gateway is not active on chain");
    }
    let epoch = node.epoch_length().await?;
    let report_interval = a.report_interval.clamp(1, epoch.max(1));

    let router = Arc::new(Router::new());
    router.refresh(&node).await?;
    let gateway = Gateway::new(
        Config {
            account: signer.account.clone(),
            key: wallet.current_key(&password)?,
            kem,
            kem_public,
            genesis,
            max_output_tokens: a.max_output_tokens,
            data_dir: a.data_dir.clone(),
        },
        Arc::new(node.clone()),
        Arc::clone(&router),
    )?;
    tokio::spawn(chain_tasks::run(
        (node, signer),
        Arc::clone(&gateway),
        router,
        a.data_dir,
        Schedule { report_interval },
    ));
    let listener = tokio::net::TcpListener::bind(&a.listen)
        .await
        .with_context(|| format!("binding {}", a.listen))?;
    let addr = listener.local_addr()?;
    println!("listening on {addr}");
    log::info!(target: TARGET, "serving on {addr}; reports every {report_interval} blocks");
    ac_wallet::http::serve(listener, move |req| Arc::clone(&gateway).handle(req)).await?;
    Ok(())
}
