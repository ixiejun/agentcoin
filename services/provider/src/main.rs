//! `ac-provider`: the inference provider agent.

use std::path::PathBuf;

use ac_provider::chain_tasks::{self, check_registration};
use ac_provider::logging::{self, Sink, TARGET};
use ac_provider::{Config, Service, keys};
use ac_wallet::market::parse_model_id;
use ac_wallet::ops::Signer;
use ac_wallet::wallet::read_password_file;
use ac_wallet::{NodeClient, Wallet};
use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "ac-provider", about = "AgentCoin inference provider agent")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Generate the X-Wing key and print the public key to register.
    Keygen {
        /// Key file to create.
        #[arg(long)]
        out: PathBuf,
        /// File holding the key file's passphrase.
        #[arg(long)]
        password_file: PathBuf,
    },
    /// Serve sealed requests from registered gateways.
    Run(RunArgs),
}

#[derive(clap::Args)]
struct RunArgs {
    /// The provider's wallet file.
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
    /// OpenAI-compatible engine base URL (vLLM, SGLang, llama.cpp server, ...).
    #[arg(long, default_value = "http://127.0.0.1:8000")]
    engine: String,
    /// `<on-chain model id>=<engine model name>`, repeatable.
    #[arg(long = "model", required = true)]
    models: Vec<String>,
    /// Address to listen on (the registered endpoint should reach it).
    #[arg(long, default_value = "127.0.0.1:8421")]
    listen: String,
    /// Directory for co-signed receipts.
    #[arg(long, default_value = "ac-provider-data")]
    data_dir: PathBuf,
    /// Log level (error, warn, info, debug, trace). Lines never contain request content.
    #[arg(long, default_value = "info")]
    log_level: log::LevelFilter,
}

#[tokio::main]
async fn main() -> Result<()> {
    match Cli::parse().command {
        Command::Keygen { out, password_file } => {
            let key = keys::generate(&out, &read_password_file(&password_file)?)?;
            println!("kem key: {}", keys::encode(&key));
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
    let (kem, kem_public) = keys::load(&a.kem_key, &kem_password)?;
    let mapping = a
        .models
        .iter()
        .map(|m| {
            let (id, name) = m
                .split_once('=')
                .context("--model must be <model id>=<engine model name>")?;
            Ok((parse_model_id(id)?, name.to_string()))
        })
        .collect::<Result<Vec<_>>>()?;

    let node = NodeClient::new(&a.node)?;
    let genesis = node.chain_context().await?.genesis_hash;
    let record = node
        .market_provider(&signer.account)
        .await?
        .context("this account is not a registered provider (register with ac-wallet market provider register)")?;
    let models = check_registration(&record, &kem_public, &mapping)?;

    let service = Service::new(
        Config {
            account: signer.account.clone(),
            key: wallet.current_key(&password)?,
            kem,
            kem_public,
            genesis,
            models,
            engine: a.engine,
            store_dir: a.data_dir.join("receipts"),
        },
        Box::new(node.clone()),
    )?;
    tokio::spawn(chain_tasks::run(
        node,
        signer,
        std::sync::Arc::clone(&service),
        mapping,
    ));
    let listener = tokio::net::TcpListener::bind(&a.listen)
        .await
        .with_context(|| format!("binding {}", a.listen))?;
    let addr = listener.local_addr()?;
    println!("listening on {addr}");
    log::info!(target: TARGET, "serving on {addr}");
    ac_wallet::http::serve(listener, move |req| {
        std::sync::Arc::clone(&service).handle(req)
    })
    .await?;
    Ok(())
}
