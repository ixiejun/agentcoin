#![doc = include_str!("../README.md")]
#![cfg_attr(not(feature = "std"), no_std)]
#![forbid(unsafe_code)]

extern crate alloc;

mod block;
mod genesis;
pub mod keys;

pub use block::{Ledger, Violation, check_block, read_ledger};
pub use genesis::{GenesisError, GenesisParams, check_genesis};
