//! AgentCoin node.

// `sc_cli::Error` and `sc_service::Error` are large SDK error types used across the node's
// start-up paths; boxing them at every call site would only add noise on cold paths.
#![allow(clippy::result_large_err)]

mod chain_spec;
mod cli;
mod command;
mod genesis_guard;
mod keys;
mod rpc;
mod service;

fn main() -> sc_cli::Result<()> {
    command::run()
}
