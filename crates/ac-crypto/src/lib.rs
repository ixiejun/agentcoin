#![doc = include_str!("../README.md")]
#![no_std]
#![forbid(unsafe_code)]

#[cfg(feature = "std")]
extern crate std;

extern crate alloc;

mod account;
mod alg;
mod error;
pub mod hash;
#[cfg(feature = "kem")]
pub mod kem;
pub mod sig;
mod tagged;

pub use account::{ACCOUNT_ID_CONTEXT, AccountId, account_id};
pub use alg::{KemAlg, PROOF_SYSTEM_RESERVED, SigAlg};
pub use error::Error;
pub use tagged::{ALG_ID_LEN, KemCiphertext, KemPublicKey, PqPublicKey, PqSignature};
