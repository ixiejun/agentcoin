//! `ac-wallet`: AgentCoin command-line wallet.

use std::io::{BufRead, IsTerminal};
use std::path::{Path, PathBuf};

use ac_wallet::evm::{self, ContractTx, Overrides};
use ac_wallet::{NodeClient, Wallet, amount, ops, parse_alg};
use anyhow::{Context, Result, bail};
use clap::Parser;
use zeroize::Zeroizing;

mod market_cli;

/// AgentCoin command-line wallet (ML-DSA keys, `atc1…` addresses).
#[derive(Debug, Parser)]
#[command(version)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, clap::Args)]
struct WalletArgs {
    /// Wallet file.
    #[arg(long, value_name = "PATH", default_value = "wallet.json")]
    wallet: PathBuf,
    /// File with the wallet passphrase; without it the passphrase is read from the terminal.
    #[arg(long, value_name = "PATH")]
    password_file: Option<PathBuf>,
}

#[derive(Debug, clap::Args)]
struct NodeArgs {
    /// Node RPC endpoint.
    #[arg(long, value_name = "URL", default_value = "http://127.0.0.1:9944")]
    node: String,
}

#[derive(Debug, clap::Subcommand)]
enum Command {
    /// Create a wallet and show its 24-word mnemonic once.
    New {
        #[command(flatten)]
        wallet: WalletArgs,
        /// Signature algorithm of the account key.
        #[arg(long, default_value = "ml-dsa-44")]
        alg: String,
    },
    /// Restore a wallet from a mnemonic read from standard input.
    Import {
        #[command(flatten)]
        wallet: WalletArgs,
        /// Signature algorithm of the account's first key.
        #[arg(long, default_value = "ml-dsa-44")]
        alg: String,
    },
    /// Print the wallet's address.
    Address {
        /// Wallet file.
        #[arg(long, value_name = "PATH", default_value = "wallet.json")]
        wallet: PathBuf,
    },
    /// Print the wallet's current key (algorithm, index, public key).
    KeyInfo {
        #[command(flatten)]
        wallet: WalletArgs,
    },
    /// Show the balance, nonce and key registration of an address or of the wallet.
    Balance {
        /// Address to query; defaults to the wallet's.
        #[arg(long)]
        address: Option<String>,
        /// Wallet file (used when no address is given).
        #[arg(long, value_name = "PATH", default_value = "wallet.json")]
        wallet: PathBuf,
        #[command(flatten)]
        node: NodeArgs,
    },
    /// Transfer ATC.
    Transfer {
        #[command(flatten)]
        wallet: WalletArgs,
        #[command(flatten)]
        node: NodeArgs,
        /// Recipient address (`atc1…`).
        #[arg(long)]
        to: String,
        /// Amount in ATC, for example `1.5`.
        #[arg(long)]
        amount: String,
    },
    /// EVM contracts: the wallet signs contract transactions with its ML-DSA key.
    Evm {
        #[command(subcommand)]
        command: EvmCommand,
    },
    /// Inference market: models, providers, gateways, escrow and vouchers.
    Market {
        #[command(subcommand)]
        command: market_cli::MarketCommand,
    },
    /// Rotate to a new key (next derivation index); the address stays the same.
    Rotate {
        #[command(flatten)]
        wallet: WalletArgs,
        #[command(flatten)]
        node: NodeArgs,
        /// Algorithm of the new key; defaults to the current one.
        #[arg(long)]
        alg: Option<String>,
    },
}

#[derive(Debug, clap::Subcommand)]
enum EvmCommand {
    /// Print the account's 20-byte EVM address (EIP-55).
    Address {
        /// Wallet file.
        #[arg(long, value_name = "PATH", default_value = "wallet.json")]
        wallet: PathBuf,
    },
    /// Deploy a contract from a `forge build` artifact or raw init code.
    Deploy {
        #[command(flatten)]
        wallet: WalletArgs,
        #[command(flatten)]
        node: NodeArgs,
        #[command(flatten)]
        deploy: DeployArgs,
        #[command(flatten)]
        limits: LimitArgs,
    },
    /// Call a contract.
    Send {
        #[command(flatten)]
        wallet: WalletArgs,
        #[command(flatten)]
        node: NodeArgs,
        #[command(flatten)]
        call: CallArgs,
        #[command(flatten)]
        limits: LimitArgs,
    },
    /// Sign a deployment or call without submitting it; prints the bytes for
    /// `eth_sendRawTransaction`.
    Raw {
        #[command(subcommand)]
        tx: RawTx,
    },
    /// Send the transactions of a `forge script` run (`run-latest.json`) one by one.
    Broadcast {
        #[command(flatten)]
        wallet: WalletArgs,
        #[command(flatten)]
        node: NodeArgs,
        /// The broadcast file, e.g. `broadcast/Deploy.s.sol/4403/run-latest.json`.
        #[arg(long, value_name = "PATH")]
        file: PathBuf,
        #[command(flatten)]
        limits: LimitArgs,
    },
    /// Sign a message for the `pq_verify` precompile (context `agentcoin/evm-verify/v1`).
    SignMessage {
        #[command(flatten)]
        wallet: WalletArgs,
        /// Message text.
        #[arg(long, conflicts_with = "hex", required_unless_present = "hex")]
        message: Option<String>,
        /// Message bytes in hex.
        #[arg(long)]
        hex: Option<String>,
    },
}

