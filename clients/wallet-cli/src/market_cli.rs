//! `ac-wallet market …`: the inference-market commands (spec `clients/wallet-cli`, requirement
//! "市场子命令").

use anyhow::{Context, Result, bail};
use frame_support::BoundedVec;
use parity_scale_codec::Encode;
use sp_runtime::AccountId32;

use ac_primitives::encode_address;
use ac_primitives::market::records::{ModelPrice, Tier};
use ac_runtime::RuntimeCall;
use ac_wallet::market::{self, Channel, Gateway, Provider, format_usd, parse_usd};
use ac_wallet::{NodeClient, amount, ops, parse_address};

use super::{NodeArgs, WalletArgs, load, password};

#[derive(Debug, clap::Subcommand)]
pub enum MarketCommand {
    /// Models: register from a manifest file, or show one.
    Model {
        #[command(subcommand)]
        command: ModelCommand,
    },
    /// Providers: register, update, heartbeat, stake, exit, show.
    Provider {
        #[command(subcommand)]
        command: ProviderCommand,
    },
    /// Serviceable providers of a model.
    Providers {
        #[command(flatten)]
        node: NodeArgs,
        /// Model ID.
        #[arg(long)]
        model: String,
    },
    /// Gateways: register, update, stake, exit, show.
    Gateway {
        #[command(subcommand)]
        command: GatewayCommand,
    },
    /// Escrow with a gateway: deposit, request a withdrawal, withdraw, change the voucher key.
    Escrow {
        #[command(subcommand)]
        command: EscrowCommand,
    },
    /// Show a credit channel.
    Channel {
        #[command(flatten)]
        node: NodeArgs,
        /// Gateway address.
        #[arg(long)]
        gateway: String,
        /// User address; defaults to the wallet's.
        #[arg(long)]
        address: Option<String>,
        /// Wallet file (used when no address is given).
        #[arg(long, value_name = "PATH", default_value = "wallet.json")]
        wallet: std::path::PathBuf,
    },
    /// Vouchers: sign one, or check one against the chain.
    Voucher {
        #[command(subcommand)]
        command: VoucherCommand,
    },
    /// Show the reference rate.
    Rate {
        #[command(flatten)]
        node: NodeArgs,
    },
}

#[derive(Debug, clap::Subcommand)]
pub enum ModelCommand {
    /// Register a model from a manifest file; prints the model ID before submitting.
    Register {
        #[command(flatten)]
        wallet: WalletArgs,
        #[command(flatten)]
        node: NodeArgs,
        /// Manifest JSON: name, arch, quant, shards, optional lineage and licenseTag.
        #[arg(long, value_name = "PATH")]
        file: std::path::PathBuf,
    },
    /// Show a model.
    Show {
        #[command(flatten)]
        node: NodeArgs,
        /// Model ID.
        #[arg(long)]
        id: String,
    },
    /// Compute the model ID of a manifest file without a node.
    Id {
        /// Manifest JSON.
        #[arg(long, value_name = "PATH")]
        file: std::path::PathBuf,
    },
}

#[derive(Debug, clap::Args)]
pub struct Signed {
    #[command(flatten)]
    wallet: WalletArgs,
    #[command(flatten)]
    node: NodeArgs,
}

#[derive(Debug, clap::Subcommand)]
pub enum ProviderCommand {
    /// Register as a provider.
    Register {
        #[command(flatten)]
        signed: Signed,
        /// `t1` or `t2`.
        #[arg(long)]
        tier: String,
        /// Endpoint gateways connect to.
        #[arg(long)]
        endpoint: String,
        /// X-Wing encryption key (hex of its AlgId-tagged encoding).
        #[arg(long)]
        kem_key: String,
        /// `MODEL_ID:INPUT_USD:OUTPUT_USD` per million tokens; repeat for several models.
        #[arg(long = "model", required = true)]
        models: Vec<String>,
        /// Stake in ATC; defaults to the tier's current threshold.
        #[arg(long)]
        stake: Option<String>,
    },
    /// Change the endpoint, encryption key or models.
    Update {
        #[command(flatten)]
        signed: Signed,
        #[arg(long)]
        endpoint: Option<String>,
        #[arg(long)]
        kem_key: Option<String>,
        /// Replaces the whole model list.
        #[arg(long = "model")]
        models: Vec<String>,
    },
    /// Send a heartbeat.
    Heartbeat {
        #[command(flatten)]
        signed: Signed,
    },
    /// Add stake (ATC).
    Bond {
        #[command(flatten)]
        signed: Signed,
        #[arg(long)]
        amount: String,
    },
    /// Start unbonding stake (ATC).
    Unbond {
        #[command(flatten)]
        signed: Signed,
        #[arg(long)]
        amount: String,
    },
    /// Stop serving; the whole stake starts unbonding.
    Exit {
        #[command(flatten)]
        signed: Signed,
    },
    /// Withdraw unbonded stake that is due.
    Withdraw {
        #[command(flatten)]
        signed: Signed,
    },
    /// Show a provider.
    Show {
        #[command(flatten)]
        node: NodeArgs,
        /// Provider address; defaults to the wallet's.
        #[arg(long)]
        address: Option<String>,
        #[arg(long, value_name = "PATH", default_value = "wallet.json")]
        wallet: std::path::PathBuf,
    },
}

