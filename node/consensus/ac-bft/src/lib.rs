#![doc = include_str!("../README.md")]
#![forbid(unsafe_code)]

pub mod chain;
pub mod gadget;
pub mod import;
pub mod network;
pub mod protocol;
#[cfg(any(test, feature = "test-utils"))]
pub mod sim;
pub mod tracker;

pub use protocol::{Action, Chain, Config, Voter, VoterState};

/// Log target of the AC-BFT node components.
pub const LOG_TARGET: &str = "ac-bft";