#[derive(Debug, clap::Subcommand)]
enum RawTx {
    /// Sign a deployment.
    Deploy {
        #[command(flatten)]
        wallet: WalletArgs,
        #[command(flatten)]
        node: NodeArgs,
        #[command(flatten)]
        deploy: DeployArgs,
        #[command(flatten)]
        limits: LimitArgs,
    },
    /// Sign a call.
    Send {
        #[command(flatten)]
        wallet: WalletArgs,
        #[command(flatten)]
        node: NodeArgs,
        #[command(flatten)]
        call: CallArgs,
        #[command(flatten)]
        limits: LimitArgs,
    },
}

#[derive(Debug, clap::Args)]
struct DeployArgs {
    /// `forge build` artifact (JSON) holding the init code.
    #[arg(long, value_name = "PATH", required_unless_present = "bytecode")]
    artifact: Option<PathBuf>,
    /// Init code in hex instead of an artifact.
    #[arg(long, conflicts_with = "artifact")]
    bytecode: Option<String>,
    /// Constructor signature, e.g. `constructor(string,string,uint256)`; the arguments follow.
    #[arg(long)]
    constructor: Option<String>,
    /// ABI-encoded constructor arguments, e.g. from `cast abi-encode`.
    #[arg(long, value_name = "HEX", conflicts_with = "constructor")]
    constructor_args: Option<String>,
    /// Value sent to the constructor, in ATC.
    #[arg(long, default_value = "0")]
    value: String,
    /// Constructor arguments.
    #[arg(allow_hyphen_values = true)]
    args: Vec<String>,
}

#[derive(Debug, clap::Args)]
struct CallArgs {
    /// Contract address.
    #[arg(long)]
    to: String,
    /// Function signature, e.g. `transfer(address,uint256)`; the arguments follow.
    #[arg(long, required_unless_present = "data")]
    sig: Option<String>,
    /// Call data in hex instead of a signature and arguments.
    #[arg(long, conflicts_with = "sig")]
    data: Option<String>,
    /// Value sent with the call, in ATC.
    #[arg(long, default_value = "0")]
    value: String,
    /// Function arguments.
    #[arg(allow_hyphen_values = true)]
    args: Vec<String>,
}

#[derive(Debug, clap::Args)]
struct LimitArgs {
    /// Ref-time weight limit instead of the dry run's estimate plus 20%.
    #[arg(long)]
    ref_time_limit: Option<u64>,
    /// Proof-size weight limit instead of the dry run's estimate plus 20%.
    #[arg(long)]
    proof_size_limit: Option<u64>,
    /// Storage deposit limit in ATC instead of the dry run's peak plus 10%.
    #[arg(long)]
    deposit_limit: Option<String>,
    /// Sign and submit even if the dry run reverts.
    #[arg(long)]
    force: bool,
}

impl LimitArgs {
    fn overrides(&self) -> Result<Overrides> {
        Ok(Overrides {
            ref_time: self.ref_time_limit,
            proof_size: self.proof_size_limit,
            deposit: self
                .deposit_limit
                .as_deref()
                .map(amount::parse_atc)
                .transpose()?,
            force: self.force,
        })
    }
}

impl DeployArgs {
    fn tx(&self) -> Result<ContractTx> {
        let code = match (&self.artifact, &self.bytecode) {
            (Some(path), _) => evm::foundry::init_code(
                &std::fs::read_to_string(path)
                    .with_context(|| format!("cannot read {}", path.display()))?,
            )?,
            (None, Some(hex)) => evm::abi::parse_hex(hex)?,
            (None, None) => bail!("give --artifact or --bytecode"),
        };
        evm::deployment(
            code,
            self.constructor.as_deref(),
            &self.args,
            self.constructor_args.as_deref(),
            amount::parse_atc(&self.value)?,
        )
    }
}

impl CallArgs {
    fn tx(&self) -> Result<ContractTx> {
        evm::call(
            &self.to,
            self.sig.as_deref(),
            &self.args,
            self.data.as_deref(),
            amount::parse_atc(&self.value)?,
        )
    }
}