#[derive(Debug, clap::Subcommand)]
pub enum GatewayCommand {
    /// Register as a gateway.
    Register {
        #[command(flatten)]
        signed: Signed,
        /// Endpoint users connect to.
        #[arg(long)]
        endpoint: String,
        /// Fee in basis points (at most 500).
        #[arg(long)]
        fee_bps: u16,
        /// Stake in ATC; defaults to the current threshold.
        #[arg(long)]
        stake: Option<String>,
    },
    /// Change the endpoint or fee.
    Update {
        #[command(flatten)]
        signed: Signed,
        #[arg(long)]
        endpoint: Option<String>,
        #[arg(long)]
        fee_bps: Option<u16>,
    },
    /// Add stake (ATC).
    Bond {
        #[command(flatten)]
        signed: Signed,
        #[arg(long)]
        amount: String,
    },
    /// Start unbonding stake (ATC).
    Unbond {
        #[command(flatten)]
        signed: Signed,
        #[arg(long)]
        amount: String,
    },
    /// Stop accepting escrow; the whole stake starts unbonding.
    Exit {
        #[command(flatten)]
        signed: Signed,
    },
    /// Withdraw unbonded stake that is due.
    Withdraw {
        #[command(flatten)]
        signed: Signed,
    },
    /// Show a gateway.
    Show {
        #[command(flatten)]
        node: NodeArgs,
        /// Gateway address; defaults to the wallet's.
        #[arg(long)]
        address: Option<String>,
        #[arg(long, value_name = "PATH", default_value = "wallet.json")]
        wallet: std::path::PathBuf,
    },
}

#[derive(Debug, clap::Subcommand)]
pub enum EscrowCommand {
    /// Escrow ATC with a gateway.
    Deposit {
        #[command(flatten)]
        signed: Signed,
        #[arg(long)]
        gateway: String,
        #[arg(long)]
        amount: String,
    },
    /// Ask to withdraw ATC after the delay.
    RequestWithdrawal {
        #[command(flatten)]
        signed: Signed,
        #[arg(long)]
        gateway: String,
        #[arg(long)]
        amount: String,
    },
    /// Withdraw once the delay passed.
    Withdraw {
        #[command(flatten)]
        signed: Signed,
        #[arg(long)]
        gateway: String,
    },
    /// Make the wallet's current key the channel's voucher key after the delay.
    ChangeKey {
        #[command(flatten)]
        signed: Signed,
        #[arg(long)]
        gateway: String,
    },
}

#[derive(Debug, clap::Subcommand)]
pub enum VoucherCommand {
    /// Sign a voucher authorizing `usd` dollars in total for a gateway; prints its hex.
    Sign {
        #[command(flatten)]
        signed: Signed,
        #[arg(long)]
        gateway: String,
        /// Cumulative dollars, e.g. `0.5`.
        #[arg(long)]
        usd: String,
    },
    /// Check a voucher against the chain as redemption would.
    Check {
        #[command(flatten)]
        node: NodeArgs,
        /// Voucher hex from `voucher sign`.
        #[arg(long)]
        voucher: String,
    },
}

fn bounded<const N: u32>(
    text: &str,
    what: &str,
) -> Result<BoundedVec<u8, frame_support::traits::ConstU32<N>>> {
    BoundedVec::try_from(text.as_bytes().to_vec())
        .map_err(|_| anyhow::anyhow!("{what} is longer than {N} bytes"))
}

