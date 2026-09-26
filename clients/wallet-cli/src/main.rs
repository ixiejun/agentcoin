//! `ac-wallet`: AgentCoin command-line wallet.

use std::io::{BufRead, IsTerminal};
use std::path::{Path, PathBuf};

use ac_wallet::{NodeClient, Wallet, amount, ops, parse_alg};
use anyhow::{Context, Result, bail};
use clap::Parser;
use zeroize::Zeroizing;

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
