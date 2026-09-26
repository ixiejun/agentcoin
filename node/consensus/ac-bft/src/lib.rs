#![doc = include_str!("../README.md")]
#![forbid(unsafe_code)]

pub mod protocol;
#[cfg(any(test, feature = "test-utils"))]
pub mod sim;

pub use protocol::{Action, Chain, Config, Voter, VoterState};
