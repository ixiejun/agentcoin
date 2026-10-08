//! `ac-auditor`: the auditor agent. `recheck` re-checks finished inferences against their TOPLOC
//! proofs with a verify-mode engine, from case files or from audit evidence; `evidence` exports a
//! case as the evidence a failing verdict commits to; `calibration-case` builds re-check cases
//! from a prover engine's candidates, for the calibration of the thresholds.

use std::path::{Path, PathBuf};
use std::time::Duration;

use ac_auditor::logging::{self, Sink, TARGET};
use ac_auditor::{EngineClient, RecheckCase, Rows, Verifier, calibration, evidence, recheck};
use ac_market_proto::audit::{AuditEvidence, EvidenceError};
use ac_primitives::market::model::QuantType;
use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use parity_scale_codec::Encode;
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
    /// Export a re-check case as audit evidence: writes the evidence file and prints its
    /// commitment (the one a failing verdict carries). Never prints the content.
    Evidence {
        /// The case file (JSON).
        #[arg(long)]
        case: PathBuf,
        /// Where to write the evidence (SCALE bytes).
        #[arg(long)]
        out: PathBuf,
    },
    /// Run the auditor agent: mystery-shopper audits of the assigned providers, verdicts, the
    /// evidence service, and reviews of disputes (configuration: a JSON file, see the README).
    Run {
        /// The configuration file.
        #[arg(long)]
        config: PathBuf,
        /// Log level (error, warn, info, debug, trace). Lines never contain request content.
        #[arg(long, default_value = "info")]
        log_level: log::LevelFilter,
    },
    /// Create the X-Wing key file of the evidence service; prints the public key.
    Keygen {
        /// Where to write the encrypted key.
        #[arg(long)]
        out: PathBuf,
        /// Password file encrypting it.
        #[arg(long)]
        password_file: PathBuf,
    },
    /// Print the thresholds this auditor judges by (version, audit length band, bounds) as
    /// JSON, for the calibration script.
    Thresholds,
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
    #[arg(long, conflicts_with_all = ["cases", "evidence"], required_unless_present_any = ["cases", "evidence"])]
    case: Option<PathBuf>,
    /// A directory of case files (`*.json`, in name order).
    #[arg(long, conflicts_with = "evidence")]
    cases: Option<PathBuf>,
    /// An evidence file (a dispute's reviewer re-checks it); needs `--commitment` and
    /// `--engine-model`.
    #[arg(long, requires_all = ["commitment", "engine_model"])]
    evidence: Option<PathBuf>,
    /// The commitment recorded on chain for `--evidence` (hex).
    #[arg(long)]
    commitment: Option<String>,
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

/// The case of `--evidence`, or `None` after printing the mismatch line when the evidence does
/// not match its commitment (no engine is involved then).
fn evidence_case(args: &RecheckArgs, file: &Path) -> Result<Option<RecheckCase>> {
    let bytes = std::fs::read(file).with_context(|| format!("reading {}", file.display()))?;
    let hex_c = args.commitment.as_deref().context("--commitment")?;
    let commitment: [u8; 32] = hex::decode(hex_c.trim_start_matches("0x"))
        .ok()
        .and_then(|v| v.try_into().ok())
        .context("--commitment: 32 bytes in hex")?;
    match AuditEvidence::open(&bytes, &commitment) {
        Ok(e) => {
            let model = args.engine_model.as_deref().context("--engine-model")?;
            Ok(Some(evidence::case_of(&e, model)?))
        }
        Err(EvidenceError::CommitmentMismatch) => {
            println!(
                "{}",
                json!({"case": name(file), "outcome": "mismatch", "reason": "evidence does not match the commitment"})
            );
            Ok(None)
        }
        Err(e) => bail!("{}: {e}", name(file)),
    }
}

async fn run_recheck(args: RecheckArgs) -> Result<()> {
    logging::init(args.log_level, Sink::Stderr);
    // Evidence is checked against its commitment before any engine is involved.
    let from_evidence = match &args.evidence {
        Some(file) => match evidence_case(&args, file)? {
            Some(case) => Some((file.clone(), case)),
            None => return Ok(()),
        },
        None => None,
    };
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
    let cases: Vec<(PathBuf, Option<RecheckCase>)> = match from_evidence {
        Some((file, case)) => vec![(file, Some(case))],
        None => case_files(&args)?.into_iter().map(|f| (f, None)).collect(),
    };
    for (file, given) in cases {
        let mut case: RecheckCase = match given {
            Some(c) => c,
            None => {
                let text = std::fs::read_to_string(&file)
                    .with_context(|| format!("reading {}", file.display()))?;
                serde_json::from_str(&text).with_context(|| format!("parsing {}", name(&file)))?
            }
        };
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
            // The verdict outcome as the chain encodes it (SCALE, hex), for the wallet.
            let onchain = evidence::onchain_outcome(&report.verdict);
            obj.insert("onchain".into(), json!(hex::encode(onchain.encode())));
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
        Command::Run { config, log_level } => {
            logging::init(log_level, Sink::Stderr);
            ac_auditor::agent::live::run(ac_auditor::agent::config::Config::load(&config)?).await
        }
        Command::Keygen { out, password_file } => {
            let password = ac_wallet::wallet::read_password_file(&password_file)?;
            let key = ac_wallet::kem_key::generate(&out, &password)?;
            println!("kem key: {}", ac_wallet::kem_key::encode(&key));
            Ok(())
        }
        Command::Evidence { case, out } => {
            let text = std::fs::read_to_string(&case)
                .with_context(|| format!("reading {}", case.display()))?;
            let case: RecheckCase =
                serde_json::from_str(&text).with_context(|| format!("parsing {}", name(&case)))?;
            let e = evidence::evidence_of(&case)?;
            std::fs::write(&out, e.to_bytes())
                .with_context(|| format!("writing {}", out.display()))?;
            let commitment = e.commitment().map_err(|e| anyhow::anyhow!("{e}"))?;
            println!("{}", json!({"commitment": hex::encode(commitment)}));
            Ok(())
        }
        Command::Thresholds => {
            let t = ac_market_proto::toploc::AUDIT_THRESHOLDS;
            println!("{}", calibration::thresholds_json(&t));
            Ok(())
        }
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
