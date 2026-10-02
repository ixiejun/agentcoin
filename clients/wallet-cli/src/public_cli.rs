//! `ac-wallet public …`: the public job commands (spec `clients/wallet-cli`, requirement
//! "公共任务子命令").

use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use parity_scale_codec::Encode;
use sp_runtime::AccountId32;

use ac_primitives::encode_address;
use ac_primitives::market::public::{UnitState, WorkerModels};
use ac_runtime::RuntimeCall;
use ac_wallet::market::{format_usd, parse_model_id, parse_usd};
use ac_wallet::public::{CanaryFile, Publish, job_spec, parse_kind};
use ac_wallet::{NodeClient, amount, parse_address};

use super::NodeArgs;
use super::market_cli::Signed;

#[derive(Debug, clap::Subcommand)]
pub enum PublicCommand {
    /// Worker registration and state.
    Worker {
        #[command(subcommand)]
        command: WorkerCommand,
    },
    /// Print the call that publishes a job, after checking the manifest and the price locally;
    /// the administration multisig proposes it.
    Publish {
        #[command(flatten)]
        node: NodeArgs,
        /// `eval`, `embed` or `clean`.
        #[arg(long)]
        kind: String,
        /// The model (evaluation and embedding), `0x` + 64 hex digits.
        #[arg(long)]
        model: Option<String>,
        /// The data manifest, as it will be served at --manifest-url.
        #[arg(long)]
        manifest: PathBuf,
        /// Where the manifest is served.
        #[arg(long)]
        manifest_url: String,
        /// Where workers upload results (`ac-worker collect`).
        #[arg(long)]
        results_url: String,
        /// Number of units.
        #[arg(long)]
        units: u32,
        /// Price per unit in dollars.
        #[arg(long)]
        price: String,
        /// Canary Merkle root printed by `ac-worker canary`.
        #[arg(long)]
        canary_root: Option<String>,
    },
    /// Print the call that cancels a job; the administration multisig proposes it.
    Cancel {
        /// The job.
        #[arg(long)]
        job: u32,
    },
    /// Jobs in progress and their units' progress.
    Jobs {
        #[command(flatten)]
        node: NodeArgs,
    },
    /// A unit's workers, commitments, reveals and result.
    Unit {
        #[command(flatten)]
        node: NodeArgs,
        /// The job.
        #[arg(long)]
        job: u32,
        /// The unit.
        #[arg(long)]
        unit: u32,
    },
    /// The parameters, the current round and the payout account's balance.
    Params {
        #[command(flatten)]
        node: NodeArgs,
    },
    /// Reveal a unit's canary from an `ac-worker canary` file.
    Canary {
        #[command(flatten)]
        signed: Signed,
        /// The canary file.
        #[arg(long)]
        file: PathBuf,
        /// The unit.
        #[arg(long)]
        unit: u32,
    },
    /// Claim the rewards of settled epochs (default: every settled epoch with pending work).
    Claim {
        #[command(flatten)]
        signed: Signed,
        /// Epochs to claim.
        #[arg(long = "epoch")]
        epochs: Vec<ac_primitives::emission::EpochIndex>,
    },
    /// Withdraw rewards whose lock has ended.
    Withdraw {
        #[command(flatten)]
        signed: Signed,
    },
    /// Pending public work and locked rewards.
    Rewards {
        #[command(flatten)]
        node: NodeArgs,
        /// Worker address; defaults to the wallet's.
        #[arg(long)]
        address: Option<String>,
        /// Wallet file (used when no address is given).
        #[arg(long, value_name = "PATH", default_value = "wallet.json")]
        wallet: PathBuf,
    },
}

#[derive(Debug, clap::Subcommand)]
pub enum WorkerCommand {
    /// Register as a worker that runs these models.
    Register {
        #[command(flatten)]
        signed: Signed,
        /// A model ID (repeat for several; none for data cleaning only).
        #[arg(long = "model")]
        models: Vec<String>,
    },
    /// Replace the registered models.
    Models {
        #[command(flatten)]
        signed: Signed,
        /// A model ID (repeat for several).
        #[arg(long = "model")]
        models: Vec<String>,
    },
    /// Deregister (once no unit is unsettled).
    Deregister {
        #[command(flatten)]
        signed: Signed,
    },
    /// Declare readiness for the current round (`ac-worker run` does it by itself).
    Ready {
        #[command(flatten)]
        signed: Signed,
    },
    /// A worker's record and unsettled units.
    Status {
        #[command(flatten)]
        node: NodeArgs,
        /// Worker address; defaults to the wallet's.
        #[arg(long)]
        address: Option<String>,
        /// Wallet file (used when no address is given).
        #[arg(long, value_name = "PATH", default_value = "wallet.json")]
        wallet: PathBuf,
    },
}

