//! Command-line interface.

use std::path::PathBuf;

use crate::keys::PqKeyArgs;

/// Top-level CLI.
#[derive(Debug, clap::Parser)]
pub struct Cli {
    /// Optional subcommand; without one the node runs.
    #[command(subcommand)]
    pub subcommand: Option<Subcommand>,

    /// Options of the running node.
    #[clap(flatten)]
    pub run: sc_cli::RunCmd,

    /// Aura-PQ authority key.
    #[clap(flatten)]
    pub pq: PqKeyArgs,
}

/// Maintenance subcommands.
#[derive(Debug, clap::Subcommand)]
pub enum Subcommand {
    /// Manage Aura-PQ authority keys.
    #[command(subcommand)]
    PqKey(PqKeyCmd),
    /// Export the chain specification.
    ExportChainSpec(sc_cli::ExportChainSpecCmd),
    /// Validate blocks.
    CheckBlock(sc_cli::CheckBlockCmd),
    /// Export blocks.
    ExportBlocks(sc_cli::ExportBlocksCmd),
    /// Export the state of a given block into a chain spec.
    ExportState(sc_cli::ExportStateCmd),
    /// Import blocks.
    ImportBlocks(sc_cli::ImportBlocksCmd),
    /// Remove the whole chain.
    PurgeChain(sc_cli::PurgeChainCmd),
    /// Revert the chain to a previous state.
    Revert(sc_cli::RevertCmd),
}

/// Aura-PQ key management.
#[derive(Debug, clap::Subcommand)]
pub enum PqKeyCmd {
    /// Generate a new ML-DSA-65 authority key into a password-encrypted file and print its
    /// public key (for the genesis authority list).
    Generate {
        /// Output file; must not exist yet.
        #[arg(long, value_name = "PATH")]
        output: PathBuf,
        /// File containing the passphrase.
        #[arg(long, value_name = "PATH")]
        password_file: PathBuf,
    },
    /// Print the public key recorded in an encrypted key file (no passphrase needed).
    Inspect {
        /// Key file.
        #[arg(long, value_name = "PATH")]
        file: PathBuf,
    },
}
