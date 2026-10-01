//! `ac-wallet audit …`: the audit commands (spec `clients/wallet-cli`, requirement "审计子命令").

use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use sp_runtime::AccountId32;

use ac_primitives::encode_address;
use ac_primitives::market::audit::Vote;
use ac_runtime::RuntimeCall;
use ac_wallet::market::format_usd;
use ac_wallet::{NodeClient, amount, audit, parse_address};

use super::NodeArgs;
use super::market_cli::Signed;

#[derive(Debug, clap::Subcommand)]
pub enum AuditCommand {
    /// Register as an auditor and bond a stake (default: the current threshold).
    Register {
        #[command(flatten)]
        signed: Signed,
        /// Stake in ATC.
        #[arg(long)]
        stake: Option<String>,
    },
    /// Add stake.
    Bond {
        #[command(flatten)]
        signed: Signed,
        /// Amount in ATC.
        #[arg(long)]
        amount: String,
    },
    /// Start unbonding part of the stake.
    Unbond {
        #[command(flatten)]
        signed: Signed,
        /// Amount in ATC.
        #[arg(long)]
        amount: String,
    },
    /// Leave: the whole stake starts unbonding.
    Exit {
        #[command(flatten)]
        signed: Signed,
    },
    /// Withdraw unbonded stake that is due.
    Withdraw {
        #[command(flatten)]
        signed: Signed,
    },
    /// Show an auditor's record and counts.
    Status {
        #[command(flatten)]
        node: NodeArgs,
        /// Auditor address; defaults to the wallet's.
        #[arg(long)]
        address: Option<String>,
        /// Wallet file (used when no address is given).
        #[arg(long, value_name = "PATH", default_value = "wallet.json")]
        wallet: PathBuf,
    },
    /// The current round and the providers an auditor is assigned to in it.
    Assignments {
        #[command(flatten)]
        node: NodeArgs,
        /// Auditor address; defaults to the wallet's.
        #[arg(long)]
        address: Option<String>,
        /// Wallet file (used when no address is given).
        #[arg(long, value_name = "PATH", default_value = "wallet.json")]
        wallet: PathBuf,
    },
    /// Submit a verdict from an `ac-auditor recheck` report line and its case file.
    Verdict {
        #[command(flatten)]
        signed: Signed,
        /// A file with the report line (JSON).
        #[arg(long)]
        report: PathBuf,
        /// The case file the report is about.
        #[arg(long)]
        case: PathBuf,
    },
    /// Show a provider's open dispute and its reviewers, and its verdict counts.
    Disputes {
        #[command(flatten)]
        node: NodeArgs,
        /// Provider address.
        #[arg(long)]
        provider: String,
    },
    /// Vote in a provider's open dispute.
    Vote {
        #[command(flatten)]
        signed: Signed,
        /// Provider address.
        #[arg(long)]
        provider: String,
        /// Dispute ID.
        #[arg(long)]
        id: u64,
        /// `confirm` or `reject`.
        #[arg(long)]
        vote: String,
    },
    /// Close a provider's dispute after its deadline (undecided).
    Close {
        #[command(flatten)]
        signed: Signed,
        /// Provider address.
        #[arg(long)]
        provider: String,
        /// Dispute ID.
        #[arg(long)]
        id: u64,
    },
    /// Show the audit pot.
    Pot {
        #[command(flatten)]
        node: NodeArgs,
    },
    /// Show the audit parameters.
    Params {
        #[command(flatten)]
        node: NodeArgs,
    },
}

/// Parses `confirm` or `reject`.
///
/// # Errors
///
/// Anything else.
pub fn parse_vote(text: &str) -> Result<Vote> {
    match text {
        "confirm" => Ok(Vote::Confirm),
        "reject" => Ok(Vote::Reject),
        other => bail!("a vote is `confirm` or `reject`, not `{other}`"),
    }
}

fn who(address: Option<String>, wallet: &std::path::Path) -> Result<AccountId32> {
    match address {
        Some(a) => parse_address(&a),
        None => super::load(wallet)?.account(),
    }
}

fn read_json(path: &std::path::Path) -> Result<serde_json::Value> {
    let text =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    serde_json::from_str(text.trim()).with_context(|| format!("parsing {}", path.display()))
}