/// Dry-runs and signs `tx`, reporting the estimate on standard error.
async fn prepare(
    wallet: &WalletArgs,
    node: &NodeArgs,
    tx: &ContractTx,
    limits: &LimitArgs,
) -> Result<(NodeClient, Wallet, evm::Signed)> {
    let w = load(&wallet.wallet)?;
    let pw = password(wallet, false)?;
    let client = NodeClient::new(&node.node)?;
    let signed = evm::prepare(&client, &w, &pw, tx, &limits.overrides()?).await?;
    let l = signed.estimate.limits;
    eprintln!(
        "limits: ref time {}, proof size {}, storage deposit {}",
        l.weight.ref_time(),
        l.weight.proof_size(),
        amount::format_atc(l.deposit)
    );
    if let Some(failure) = &signed.estimate.failure {
        eprintln!("warning: the dry run {failure}; submitting anyway (--force)");
    }
    Ok((client, w, signed))
}

/// Submits a prepared transaction and prints where it landed.
async fn submit(client: &NodeClient, w: &Wallet, signed: &evm::Signed) -> Result<evm::Outcome> {
    let me = evm::evm_address(&w.account()?);
    let outcome = evm::submit(client, signed, me).await?;
    println!("transaction: {:?}", outcome.hash);
    println!("block: {:?}", outcome.inclusion.block_hash);
    if !outcome.inclusion.success {
        bail!(
            "the transaction failed: {}",
            outcome.error.as_deref().unwrap_or("unknown reason")
        );
    }
    Ok(outcome)
}

fn print_raw(signed: &evm::Signed) {
    println!("transaction: {:?}", signed.hash);
    println!(
        "raw: 0x{}",
        hex::encode(parity_scale_codec::Encode::encode(&signed.xt))
    );
}

async fn run_evm(command: EvmCommand) -> Result<()> {
    match command {
        EvmCommand::Address { wallet } => {
            println!(
                "{}",
                evm::checksummed(evm::evm_address(&load(&wallet)?.account()?))
            );
        }
        EvmCommand::Deploy {
            wallet,
            node,
            deploy,
            limits,
        } => {
            let tx = deploy.tx()?;
            let (client, w, signed) = prepare(&wallet, &node, &tx, &limits).await?;
            let outcome = submit(&client, &w, &signed).await?;
            let contract = outcome
                .contract
                .context("the deployment succeeded but reported no contract")?;
            println!("contract: {}", evm::checksummed(contract));
        }
        EvmCommand::Send {
            wallet,
            node,
            call,
            limits,
        } => {
            let tx = call.tx()?;
            let (client, w, signed) = prepare(&wallet, &node, &tx, &limits).await?;
            submit(&client, &w, &signed).await?;
            println!("status: success");
        }
        EvmCommand::Raw { tx } => {
            let signed = match tx {
                RawTx::Deploy {
                    wallet,
                    node,
                    deploy,
                    limits,
                } => prepare(&wallet, &node, &deploy.tx()?, &limits).await?.2,
                RawTx::Send {
                    wallet,
                    node,
                    call,
                    limits,
                } => prepare(&wallet, &node, &call.tx()?, &limits).await?.2,
            };
            print_raw(&signed);
        }
        EvmCommand::Broadcast {
            wallet,
            node,
            file,
            limits,
        } => {
            let w = load(&wallet.wallet)?;
            let me = evm::evm_address(&w.account()?);
            let text = std::fs::read_to_string(&file)
                .with_context(|| format!("cannot read {}", file.display()))?;
            let steps = evm::foundry::broadcast_steps(&text, me)?;
            let pw = password(&wallet, false)?;
            let client = NodeClient::new(&node.node)?;
            let sender = evm::Sender {
                client: &client,
                wallet: &w,
                password: &pw,
            };
            let (done, stop) = evm::broadcast(sender, &steps, &limits.overrides()?, |n, result| {
                let created = result
                    .created
                    .map(|c| format!(", contract {}", evm::checksummed(c)))
                    .unwrap_or_default();
                println!("{n}/{}: {:?}{created}", steps.len(), result.hash);
            })
            .await?;
            if let Some(stop) = stop {
                bail!("{stop}");
            }
            println!("broadcast complete: {} transaction(s)", done.len());
        }
        EvmCommand::SignMessage {
            wallet,
            message,
            hex: hex_message,
        } => {
            let bytes = match (message, hex_message) {
                (Some(text), _) => text.into_bytes(),
                (None, Some(h)) => evm::abi::parse_hex(&h)?,
                (None, None) => bail!("give --message or --hex"),
            };
            let w = load(&wallet.wallet)?;
            let pw = password(&wallet, false)?;
            let signed = evm::sign_message(&w, &pw, &bytes)?;
            println!("alg: {}", signed.alg.id());
            println!(
                "public key: 0x{}",
                hex::encode(signed.public_key.as_bytes())
            );
            println!("message: 0x{}", hex::encode(&bytes));
            println!("signature: 0x{}", hex::encode(signed.signature.as_bytes()));
        }
    }
    Ok(())
}

