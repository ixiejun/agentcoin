//! Deterministic key-seed derivation for wallets and development chains.
//!
//! ML-DSA has no public-key derivation, so wallet keys use only "hardened" derivation: every
//! key seed is a domain-separated BLAKE3 hash of the wallet entropy, the algorithm and an index.

use zeroize::{Zeroize, ZeroizeOnDrop, Zeroizing};

use crate::alg::SigAlg;
use crate::error::Error;
use crate::hash::derive;
use crate::sig::SecretSeed;

/// Context for wallet key derivation. Never change it.
pub const WALLET_KEY_CONTEXT: &str = "agentcoin 2026-09 wallet-key v1";

/// Context for public development seeds. Never change it.
pub const DEV_SEED_CONTEXT: &str = "agentcoin 2026-09 dev-seed v1";

/// Length of wallet entropy: 256 bits, encoded as a 24-word mnemonic.
pub const ENTROPY_LEN: usize = 32;

/// 32 bytes of wallet entropy (the root secret behind a mnemonic), wiped on drop.
#[derive(Clone, Zeroize, ZeroizeOnDrop)]
pub struct WalletEntropy([u8; ENTROPY_LEN]);

impl WalletEntropy {
    /// Wraps entropy bytes.
    #[must_use]
    pub const fn new(bytes: [u8; ENTROPY_LEN]) -> Self {
        Self(bytes)
    }

    /// Draws fresh entropy from a cryptographically secure RNG.
    #[cfg(feature = "rand")]
    pub fn generate<R: rand_core::CryptoRng + ?Sized>(rng: &mut R) -> Self {
        let mut bytes = [0u8; ENTROPY_LEN];
        rng.fill_bytes(&mut bytes);
        Self(bytes)
    }

    /// The entropy bytes. Key material: handle with care.
    #[must_use]
    pub const fn expose(&self) -> &[u8; ENTROPY_LEN] {
        &self.0
    }
}

impl core::fmt::Debug for WalletEntropy {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("WalletEntropy(<redacted>)")
    }
}

/// Derives the key-generation seed of wallet key number `index` for `alg`:
/// `derive_key(WALLET_KEY_CONTEXT, alg_id ‖ u32_le(index) ‖ entropy)`.
///
/// # Errors
///
/// [`Error::InvalidContext`] only if the built-in context were malformed (never in practice).
pub fn wallet_key_seed(
    entropy: &WalletEntropy,
    alg: SigAlg,
    index: u32,
) -> Result<SecretSeed, Error> {
    let mut material = Zeroizing::new([0u8; 1 + 4 + ENTROPY_LEN]);
    let (head, rest) = material.split_at_mut(1);
    head.copy_from_slice(&[alg.id()]);
    let (idx, ent) = rest.split_at_mut(4);
    idx.copy_from_slice(&index.to_le_bytes());
    ent.copy_from_slice(entropy.expose());
    let seed = Zeroizing::new(derive(WALLET_KEY_CONTEXT, material.as_slice())?);
    Ok(SecretSeed::new(*seed))
}

/// Derives the **public** development seed for `name` (for example `alice`).
///
/// These seeds are well known by construction: they must only ever control accounts and
/// authorities on development and local chains. The node refuses them on live chains.
///
/// # Errors
///
/// [`Error::InvalidContext`] only if the built-in context were malformed (never in practice).
pub fn dev_seed(name: &str) -> Result<SecretSeed, Error> {
    Ok(SecretSeed::new(derive(DEV_SEED_CONTEXT, name.as_bytes())?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contexts_are_well_formed() {
        assert!(crate::hash::validate_context(WALLET_KEY_CONTEXT).is_ok());
        assert!(crate::hash::validate_context(DEV_SEED_CONTEXT).is_ok());
    }

    // Requirement "助记词": same entropy, algorithm and index always give the same key.
    #[test]
    fn wallet_derivation_is_deterministic_and_separated() {
        let e = WalletEntropy::new([7; ENTROPY_LEN]);
        let a = wallet_key_seed(&e, SigAlg::MlDsa44, 0).unwrap();
        let b = wallet_key_seed(&e, SigAlg::MlDsa44, 0).unwrap();
        assert_eq!(a.expose(), b.expose());
        let other_index = wallet_key_seed(&e, SigAlg::MlDsa44, 1).unwrap();
        let other_alg = wallet_key_seed(&e, SigAlg::MlDsa65, 0).unwrap();
        assert_ne!(a.expose(), other_index.expose());
        assert_ne!(a.expose(), other_alg.expose());
    }

    // Scenario "开发密钥可复现".
    #[test]
    fn dev_seeds_are_reproducible() {
        assert_eq!(
            dev_seed("alice").unwrap().expose(),
            dev_seed("alice").unwrap().expose()
        );
        assert_ne!(
            dev_seed("alice").unwrap().expose(),
            dev_seed("bob").unwrap().expose()
        );
    }

    #[test]
    fn entropy_debug_is_redacted() {
        let e = WalletEntropy::new([0xAB; ENTROPY_LEN]);
        let s = alloc::format!("{e:?}");
        assert!(!s.contains("ab") && !s.contains("171"));
    }
}
