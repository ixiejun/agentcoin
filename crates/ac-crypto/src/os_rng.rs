//! A cryptographically secure RNG seeded from the operating system, without panics.
//!
//! `OsRng::new` draws 32 bytes from the OS CSPRNG (the only step that can fail, reported as
//! [`Error::Randomness`]) and expands them with the BLAKE3 extendable output function in
//! `derive_key` mode. Generation afterwards is infallible, so the RNG satisfies
//! `rand_core::CryptoRng` without any "panic if the OS fails" adapter.

use core::convert::Infallible;

use zeroize::Zeroizing;

use crate::error::Error;

/// Context of the output stream. Never change it.
pub const OS_RNG_CONTEXT: &str = "agentcoin 2026-09 os-rng v1";

/// CSPRNG seeded once from the operating system.
pub struct OsRng(blake3::OutputReader);

impl OsRng {
    /// Seeds a new RNG from the operating system.
    ///
    /// # Errors
    ///
    /// [`Error::Randomness`] if the operating system's random source fails.
    pub fn new() -> Result<Self, Error> {
        let mut seed = Zeroizing::new([0u8; 32]);
        getrandom::fill(seed.as_mut_slice()).map_err(|_| Error::Randomness)?;
        let mut hasher = blake3::Hasher::new_derive_key(OS_RNG_CONTEXT);
        hasher.update(seed.as_slice());
        Ok(Self(hasher.finalize_xof()))
    }
}

impl core::fmt::Debug for OsRng {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("OsRng(<redacted>)")
    }
}

impl rand_core::TryRng for OsRng {
    type Error = Infallible;

    fn try_next_u32(&mut self) -> Result<u32, Infallible> {
        let mut b = [0u8; 4];
        self.0.fill(&mut b);
        Ok(u32::from_le_bytes(b))
    }

    fn try_next_u64(&mut self) -> Result<u64, Infallible> {
        let mut b = [0u8; 8];
        self.0.fill(&mut b);
        Ok(u64::from_le_bytes(b))
    }

    fn try_fill_bytes(&mut self, dst: &mut [u8]) -> Result<(), Infallible> {
        self.0.fill(dst);
        Ok(())
    }
}

impl rand_core::TryCryptoRng for OsRng {}

#[cfg(test)]
mod tests {
    use super::*;
    use rand_core::Rng;

    #[test]
    fn context_is_well_formed() {
        assert!(crate::hash::validate_context(OS_RNG_CONTEXT).is_ok());
    }

    #[test]
    fn independent_instances_differ() {
        let mut a = OsRng::new().unwrap();
        let mut b = OsRng::new().unwrap();
        let (mut x, mut y) = ([0u8; 32], [0u8; 32]);
        a.fill_bytes(&mut x);
        b.fill_bytes(&mut y);
        assert_ne!(x, y);
        let mut z = [0u8; 32];
        a.fill_bytes(&mut z);
        assert_ne!(x, z);
    }

    #[test]
    fn hedged_signature_verifies() {
        use crate::sig::{SecretSeed, SigningKey, verify};
        let key = SigningKey::from_seed(crate::SigAlg::MlDsa65, &SecretSeed::new([3; 32])).unwrap();
        let mut rng = OsRng::new().unwrap();
        let sig = key
            .sign(b"block", b"agentcoin/aura-seal/v1", &mut rng)
            .unwrap();
        assert!(
            verify(
                &key.public_key().unwrap(),
                b"block",
                b"agentcoin/aura-seal/v1",
                &sig
            )
            .is_ok()
        );
    }
}