fn models(
    entries: &[String],
) -> Result<BoundedVec<ModelPrice, frame_support::traits::ConstU32<16>>> {
    let list = entries
        .iter()
        .map(|e| market::parse_model_price(e))
        .collect::<Result<Vec<_>>>()?;
    BoundedVec::try_from(list).map_err(|_| anyhow::anyhow!("at most 16 models"))
}

async fn send(signed: &Signed, call: RuntimeCall) -> Result<()> {
    let w = load(&signed.wallet.wallet)?;
    let pw = password(&signed.wallet, false)?;
    let client = NodeClient::new(&signed.node.node)?;
    let inclusion = ops::submit(&client, &w, &pw, call).await?;
    println!("included in block {:?}", inclusion.block_hash);
    Ok(())
}

fn who(address: Option<String>, wallet: &std::path::Path) -> Result<AccountId32> {
    match address {
        Some(a) => parse_address(&a),
        None => load(wallet)?.account(),
    }
}

fn print_provider(who: &AccountId32, p: &Provider, serviceable: bool) {
    println!("provider: {}", encode_address(who.as_ref()));
    println!("tier: {:?}", p.tier);
    println!("status: {:?}", p.status);
    println!("serviceable: {serviceable}");
    println!("endpoint: {}", String::from_utf8_lossy(&p.endpoint));
    println!("stake: {}", amount::format_atc(p.stake));
    for c in &p.unlocking {
        println!(
            "unbonding: {} at block {}",
            amount::format_atc(c.amount),
            c.unlock_at
        );
    }
    println!("last heartbeat: block {}", p.last_heartbeat);
    for m in &p.models {
        println!(
            "model: {:?} input {} output {} per million tokens",
            m.model,
            format_usd(m.price.input),
            format_usd(m.price.output)
        );
    }
}

fn print_gateway(who: &AccountId32, g: &Gateway) {
    println!("gateway: {}", encode_address(who.as_ref()));
    println!("status: {:?}", g.status);
    println!("endpoint: {}", String::from_utf8_lossy(&g.endpoint));
    println!("fee: {} bps", g.fee_bps);
    println!("stake: {}", amount::format_atc(g.stake));
    for c in &g.unlocking {
        println!(
            "unbonding: {} at block {}",
            amount::format_atc(c.amount),
            c.unlock_at
        );
    }
}

fn print_channel(c: &Channel) {
    println!("escrow: {}", amount::format_atc(c.escrow));
    println!("channel: {}", c.number);
    println!("redeemed: {}", format_usd(c.redeemed));
    println!("voucher key: 0x{}", hex::encode(c.key));
    if let Some((a, at)) = c.pending_withdrawal {
        println!(
            "pending withdrawal: {} from block {at}",
            amount::format_atc(a)
        );
    }
    if let Some((k, at)) = c.pending_key {
        println!("pending voucher key: 0x{} from block {at}", hex::encode(k));
    }
}