pub async fn run(command: AuditCommand) -> Result<()> {
    let (signed, call) = match command {
        AuditCommand::Register { signed, stake } => {
            let stake = match stake {
                Some(s) => amount::parse_atc(&s)?,
                None => NodeClient::new(&signed.node().node)?
                    .audit_threshold()
                    .await?
                    .map_err(|e| anyhow::anyhow!("{e:?}"))?,
            };
            (signed, pallet_audit::Call::register { stake })
        }
        AuditCommand::Bond { signed, amount: a } => (
            signed,
            pallet_audit::Call::bond_extra {
                amount: amount::parse_atc(&a)?,
            },
        ),
        AuditCommand::Unbond { signed, amount: a } => (
            signed,
            pallet_audit::Call::unbond {
                amount: amount::parse_atc(&a)?,
            },
        ),
        AuditCommand::Exit { signed } => (signed, pallet_audit::Call::exit {}),
        AuditCommand::Withdraw { signed } => (signed, pallet_audit::Call::withdraw_unbonded {}),
        AuditCommand::Verdict {
            signed,
            report,
            case,
        } => {
            let client = NodeClient::new(&signed.node().node)?;
            let (round, _, _) = client
                .audit_round()
                .await?
                .context("audits are not configured on this chain")?;
            let verdict = audit::verdict_from(&read_json(&report)?, &read_json(&case)?, round)?;
            // Checked before anything is signed (spec "提交前发现未被分配").
            let me = super::load(&signed.wallet().wallet)?.account()?;
            let assigned = client.audit_assignment(round, &verdict.provider).await?;
            if !assigned.contains(&me) {
                bail!(
                    "{} is not assigned to provider {} in round {round}",
                    encode_address(me.as_ref()),
                    encode_address(verdict.provider.as_ref())
                );
            }
            if let Some(c) = verdict.evidence {
                println!("evidence commitment: {}", hex::encode(c));
            }
            (
                signed,
                pallet_audit::Call::submit_verdict {
                    verdict: Box::new(verdict),
                },
            )
        }
        AuditCommand::Vote {
            signed,
            provider,
            id,
            vote,
        } => (
            signed,
            pallet_audit::Call::vote {
                provider: parse_address(&provider)?,
                id,
                vote: parse_vote(&vote)?,
            },
        ),
        AuditCommand::Close {
            signed,
            provider,
            id,
        } => (
            signed,
            pallet_audit::Call::close_dispute {
                provider: parse_address(&provider)?,
                id,
            },
        ),
        AuditCommand::Status {
            node,
            address,
            wallet,
        } => {
            let who = who(address, &wallet)?;
            let client = NodeClient::new(&node.node)?;
            println!("auditor: {}", encode_address(who.as_ref()));
            match client.audit_auditor(&who).await? {
                Some(a) => {
                    println!("status: {:?}", a.status);
                    println!("stake: {}", amount::format_atc(a.stake));
                    for c in &a.unlocking {
                        println!(
                            "unbonding: {} at block {}",
                            amount::format_atc(c.amount),
                            c.unlock_at
                        );
                    }
                }
                None => println!("status: not registered"),
            }
            let s = client.audit_auditor_stats(&who).await?;
            println!(
                "verdicts: {}, votes: {}, missed votes: {}",
                s.verdicts, s.votes, s.missed_votes
            );
            return Ok(());
        }
        AuditCommand::Assignments {
            node,
            address,
            wallet,
        } => {
            let who = who(address, &wallet)?;
            let client = NodeClient::new(&node.node)?;
            let (round, start, next) = client
                .audit_round()
                .await?
                .context("audits are not configured on this chain")?;
            println!("round: {round} (blocks {start}..{next})");
            for (p, done) in client.audit_assigned_to(round, &who).await? {
                let state = if done { "submitted" } else { "pending" };
                println!("provider: {} {state}", encode_address(p.as_ref()));
            }
            return Ok(());
        }
        AuditCommand::Disputes { node, provider } => {
            let provider = parse_address(&provider)?;
            let client = NodeClient::new(&node.node)?;
            let s = client.audit_provider_stats(&provider).await?;
            println!(
                "verdicts: {} pass, {} fail, {} inconclusive; {} confirmed disputes",
                s.pass, s.fail, s.inconclusive, s.confirmed
            );
            match client.audit_open_dispute(&provider).await? {
                None => println!("open dispute: none"),
                Some(id) => {
                    println!("open dispute: {id}");
                    if let Some(d) = client.audit_dispute(id).await? {
                        println!("deadline: block {}", d.deadline);
                        for a in &d.accusers {
                            println!(
                                "accuser: {} (round {})",
                                encode_address(a.auditor.as_ref()),
                                a.round
                            );
                        }
                        for (r, v) in &d.reviewers {
                            let v = v.map_or("none".to_owned(), |v| format!("{v:?}"));
                            println!("reviewer: {} vote {v}", encode_address(r.as_ref()));
                        }
                    }
                }
            }
            return Ok(());
        }
        AuditCommand::Pot { node } => {
            let (pot, balance) = NodeClient::new(&node.node)?.audit_pot().await?;
            println!("pot: {}", encode_address(pot.as_ref()));
            println!("balance: {}", amount::format_atc(balance));
            return Ok(());
        }
        AuditCommand::Params { node } => {
            let Some((p, a)) = NodeClient::new(&node.node)?.audit_params().await? else {
                bail!("audits are not configured on this chain");
            };
            println!("round blocks: {}", p.round_blocks);
            println!("auditors per provider: {}", p.assign);
            println!("reviewers: {} (quorum {})", p.reviewers, p.quorum);
            println!("vote blocks: {}", p.vote_blocks);
            println!(
                "slash: provider {:?}, auditor {:?}",
                p.provider_slash, p.auditor_slash
            );
            println!("auditor stake: {}", format_usd(a.stake_usd));
            println!("payment: {}", format_usd(a.payment_usd));
            println!("thresholds version: {}", a.thresholds_version);
            return Ok(());
        }
    };
    super::market_cli::send(&signed, RuntimeCall::Audit(call)).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn votes_parse() {
        assert_eq!(parse_vote("confirm").unwrap(), Vote::Confirm);
        assert_eq!(parse_vote("reject").unwrap(), Vote::Reject);
        assert!(parse_vote("yes").is_err());
    }
}
