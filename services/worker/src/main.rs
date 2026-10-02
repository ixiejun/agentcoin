//! `ac-worker`: the public job worker and the publisher's tools.

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

use ac_worker::agent::MAX_SHARD;
use ac_worker::agent::live::os_salts;
use ac_worker::canary::CanaryFile;
use ac_worker::manifest::{MAX_DOCUMENTS, Manifest, check_shard, parse_eval, parse_texts};
use ac_worker::{EngineClient, exec};

#[derive(Parser)]
#[command(about = "AgentCoin public job worker")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Runs the worker service (configuration: see the README).
    Run {
        /// The configuration file (JSON).
        #[arg(long)]
        config: PathBuf,
    },
    /// Runs the publisher's result collector (configuration: see the README).
    Collect {
        /// The configuration file (JSON).
        #[arg(long)]
        config: PathBuf,
    },
    /// Builds a job's canaries: expected summaries of the chosen units, salts, the Merkle root
    /// (printed; publish the job with it) and the file of leaves and proofs (keep it private).
    Canary {
        /// The job number the job will be published as (embedding directions and leaves).
        #[arg(long)]
        job: u32,
        /// `eval`, `embed` or `clean`.
        #[arg(long, value_parser = ["eval", "embed", "clean"])]
        kind: String,
        /// The job's data manifest (its shards are downloaded and checked).
        #[arg(long)]
        manifest: PathBuf,
        /// Canary units, comma separated.
        #[arg(long, value_delimiter = ',', required = true)]
        units: Vec<u32>,
        /// The engine's base URL (evaluation and embedding).
        #[arg(long)]
        engine: Option<String>,
        /// The model's name on the engine.
        #[arg(long, default_value = "model")]
        model: String,
        /// Where to write the canary file.
        #[arg(long)]
        out: PathBuf,
    },
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

/// Executes a shard of `kind` (`eval`, `embed` or `clean`).
async fn execute(
    kind: &str,
    bytes: &[u8],
    engine: Option<&str>,
    model: &str,
    job: u32,
) -> Result<exec::Output> {
    let engine =
        || -> Result<EngineClient> { EngineClient::new(engine.context("--engine is required")?) };
    match kind {
        "eval" => exec::eval_unit(&engine()?, model, &parse_eval(bytes)?).await,
        "embed" => {
            let texts = parse_texts(bytes, ac_primitives::market::public::MAX_ITEMS)?;
            exec::embed_unit(&engine()?, model, job, &texts).await
        }
        _ => exec::clean_unit(&parse_texts(bytes, MAX_DOCUMENTS)?),
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    ac_worker::logging::init(log::LevelFilter::Info, ac_worker::logging::Sink::Stderr);
    match Cli::parse().command {
        Command::Run { config } => {
            ac_worker::agent::live::run(ac_worker::agent::config::Config::load(&config)?).await
        }
        Command::Collect { config } => {
            ac_worker::collect::run(ac_worker::collect::CollectConfig::load(&config)?).await
        }
        Command::Canary {
            job,
            kind,
            manifest,
            units,
            engine,
            model,
            out,
        } => {
            let bytes =
                std::fs::read(&manifest).with_context(|| format!("{}", manifest.display()))?;
            let parsed: Manifest = serde_json::from_slice(&bytes).context("malformed manifest")?;
            let http = ac_wallet::http::Client::new()?;
            let mut summaries = Vec::with_capacity(units.len());
            for unit in units {
                let shard = parsed
                    .units
                    .get(usize::try_from(unit)?)
                    .with_context(|| format!("the manifest has no unit {unit}"))?;
                let data = http.get_bytes(&shard.url, MAX_SHARD).await?;
                check_shard(&data, shard)?;
                let output = execute(&kind, &data, engine.as_deref(), &model, job).await?;
                summaries.push((unit, output.summary));
            }
            let file = CanaryFile::build(job, &summaries, os_salts()?)?;
            std::fs::write(&out, serde_json::to_vec_pretty(&file)?)?;
            println!("canary root: 0x{}", file.root);
            Ok(())
        }
        Command::Exec {
            kind,
            shard,
            engine,
            model,
            job,
            out,
        } => {
            let bytes = std::fs::read(&shard).with_context(|| format!("{}", shard.display()))?;
            let output = execute(&kind, &bytes, engine.as_deref(), &model, job).await?;
            if let Some(path) = out {
                std::fs::write(&path, &output.result)?;
            }
            println!("summary: 0x{}", hex::encode(&output.summary));
            println!("result hash: 0x{}", hex::encode(output.result_hash));
            Ok(())
        }
    }
}