/// Parses `--model` arguments.
///
/// # Errors
///
/// A malformed ID or too many models.
pub fn parse_models(models: &[String]) -> Result<WorkerModels> {
    let ids = models
        .iter()
        .map(|m| parse_model_id(m))
        .collect::<Result<Vec<_>>>()?;
    WorkerModels::try_from(ids).map_err(|_| anyhow::anyhow!("too many models"))
}

fn who(address: Option<String>, wallet: &std::path::Path) -> Result<AccountId32> {
    match address {
        Some(a) => parse_address(&a),
        None => super::load(wallet)?.account(),
    }
}

fn print_call(call: &RuntimeCall) {
    let bytes = call.encode();
    println!("call: 0x{}", hex::encode(&bytes));
    println!("length bound: {}", bytes.len());
}

async fn worker(command: WorkerCommand) -> Result<()> {
    let (signed, call) = match command {
        WorkerCommand::Register { signed, models } => (
            signed,
            pallet_public_jobs::Call::register {
                models: parse_models(&models)?,
            },
        ),
        WorkerCommand::Models { signed, models } => (
            signed,
            pallet_public_jobs::Call::set_models {
                models: parse_models(&models)?,
            },
        ),
        WorkerCommand::Deregister { signed } => (signed, pallet_public_jobs::Call::deregister {}),
        WorkerCommand::Ready { signed } => (signed, pallet_public_jobs::Call::ready {}),
        WorkerCommand::Status {
            node,
            address,
            wallet,
        } => {
            let who = who(address, &wallet)?;
            let client = NodeClient::new(&node.node)?;
            println!("worker: {}", encode_address(who.as_ref()));
            let Some(w) = client.public_worker(&who).await? else {
                println!("status: not registered");
                return Ok(());
            };
            for m in &w.models {
                println!("model: 0x{}", hex::encode(m.0));
            }
            println!(
                "last ready round: {}",
                w.last_ready.map_or("none".into(), |r| r.to_string())
            );
            if let Some(until) = w.suspended_until {
                println!("suspended until block {until}");
            }
            println!(
                "accepted: {}, missed: {} ({} in a row)",
                w.accepted, w.missed, w.misses
            );
            for a in client.public_assigned(&who).await? {
                println!(
                    "unit: job {} unit {} attempt {}: commit by {}, reveal by {}, committed {}, revealed {}",
                    a.job, a.unit, a.attempt, a.commit_by, a.reveal_by, a.committed, a.revealed
                );
            }
            return Ok(());
        }
    };
    super::market_cli::send(&signed, RuntimeCall::PublicJobs(call)).await
}

