#![doc = include_str!("../README.md")]
#![cfg_attr(not(feature = "std"), no_std)]
#![forbid(unsafe_code)]

extern crate alloc;

mod block;
mod genesis;
pub mod keys;
mod transition;

pub use block::{Ledger, Violation, check_block, read_ledger};
pub use genesis::{GenesisError, GenesisParams, check_genesis};
pub use transition::{
    StakeSnapshot, SwitchState, TransitionGenesis, check_transition, read_switch_state,
    stake_snapshot,
};
