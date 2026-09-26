#![doc = include_str!("../README.md")]
#![cfg_attr(not(feature = "std"), no_std)]
#![forbid(unsafe_code)]

extern crate alloc;

mod address;
pub mod aura_pq;
mod hashing;
pub mod profile;
mod signature;

pub use address::{
    ADDRESS_HRP, AddressError, decode_address, encode_address, from_runtime_account,
    to_runtime_account,
};
pub use hashing::Blake3Hasher;
pub use profile::{ChainHashing, ChainProfile};
pub use signature::{NoClassicSignature, NoClassicSigner};