pub async fn run(command: PublicCommand) -> Result<()> {
    let (signed, call) = match command {
        PublicCommand::Worker { command } => return worker(command).await,
        PublicCommand::Publish {
            node,
            kind,
            model,
            manifest,
            manifest_url,
            results_url,
            units,
            price,
            canary_root,
        } => {
            let client = NodeClient::new(&node.node)?;
            let params = client
                .public_params()
                .await?
                .context("public jobs are not configured on this chain")?;
            let manifest = std::fs::read(&manifest)
                .with_context(|| format!("reading {}", manifest.display()))?;
            let model = model.as_deref().map(parse_model_id).transpose()?;
            if let Some(m) = model
                && client.market_model(m).await?.is_none()
            {
                bail!("model 0x{} is not registered", hex::encode(m.0));
            }
            let canary_root = canary_root
                .map(|r| -> Result<[u8; 32]> {
                    let v = hex::decode(r.trim_start_matches("0x"))?;
                    <[u8; 32]>::try_from(v.as_slice()).context("a canary root is 32 bytes")
                })
                .transpose()?;
            let spec = job_spec(
                Publish {
                    kind: parse_kind(&kind)?,
                    model,
                    manifest,
                    manifest_url,
                    results_url,
                    units,
                    price: parse_usd(&price)?,
                    canary_root,
                },
                params.price_cap,
            )?;
            print_call(&RuntimeCall::PublicJobs(
                pallet_public_jobs::Call::publish {
                    spec: Box::new(spec),
                },
            ));
            return Ok(());
        }
        PublicCommand::Cancel { job } => {
            print_call(&RuntimeCall::PublicJobs(pallet_public_jobs::Call::cancel {
                job,
            }));
            return Ok(());
        }
        PublicCommand::Jobs { node } => {
            let client = NodeClient::new(&node.node)?;
            for id in client.public_jobs().await? {
                let Some(j) = client.public_job(id).await? else {
                    continue;
                };
                println!(
                    "job {id}: {:?}, {} units at {}; opened {}, accepted {}, failed {}{}",
                    j.spec.kind,
                    j.spec.units,
                    format_usd(j.spec.price),
                    j.opened,
                    j.accepted,
                    j.failed,
                    if j.cancelled { ", cancelled" } else { "" }
                );
            }
            return Ok(());
        }
        PublicCommand::Unit { node, job, unit } => {
            let client = NodeClient::new(&node.node)?;
            let Some(u) = client.public_unit(job, unit).await? else {
                bail!("no record of job {job} unit {unit}");
            };
            println!("attempt: {}", u.attempt);
            println!(
                "opened at {}, commit by {}, reveal by {}",
                u.opened_at, u.commit_by, u.reveal_by
            );
            for (i, w) in u.assigned.iter().enumerate() {
                let committed = u.commits.get(i).copied().flatten().is_some();
                let revealed = u.reveals.get(i).cloned().flatten();
                println!(
                    "worker {}: committed {committed}, revealed {}",
                    encode_address(w.as_ref()),
                    revealed.map_or("no".into(), |(s, h)| format!(
                        "summary 0x{} result 0x{}",
                        hex::encode(&s),
                        hex::encode(h)
                    ))
                );
            }
            match u.state {
                UnitState::Accepted {
                    reference,
                    majority,
                } => println!("state: accepted (reference {reference}, majority {majority:?})"),
                other => println!("state: {other:?}"),
            }
            println!("canary revealed: {}", u.canary_revealed);
            return Ok(());
        }
        PublicCommand::Params { node } => {
            let client = NodeClient::new(&node.node)?;
            let Some(p) = client.public_params().await? else {
                bail!("public jobs are not configured on this chain");
            };
            println!("round blocks: {}", p.round_blocks);
            println!("units per round: {}", p.units_per_round);
            println!("commit blocks: {}", p.commit_blocks);
            println!("reveal blocks: {}", p.reveal_blocks);
            println!("challenge epochs: {}", p.challenge_epochs);
            println!("lock blocks: {}", p.lock_blocks);
            println!("suspend blocks: {}", p.suspend_blocks);
            println!("price cap: {}", format_usd(p.price_cap));
            if let Some((round, start, end)) = client.public_round().await? {
                println!("round: {round} (blocks {start}..={end})");
            }
            println!(
                "payout balance: {}",
                amount::format_atc(client.public_pot().await?)
            );
            return Ok(());
        }
        PublicCommand::Canary { signed, file, unit } => {
            let bytes =
                std::fs::read(&file).with_context(|| format!("reading {}", file.display()))?;
            let canaries: CanaryFile = serde_json::from_slice(&bytes)?;
            let canary = canaries
                .get(unit)
                .with_context(|| format!("unit {unit} is not a canary of this file"))?;
            (
                signed,
                pallet_public_jobs::Call::reveal_canary {
                    job: canaries.job,
                    unit,
                    canary: Box::new(canary.reveal()?),
                },
            )
        }
        PublicCommand::Claim { signed, epochs } => {
            let epochs = if epochs.is_empty() {
                let client = NodeClient::new(&signed.node().node)?;
                let me = super::load(&signed.wallet().wallet)?.account()?;
                let mut due = Vec::new();
                for (e, _) in client.public_pending(&me).await? {
                    if client.public_epoch(e).await?.emission.is_some() {
                        due.push(e);
                    }
                }
                if due.is_empty() {
                    bail!("no settled epoch to claim");
                }
                due
            } else {
                epochs
            };
            (
                signed,
                pallet_public_jobs::Call::claim {
                    epochs: frame_support::BoundedVec::try_from(epochs)
                        .map_err(|_| anyhow::anyhow!("at most 16 epochs per claim"))?,
                },
            )
        }
        PublicCommand::Withdraw { signed } => (signed, pallet_public_jobs::Call::withdraw {}),
        PublicCommand::Rewards {
            node,
            address,
            wallet,
        } => {
            let who = who(address, &wallet)?;
            let client = NodeClient::new(&node.node)?;
            println!("worker: {}", encode_address(who.as_ref()));
            for (e, work) in client.public_pending(&who).await? {
                let settled = client.public_epoch(e).await?.emission.is_some();
                println!(
                    "pending: epoch {e} work {work}{}",
                    if settled { " (claimable)" } else { "" }
                );
            }
            for (at, value) in client.public_locked(&who).await? {
                println!("locked: {} until block {at}", amount::format_atc(value));
            }
            return Ok(());
        }
    };
    super::market_cli::send(&signed, RuntimeCall::PublicJobs(call)).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[derive(clap::Parser)]
    struct Cli {
        #[command(subcommand)]
        command: PublicCommand,
    }

    #[test]
    fn arguments_parse() {
        let m = format!("0x{}", "11".repeat(32));
        let cli =
            Cli::try_parse_from(["x", "worker", "register", "--model", &m, "--model", &m]).unwrap();
        let PublicCommand::Worker {
            command: WorkerCommand::Register { models, .. },
        } = cli.command
        else {
            panic!("wrong command");
        };
        assert_eq!(parse_models(&models).unwrap().len(), 2);
        assert!(parse_models(&["0x12".into()]).is_err());
        let cli = Cli::try_parse_from(["x", "claim", "--epoch", "3", "--epoch", "4"]).unwrap();
        assert!(matches!(cli.command, PublicCommand::Claim { epochs, .. } if epochs == vec![3, 4]));
        assert!(Cli::try_parse_from(["x", "publish", "--kind", "clean"]).is_err());
    }
}
