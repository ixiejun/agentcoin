#![doc = include_str!("../README.md")]
#![cfg_attr(not(feature = "std"), no_std)]
#![forbid(unsafe_code)]

extern crate alloc;

pub mod ac_bft;
mod address;
pub mod aura_pq;
pub mod epoch;
mod hashing;
pub mod offences;
pub mod profile;
mod signature;
pub mod validator_set;

pub use address::{
    ADDRESS_HRP, AddressError, decode_address, encode_address, from_runtime_account,
    to_runtime_account,
};
pub use hashing::Blake3Hasher;
pub use profile::{ChainHashing, ChainProfile};
pub use signature::{NoClassicSignature, NoClassicSigner};
