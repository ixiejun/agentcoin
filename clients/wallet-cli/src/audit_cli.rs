//! `ac-wallet audit …`: the audit commands (spec `clients/wallet-cli`, requirement "审计子命令").

use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use sp_runtime::AccountId32;

use ac_primitives::encode_address;
use ac_primitives::market::audit::{AuditProfile, Vote};
use ac_runtime::RuntimeCall;
use ac_wallet::market::{format_usd, parse_model_id};
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
    /// Show an auditor's record, counts (spot checks, overturned verdicts, unavailable evidence)
    /// and slashing locks.
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
    /// Show a provider's verdicts in a round (default: the current one) with their statistics
    /// and what judged each (a dispute or a spot check), or whether it is drawn for a spot check.
    Verdicts {
        #[command(flatten)]
        node: NodeArgs,
        /// Provider address.
        #[arg(long)]
        provider: String,
        /// Round.
        #[arg(long)]
        round: Option<u32>,
    },
    /// Show a provider's open dispute (kind, accusers, reviewers), its verdict counts and its
    /// statistical state.
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
        /// Positions (from 0) of the dispute's cited verdicts whose evidence you did not
        /// obtain, comma-separated.
        #[arg(long, value_delimiter = ',')]
        unavailable: Vec<u32>,
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
    /// Show the open spot checks, or one spot check (open or closed).
    SpotChecks {
        #[command(flatten)]
        node: NodeArgs,
        /// A spot check ID.
        #[arg(long)]
        id: Option<u64>,
    },
    /// Vote in an open spot check.
    SpotVote {
        #[command(flatten)]
        signed: Signed,
        /// Spot check ID.
        #[arg(long)]
        id: u64,
        /// `uphold` or `overturn`.
        #[arg(long)]
        vote: String,
        /// The verdict's evidence was not obtained (not served, not opening against the
        /// commitment, or not the verdict's receipt).
        #[arg(long)]
        not_obtained: bool,
    },
    /// Close a spot check after its deadline (undecided).
    CloseSpotCheck {
        #[command(flatten)]
        signed: Signed,
        /// Spot check ID.
        #[arg(long)]
        id: u64,
    },
    /// Show a model's audit profile: the versions verdicts on it use, or none (uncalibrated).
    Profile {
        #[command(flatten)]
        node: NodeArgs,
        /// Model ID (`0x` + 64 hex digits).
        #[arg(long)]
        model: String,
    },
    /// Print the call that sets a model's audit profile, after checking it locally; the
    /// administration multisig proposes it.
    SetProfile {
        #[command(flatten)]
        node: NodeArgs,
        /// Model ID (`0x` + 64 hex digits).
        #[arg(long)]
        model: String,
        /// Thresholds version.
        #[arg(long)]
        thresholds: u16,
        /// Statistical parameter version.
        #[arg(long)]
        stats: u16,
    },
    /// Set or clear the endpoint where this auditor serves the evidence of its failing
    /// verdicts to reviewers (`ac-auditor run` sets it by itself).
    Endpoint {
        #[command(flatten)]
        signed: Signed,
        /// The endpoint URL (with --kem-key).
        #[arg(long, requires = "kem_key", conflicts_with = "clear")]
        set: Option<String>,
        /// The X-Wing public key reviewers encrypt to (hex, as `ac-auditor keygen` prints).
        #[arg(long, requires = "set")]
        kem_key: Option<String>,
        /// Remove the endpoint.
        #[arg(long)]
        clear: bool,
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

/// The marks of a dispute ballot from the positions of the verdicts whose evidence was not
/// obtained.
///
/// # Errors
///
/// A position of 128 or more.
pub fn unavailable_mask(positions: &[u32]) -> Result<u128> {
    positions.iter().try_fold(0u128, |mask, &i| {
        let bit = 1u128
            .checked_shl(i)
            .with_context(|| format!("a dispute cites at most 128 verdicts, not position {i}"))?;
        Ok(mask | bit)
    })
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

/// The `set_endpoint` argument of `audit endpoint`.
///
/// # Errors
///
/// Neither `--set` nor `--clear`, a bad key or an overlong endpoint.
pub fn endpoint_arg(
    set: Option<String>,
    kem_key: Option<String>,
    clear: bool,
) -> Result<Option<pallet_audit::AuditorEndpoint>> {
    match (set, kem_key, clear) {
        (None, None, true) => Ok(None),
        (Some(url), Some(key), false) => {
            let url = frame_support::BoundedVec::try_from(url.into_bytes())
                .map_err(|_| anyhow::anyhow!("the endpoint is too long"))?;
            Ok(Some((url, ac_wallet::market::parse_kem_key(&key)?)))
        }
        _ => bail!("give --set URL --kem-key KEY, or --clear"),
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
        AuditCommand::Endpoint {
            signed,
            set,
            kem_key,
            clear,
        } => (
            signed,
            pallet_audit::Call::set_endpoint {
                endpoint: endpoint_arg(set, kem_key, clear)?,
            },
        ),
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
            // Checked before anything is signed (spec "提交前发现未被分配", "未校准模型在本地被
            // 拦下").
            let me = super::load(&signed.wallet().wallet)?.account()?;
            let assigned = client.audit_assignment(round, &verdict.provider).await?;
            let profile = client
                .audit_profile(&verdict.receipt.body.model)
                .await?
                .map(|(p, _)| p);
            audit::check_verdict(&verdict, &me, &assigned, profile.as_ref())?;
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
            unavailable,
        } => (
            signed,
            pallet_audit::Call::vote_dispute {
                provider: parse_address(&provider)?,
                id,
                vote: parse_vote(&vote)?,
                unavailable: unavailable_mask(&unavailable)?,
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
        AuditCommand::SpotVote {
            signed,
            id,
            vote,
            not_obtained,
        } => (
            signed,
            pallet_audit::Call::vote_review {
                id,
                vote: audit::parse_review_vote(&vote)?,
                obtained: !not_obtained,
            },
        ),
        AuditCommand::CloseSpotCheck { signed, id } => {
            (signed, pallet_audit::Call::close_spot_check { id })
        }
        AuditCommand::SpotChecks { node, id } => {
            let client = NodeClient::new(&node.node)?;
            let ids = match id {
                Some(id) => vec![id],
                None => client.audit_open_reviews(None, 256).await?,
            };
            if ids.is_empty() {
                println!("open spot checks: none");
            }
            for id in ids {
                match client.audit_review(id).await? {
                    Some(r) => audit::format_review(id, &r)
                        .iter()
                        .for_each(|l| println!("{l}")),
                    None => println!("spot check {id}: unknown"),
                }
            }
            return Ok(());
        }
        AuditCommand::Profile { node, model } => {
            let model = parse_model_id(&model)?;
            let client = NodeClient::new(&node.node)?;
            println!("model: 0x{}", hex::encode(model.0));
            println!(
                "{}",
                audit::format_profile(client.audit_profile(&model).await?.as_ref())
            );
            return Ok(());
        }
        AuditCommand::SetProfile {
            node,
            model,
            thresholds,
            stats,
        } => {
            let model = parse_model_id(&model)?;
            let client = NodeClient::new(&node.node)?;
            let quant = client
                .market_model(model)
                .await?
                .with_context(|| format!("model 0x{} is not registered", hex::encode(model.0)))?
                .manifest
                .quant;
            let profile = AuditProfile {
                thresholds_version: thresholds,
                stats_version: stats,
            };
            // The chain's own rules, checked before the multisig spends a motion on it.
            let current = client.audit_profile(&model).await?.map(|(p, _)| p);
            profile
                .check_update(quant, current.as_ref())
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            super::public_cli::print_call(&RuntimeCall::Audit(
                pallet_audit::Call::set_audit_profile { model, profile },
            ));
            return Ok(());
        }
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
            match client.audit_endpoint(&who).await? {
                Some((url, _)) => println!("evidence endpoint: {}", String::from_utf8_lossy(&url)),
                None => println!("evidence endpoint: none"),
            }
            let s = client.audit_auditor_stats(&who).await?;
            println!(
                "verdicts: {}, votes: {}, missed votes: {}",
                s.verdicts, s.votes, s.missed_votes
            );
            let r = client.audit_review_stats(&who).await?;
            println!(
                "spot-checked: {}, overturned: {}, evidence unavailable: {}",
                r.spot_checked, r.overturned, r.unavailable
            );
            let locks = client.audit_locks(&who).await?;
            println!("slashing locks (open disputes and spot checks citing its verdicts): {locks}");
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
        AuditCommand::Verdicts {
            node,
            provider,
            round,
        } => {
            let provider = parse_address(&provider)?;
            let client = NodeClient::new(&node.node)?;
            let round = match round {
                Some(r) => r,
                None => {
                    client
                        .audit_round()
                        .await?
                        .context("audits are not configured on this chain")?
                        .0
                }
            };
            println!("round: {round}");
            let next_seed = audit::seed(&client, round.saturating_add(1)).await?;
            let rate = client
                .audit_params()
                .await?
                .map_or(0, |(_, a)| a.spot_check_bps);
            let judged = client.audit_judged(round, &provider).await?;
            for v in client.audit_verdicts(round, &provider).await? {
                let status = audit::verdict_status(&v, &judged, next_seed.as_ref(), rate);
                println!(
                    "verdict: {} {:?} (thresholds v{}; {status})",
                    encode_address(v.auditor.as_ref()),
                    v.outcome,
                    v.thresholds_version
                );
                if let Some(s) = &v.stats {
                    println!("  {}", audit::format_stats(s));
                }
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
            let states = client.audit_sprt_states(&provider).await?;
            if states.is_empty() {
                println!("statistical state: none");
            }
            for (model, state) in &states {
                let params = client
                    .audit_profile(model)
                    .await?
                    .and_then(|(p, _)| ac_primitives::market::audit::stats_params(p.stats_version));
                println!(
                    "model {}: {}",
                    hex::encode(model.0),
                    audit::format_state(state, params)
                );
            }
            match client.audit_open_dispute(&provider).await? {
                None => println!("open dispute: none"),
                Some(id) => {
                    println!("open dispute: {id}");
                    if let Some(d) = client.audit_dispute(id).await? {
                        println!("kind: {}", audit::format_kind(&d.kind));
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
            println!(
                "spot checks: {}.{:02}% of the verdicts judged by the thresholds",
                a.spot_check_bps / 100,
                a.spot_check_bps % 100
            );
            let client = NodeClient::new(&node.node)?;
            let enabled = client.audit_stats_enabled().await?;
            println!(
                "statistical judgment: {}",
                if enabled { "enabled" } else { "disabled" }
            );
            {
                let c = ac_primitives::market::audit::CURRENT_STATS;
                println!("statistical parameters built in: v{}", c.version);
                if let Some(sp) = ac_primitives::market::audit::stats_params(c.version) {
                    println!(
                        "statistical bound: {} nats, per verdict {} to {}, per auditor {}",
                        audit::format_nats(sp.bound),
                        audit::format_nats(i64::from(sp.clamp.0)),
                        audit::format_nats(i64::from(sp.clamp.1)),
                        audit::format_nats(sp.per_auditor_cap)
                    );
                }
            }
            return Ok(());
        }
    };
    super::market_cli::send(&signed, RuntimeCall::Audit(call)).await
}

#[cfg(test)]
mod tests {
    use super::*;

    // Spec clients/wallet-cli (m6-auditor-agent 4.5): `audit endpoint` takes a URL and a key, or
    // --clear.
    #[test]
    fn endpoint_arguments() {
        let key = ac_crypto::KemPublicKey::new(ac_crypto::KemAlg::XWing, &[7; 1216]).unwrap();
        let hex_key = hex::encode(key.to_canonical());
        let (url, k) = endpoint_arg(Some("http://a:1".into()), Some(hex_key.clone()), false)
            .unwrap()
            .unwrap();
        assert_eq!((&url[..], k), (&b"http://a:1"[..], key));
        assert!(endpoint_arg(None, None, true).unwrap().is_none());
        assert!(endpoint_arg(None, None, false).is_err());
        assert!(endpoint_arg(Some("u".into()), Some("zz".into()), false).is_err());
        assert!(endpoint_arg(Some("x".repeat(10_000)), Some(hex_key), false).is_err());
    }

    #[test]
    fn votes_parse() {
        assert_eq!(parse_vote("confirm").unwrap(), Vote::Confirm);
        assert_eq!(parse_vote("reject").unwrap(), Vote::Reject);
        assert!(parse_vote("yes").is_err());
    }
}
