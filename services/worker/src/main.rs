//! `ac-worker`: the public job worker and the publisher's tools.

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

use ac_worker::manifest::{MAX_DOCUMENTS, parse_eval, parse_texts};
use ac_worker::{EngineClient, exec};

#[derive(Parser)]
#[command(about = "AgentCoin public job worker")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Runs one unit's shard and prints its summary and result hash (checks and debugging).
    Exec {
        /// `eval`, `embed` or `clean`.
        #[arg(long, value_parser = ["eval", "embed", "clean"])]
        kind: String,
        /// The shard (JSON Lines).
        #[arg(long)]
        shard: PathBuf,
        /// The engine's base URL (evaluation and embedding).
        #[arg(long)]
        engine: Option<String>,
        /// The model's name on the engine.
        #[arg(long, default_value = "model")]
        model: String,
        /// The job number (embedding directions).
        #[arg(long, default_value_t = 0)]
        job: u32,
        /// Also write the full result here.
        #[arg(long)]
        out: Option<PathBuf>,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    ac_worker::logging::init(log::LevelFilter::Info, ac_worker::logging::Sink::Stderr);
    match Cli::parse().command {
        Command::Exec {
            kind,
            shard,
            engine,
            model,
            job,
            out,
        } => {
            let bytes = std::fs::read(&shard).with_context(|| format!("{}", shard.display()))?;
            let engine = || -> Result<EngineClient> {
                EngineClient::new(engine.as_deref().context("--engine is required")?)
            };
            let output = match kind.as_str() {
                "eval" => exec::eval_unit(&engine()?, &model, &parse_eval(&bytes)?).await?,
                "embed" => {
                    let texts = parse_texts(&bytes, ac_primitives::market::public::MAX_ITEMS)?;
                    exec::embed_unit(&engine()?, &model, job, &texts).await?
                }
                _ => exec::clean_unit(&parse_texts(&bytes, MAX_DOCUMENTS)?)?,
            };
            if let Some(path) = out {
                std::fs::write(&path, &output.result)?;
            }
            println!("summary: 0x{}", hex::encode(&output.summary));
            println!("result hash: 0x{}", hex::encode(output.result_hash));
            Ok(())
        }
    }
}
