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
mod keys;
#[cfg(feature = "keystore")]
pub mod keystore;
#[cfg(feature = "mnemonic")]
pub mod mnemonic;
#[cfg(feature = "getrandom")]
mod os_rng;
pub mod sig;
mod tagged;

pub use account::{ACCOUNT_ID_CONTEXT, AccountId, account_id};
pub use alg::{EXTENSION_MARKER, KemAlg, SigAlg};
pub use error::{Error, MnemonicError};
pub use keys::{
    DEV_SEED_CONTEXT, ENTROPY_LEN, RANDOMNESS_COMMIT_CONTEXT, RANDOMNESS_SECRET_CONTEXT,
    RandomnessSecret, WALLET_KEY_CONTEXT, WalletEntropy, dev_seed, randomness_commit,
    randomness_secret, wallet_key_seed,
};
#[cfg(feature = "getrandom")]
pub use os_rng::{OS_RNG_CONTEXT, OsRng};
pub use tagged::{ALG_ID_LEN, KemCiphertext, KemPublicKey, PqPublicKey, PqSignature};
