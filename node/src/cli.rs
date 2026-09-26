//! Command-line interface.

/// Top-level CLI.
#[derive(Debug, clap::Parser)]
pub struct Cli {
    /// Optional subcommand; without one the node runs.
    #[command(subcommand)]
    pub subcommand: Option<Subcommand>,

    /// Options of the running node.
    #[clap(flatten)]
    pub run: sc_cli::RunCmd,
}

/// Maintenance subcommands.
#[derive(Debug, clap::Subcommand)]
pub enum Subcommand {
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
