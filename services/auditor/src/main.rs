//! `ac-auditor`: the auditor agent. `recheck` re-checks finished inferences against their TOPLOC
//! proofs with a verify-mode engine; `calibration-case` builds re-check cases from a prover
//! engine's candidates, for the calibration of the thresholds.

use std::path::{Path, PathBuf};
use std::time::Duration;

use ac_auditor::logging::{self, Sink, TARGET};
use ac_auditor::{EngineClient, RecheckCase, Rows, Verifier, calibration, recheck};
use ac_primitives::market::model::QuantType;
use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use serde_json::json;

#[derive(Parser)]
#[command(name = "ac-auditor", about = "AgentCoin auditor agent")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Re-check inferences: prints one JSON line per case (outcome, reason, metrics).
    Recheck(RecheckArgs),
    /// Build a re-check case from a prover engine's candidates (calibration and tests only:
    /// the receipt is signed by a key made for the run). Reads JSON, prints the case.
    CalibrationCase {
        /// Input file (`-` for standard input).
        #[arg(long, default_value = "-")]
        input: PathBuf,
    },
}

#[derive(clap::Args)]
struct RecheckArgs {
    /// One case file (JSON).
    #[arg(long, conflicts_with = "cases", required_unless_present = "cases")]
    case: Option<PathBuf>,
    /// A directory of case files (`*.json`, in name order).
    #[arg(long)]
    cases: Option<PathBuf>,
    /// The re-check engine's base URL (vLLM with the plugin in verify mode).
    #[arg(long, default_value = "http://127.0.0.1:8000")]
    engine: String,
    /// The engine's model name, instead of each case's `engine_model`.
    #[arg(long)]
    engine_model: Option<String>,
    /// Local Unix socket the engine's plugin sends to (its `AGENTCOIN_TOPLOC_SOCKET`).
    #[arg(long)]
    socket: PathBuf,
    /// Node RPC URL, to read each model's registered precision.
    #[arg(long, conflicts_with = "quant", required_unless_present = "quant")]
    node: Option<String>,
    /// The models' precision instead of reading it from a node (bf16, fp16, fp8, int8, int4).
    #[arg(long)]
    quant: Option<String>,
    /// Seconds to wait for the engine's plugin to connect.
    #[arg(long, default_value_t = 120)]
    connect_wait: u64,
    /// Log level (error, warn, info, debug, trace). Lines never contain request content.
    #[arg(long, default_value = "info")]
    log_level: log::LevelFilter,
}

fn parse_quant(text: &str) -> Result<QuantType> {
    Ok(match text.to_ascii_lowercase().as_str() {
        "bf16" => QuantType::Bf16,
        "fp16" => QuantType::Fp16,
        "fp8" => QuantType::Fp8,
        "int8" => QuantType::Int8,
        "int4" => QuantType::Int4,
        other => bail!("unknown precision {other}: use bf16, fp16, fp8, int8 or int4"),
    })
}

fn case_files(args: &RecheckArgs) -> Result<Vec<PathBuf>> {
    if let Some(c) = &args.case {
        return Ok(vec![c.clone()]);
    }
    let dir = args.cases.as_deref().context("--case or --cases")?;
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .with_context(|| format!("reading {}", dir.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .collect();
    files.sort();
    Ok(files)
}

fn name(p: &Path) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

async fn run_recheck(args: RecheckArgs) -> Result<()> {
    logging::init(args.log_level, Sink::Stderr);
    let rows = Rows::listen(&args.socket)?;
    let verifier = Verifier::new(EngineClient::new(&args.engine)?, rows.clone());
    let node = args
        .node
        .as_deref()
        .map(ac_wallet::NodeClient::new)
        .transpose()?;
    let fixed = args.quant.as_deref().map(parse_quant).transpose()?;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(args.connect_wait);
    while rows.connections() == 0 {
        if tokio::time::Instant::now() >= deadline {
            bail!(
                "the re-check engine's plugin did not connect to {}",
                args.socket.display()
            );
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    for file in case_files(&args)? {
        let text = std::fs::read_to_string(&file)
            .with_context(|| format!("reading {}", file.display()))?;
        let mut case: RecheckCase =
            serde_json::from_str(&text).with_context(|| format!("parsing {}", name(&file)))?;
        if let Some(m) = &args.engine_model {
            case.engine_model.clone_from(m);
        }
        let quant = match (fixed, &node) {
            (Some(q), _) => q,
            (None, Some(node)) => {
                let model = ac_wallet::market::parse_model_id(&case.model)?;
                recheck::model_quant(node, model).await?
            }
            (None, None) => bail!("--node or --quant"),
        };
        let report = verifier.recheck(&case, quant).await;
        let mut line = serde_json::to_value(&report)?;
        if let Some(obj) = line.as_object_mut() {
            obj.insert("case".into(), json!(name(&file)));
        }
        println!("{line}");
    }
    log::debug!(target: TARGET, "re-checks done");
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    match Cli::parse().command {
        Command::Recheck(args) => run_recheck(args).await,
        Command::CalibrationCase { input } => {
            let text = if input.as_os_str() == "-" {
                std::io::read_to_string(std::io::stdin())?
            } else {
                std::fs::read_to_string(&input)?
            };
            let case = calibration::case_from_json(&text)?;
            println!("{}", serde_json::to_string(&case)?);
            Ok(())
        }
    }
}