/// Runs one `market` command.
pub async fn run(command: MarketCommand) -> Result<()> {
    match command {
        MarketCommand::Model { command } => match command {
            ModelCommand::Register { wallet, node, file } => {
                let text = std::fs::read_to_string(&file)
                    .with_context(|| format!("cannot read {}", file.display()))?;
                let model = market::parse_manifest(&text)?;
                let id = model.id()?;
                println!("model id: {id:?}");
                let call = RuntimeCall::ModelRegistry(pallet_model_registry::Call::register {
                    manifest: model.manifest,
                    lineage: model.lineage,
                    license_tag: BoundedVec::try_from(model.license_tag)
                        .map_err(|_| anyhow::anyhow!("licence tag too long"))?,
                    royalty: None,
                });
                send(&Signed { wallet, node }, call).await?;
            }
            ModelCommand::Show { node, id } => {
                let id = market::parse_model_id(&id)?;
                let client = NodeClient::new(&node.node)?;
                let Some(m) = client.market_model(id).await? else {
                    bail!("model {id:?} is not registered");
                };
                println!("model id: {id:?}");
                println!("name: {}", String::from_utf8_lossy(&m.manifest.name));
                println!("arch: {}", String::from_utf8_lossy(&m.manifest.arch));
                println!("quant: {:?}", m.manifest.quant);
                println!("shards: {}", m.manifest.shards.len());
                if let Some(l) = m.lineage {
                    println!("lineage: {:?} of {:?}", l.kind, l.parent);
                }
                println!("licence tag: {}", String::from_utf8_lossy(&m.license_tag));
                println!("owner: {}", encode_address(m.owner.as_ref()));
                println!("deposit: {}", amount::format_atc(m.deposit));
            }
            ModelCommand::Id { file } => {
                let text = std::fs::read_to_string(&file)
                    .with_context(|| format!("cannot read {}", file.display()))?;
                println!("{:?}", market::parse_manifest(&text)?.id()?);
            }
        },
        MarketCommand::Provider { command } => run_provider(command).await?,
        MarketCommand::Providers { node, model } => {
            let model = market::parse_model_id(&model)?;
            let client = NodeClient::new(&node.node)?;
            let list = client.market_serviceable(model).await?;
            println!("serviceable providers: {}", list.len());
            for (who, p) in list {
                print_provider(&who, &p, true);
            }
        }
        MarketCommand::Gateway { command } => run_gateway(command).await?,
        MarketCommand::Escrow { command } => {
            let (signed, call) = match command {
                EscrowCommand::Deposit {
                    signed,
                    gateway,
                    amount: a,
                } => {
                    let call = pallet_credits::Call::deposit {
                        gateway: parse_address(&gateway)?,
                        amount: amount::parse_atc(&a)?,
                    };
                    (signed, call)
                }
                EscrowCommand::RequestWithdrawal {
                    signed,
                    gateway,
                    amount: a,
                } => {
                    let call = pallet_credits::Call::request_withdrawal {
                        gateway: parse_address(&gateway)?,
                        amount: amount::parse_atc(&a)?,
                    };
                    (signed, call)
                }
                EscrowCommand::Withdraw { signed, gateway } => (
                    signed,
                    pallet_credits::Call::withdraw {
                        gateway: parse_address(&gateway)?,
                    },
                ),
                EscrowCommand::ChangeKey { signed, gateway } => (
                    signed,
                    pallet_credits::Call::request_key_change {
                        gateway: parse_address(&gateway)?,
                    },
                ),
            };
            send(&signed, RuntimeCall::Credits(call)).await?;
        }
        MarketCommand::Channel {
            node,
            gateway,
            address,
            wallet,
        } => {
            let user = who(address, &wallet)?;
            let client = NodeClient::new(&node.node)?;
            match client
                .market_channel(&user, &parse_address(&gateway)?)
                .await?
            {
                Some(c) => print_channel(&c),
                None => println!("no channel"),
            }
        }
        MarketCommand::Voucher { command } => match command {
            VoucherCommand::Sign {
                signed,
                gateway,
                usd,
            } => {
                // Parse before touching the wallet or the node: a bad amount signs nothing.
                let cumulative = parse_usd(&usd)?;
                let gateway = parse_address(&gateway)?;
                let w = load(&signed.wallet.wallet)?;
                let pw = password(&signed.wallet, false)?;
                let client = NodeClient::new(&signed.node.node)?;
                let v = market::sign_voucher(&client, &w, &pw, &gateway, cumulative).await?;
                println!("channel: {}", v.body.channel);
                println!("cumulative: {}", format_usd(v.body.cumulative));
                println!("voucher: 0x{}", hex::encode(v.encode()));
            }
            VoucherCommand::Check { node, voucher } => {
                let v = market::decode_voucher(&voucher)?;
                let client = NodeClient::new(&node.node)?;
                match client.market_check_voucher(&v).await? {
                    Ok(check) => {
                        println!("valid: yes");
                        println!(
                            "increment: {} ({} micro-dollars)",
                            format_usd(check.increment),
                            check.increment.0
                        );
                        println!(
                            "increment in ATC: {}",
                            amount::format_atc(check.increment_atc)
                        );
                        println!("covered by the escrow: {}", check.covered);
                    }
                    Err(e) => {
                        println!("valid: no");
                        println!("reason: {e}");
                    }
                }
            }
        },
        MarketCommand::Rate { node } => {
            let client = NodeClient::new(&node.node)?;
            match client.market_rate().await? {
                Some((rate, at)) => {
                    println!("rate: {} per USD", amount::format_atc(rate.0));
                    println!("set at block: {at}");
                }
                None => println!("rate: not set"),
            }
        }
    }
    Ok(())
}

async fn default_stake(node: &NodeArgs, tier: Option<Tier>) -> Result<u128> {
    let client = NodeClient::new(&node.node)?;
    let threshold = match tier {
        Some(t) => client.market_provider_threshold(t).await?,
        None => client.market_gateway_threshold().await?,
    };
    threshold.map_err(|e| anyhow::anyhow!("cannot compute the stake threshold: {e}"))
}