/// Reads the passphrase from a file (one trailing newline dropped) or the terminal.
fn password(args: &WalletArgs, confirm: bool) -> Result<Zeroizing<Vec<u8>>> {
    if let Some(path) = &args.password_file {
        let mut bytes = Zeroizing::new(
            std::fs::read(path).with_context(|| format!("cannot read {}", path.display()))?,
        );
        if bytes.last() == Some(&b'\n') {
            bytes.pop();
            if bytes.last() == Some(&b'\r') {
                bytes.pop();
            }
        }
        return Ok(bytes);
    }
    if !std::io::stdin().is_terminal() {
        bail!("no terminal to read the passphrase from; use --password-file");
    }
    let first = Zeroizing::new(rpassword::prompt_password("Wallet passphrase: ")?);
    if confirm {
        let second = Zeroizing::new(rpassword::prompt_password("Repeat passphrase: ")?);
        if *first != *second {
            bail!("passphrases do not match");
        }
    }
    Ok(Zeroizing::new(first.as_bytes().to_vec()))
}

fn load(path: &Path) -> Result<Wallet> {
    Wallet::load(path)
}

#[tokio::main]
async fn main() -> Result<()> {
    match Cli::parse().command {
        Command::New { wallet, alg } => {
            let pw = password(&wallet, true)?;
            let created = Wallet::create(parse_alg(&alg)?, &pw)?;
            created.wallet.save(&wallet.wallet, false)?;
            println!("address: {}", created.wallet.address());
            eprintln!(
                "Write down these 24 words; they are shown only once:\n{}",
                *created.mnemonic
            );
        }
        Command::Import { wallet, alg } => {
            eprintln!("Enter the 24-word mnemonic:");
            let mut line = Zeroizing::new(String::new());
            std::io::stdin().lock().read_line(&mut line)?;
            let pw = password(&wallet, true)?;
            let imported = Wallet::import(&line, parse_alg(&alg)?, &pw)?;
            imported.save(&wallet.wallet, false)?;
            println!("address: {}", imported.address());
        }
        Command::Address { wallet } => println!("{}", load(&wallet)?.address()),
        Command::KeyInfo { wallet } => {
            let w = load(&wallet.wallet)?;
            let (alg, index) = w.current();
            let pw = password(&wallet, false)?;
            let public = w.current_key(&pw)?.public_key()?;
            println!(
                "address: {}\nalgorithm: {alg:?}\nindex: {index}",
                w.address()
            );
            println!("public key: 0x{}", hex::encode(public.to_canonical()));
        }
        Command::Balance {
            address,
            wallet,
            node,
        } => {
            let address = match address {
                Some(a) => a,
                None => load(&wallet)?.address().to_string(),
            };
            let who = ac_wallet::parse_address(&address)?;
            let client = NodeClient::new(&node.node)?;
            println!("address: {address}");
            println!(
                "balance: {}",
                amount::format_atc(client.free_balance(&who).await?)
            );
            println!("nonce: {}", client.nonce(&who).await?);
            match client.current_key(&who).await? {
                Some((key, rotations)) => {
                    println!("registered: {:?} key, {rotations} rotation(s)", key.alg());
                }
                None => println!("registered: no (the first transaction carries the key)"),
            }
        }
        Command::Transfer {
            wallet,
            node,
            to,
            amount: text,
        } => {
            let to = ac_wallet::parse_address(&to)?;
            let value = amount::parse_atc(&text)?;
            let w = load(&wallet.wallet)?;
            let pw = password(&wallet, false)?;
            let client = NodeClient::new(&node.node)?;
            let inclusion = ops::transfer(&client, &w, &pw, &to, value).await?;
            if !inclusion.success {
                bail!(
                    "transfer included in block {:?} but failed",
                    inclusion.block_hash
                );
            }
            println!("included in block {:?}", inclusion.block_hash);
        }
        Command::Evm { command } => run_evm(command).await?,
        Command::Market { command } => market_cli::run(command).await?,
        Command::Rotate { wallet, node, alg } => {
            let mut w = load(&wallet.wallet)?;
            let alg = match alg {
                Some(name) => parse_alg(&name)?,
                None => w.current().0,
            };
            let pw = password(&wallet, false)?;
            let client = NodeClient::new(&node.node)?;
            let inclusion = ops::rotate(&client, &mut w, &pw, alg).await?;
            w.save(&wallet.wallet, true)?;
            println!(
                "rotated in block {:?}; address unchanged: {}",
                inclusion.block_hash,
                w.address()
            );
        }
    }
    Ok(())
}