async fn run_provider(command: ProviderCommand) -> Result<()> {
    let (signed, call) = match command {
        ProviderCommand::Register {
            signed,
            tier,
            endpoint,
            kem_key,
            models: entries,
            stake,
        } => {
            let tier = market::parse_tier(&tier)?;
            let registration = pallet_providers::Registration {
                tier,
                endpoint: bounded(&endpoint, "the endpoint")?,
                kem_pk: market::parse_kem_key(&kem_key)?,
                models: models(&entries)?,
                stake: match stake {
                    Some(s) => amount::parse_atc(&s)?,
                    None => default_stake(&signed.node, Some(tier)).await?,
                },
                attestation: None,
            };
            (signed, pallet_providers::Call::register { registration })
        }
        ProviderCommand::Update {
            signed,
            endpoint,
            kem_key,
            models: entries,
        } => {
            let call = pallet_providers::Call::update {
                endpoint: endpoint.map(|e| bounded(&e, "the endpoint")).transpose()?,
                kem_pk: kem_key.map(|k| market::parse_kem_key(&k)).transpose()?,
                models: if entries.is_empty() {
                    None
                } else {
                    Some(models(&entries)?)
                },
            };
            (signed, call)
        }
        ProviderCommand::Heartbeat { signed } => (signed, pallet_providers::Call::heartbeat {}),
        ProviderCommand::Bond { signed, amount: a } => (
            signed,
            pallet_providers::Call::bond_extra {
                amount: amount::parse_atc(&a)?,
            },
        ),
        ProviderCommand::Unbond { signed, amount: a } => (
            signed,
            pallet_providers::Call::unbond {
                amount: amount::parse_atc(&a)?,
            },
        ),
        ProviderCommand::Exit { signed } => (signed, pallet_providers::Call::exit {}),
        ProviderCommand::Withdraw { signed } => {
            (signed, pallet_providers::Call::withdraw_unbonded {})
        }
        ProviderCommand::Show {
            node,
            address,
            wallet,
        } => {
            let who = who(address, &wallet)?;
            let client = NodeClient::new(&node.node)?;
            let Some(p) = client.market_provider(&who).await? else {
                bail!("{} is not a provider", encode_address(who.as_ref()));
            };
            let serviceable = client.market_is_serviceable(&who).await?;
            print_provider(&who, &p, serviceable);
            return Ok(());
        }
    };
    send(&signed, RuntimeCall::Providers(call)).await
}

async fn run_gateway(command: GatewayCommand) -> Result<()> {
    let (signed, call) = match command {
        GatewayCommand::Register {
            signed,
            endpoint,
            fee_bps,
            stake,
        } => {
            let stake = match stake {
                Some(s) => amount::parse_atc(&s)?,
                None => default_stake(&signed.node, None).await?,
            };
            let call = pallet_gateways::Call::register {
                endpoint: bounded(&endpoint, "the endpoint")?,
                fee_bps,
                stake,
            };
            (signed, call)
        }
        GatewayCommand::Update {
            signed,
            endpoint,
            fee_bps,
        } => {
            let call = pallet_gateways::Call::update {
                endpoint: endpoint.map(|e| bounded(&e, "the endpoint")).transpose()?,
                fee_bps,
            };
            (signed, call)
        }
        GatewayCommand::Bond { signed, amount: a } => (
            signed,
            pallet_gateways::Call::bond_extra {
                amount: amount::parse_atc(&a)?,
            },
        ),
        GatewayCommand::Unbond { signed, amount: a } => (
            signed,
            pallet_gateways::Call::unbond {
                amount: amount::parse_atc(&a)?,
            },
        ),
        GatewayCommand::Exit { signed } => (signed, pallet_gateways::Call::exit {}),
        GatewayCommand::Withdraw { signed } => {
            (signed, pallet_gateways::Call::withdraw_unbonded {})
        }
        GatewayCommand::Show {
            node,
            address,
            wallet,
        } => {
            let who = who(address, &wallet)?;
            let client = NodeClient::new(&node.node)?;
            let Some(g) = client.market_gateway(&who).await? else {
                bail!("{} is not a gateway", encode_address(who.as_ref()));
            };
            print_gateway(&who, &g);
            return Ok(());
        }
    };
    send(&signed, RuntimeCall::Gateways(call)).await
}
