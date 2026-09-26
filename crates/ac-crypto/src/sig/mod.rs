//! Post-quantum signatures dispatched by [`SigAlg`].
//!
//! Every signature is bound to a context string (FIPS 204 "pure" interface). Use one
//! registered context per purpose, formatted `agentcoin/<purpose>/v<version>`.

mod ml_dsa;

use zeroize::{Zeroize, ZeroizeOnDrop};

use crate::alg::SigAlg;
use crate::error::Error;
use crate::tagged::{PqPublicKey, PqSignature};

use self::ml_dsa::MlDsaKey;

/// Maximum length of a signing context string (FIPS 204).
pub const MAX_CONTEXT_LEN: usize = 255;

/// Length of a key-generation seed (FIPS 204 ξ).
pub const SEED_LEN: usize = 32;

/// A 32-byte secret key-generation seed, wiped on drop.
#[derive(Clone, Zeroize, ZeroizeOnDrop)]
pub struct SecretSeed([u8; SEED_LEN]);

impl SecretSeed {
    /// Wraps seed bytes.
    #[must_use]
    pub const fn new(bytes: [u8; SEED_LEN]) -> Self {
        Self(bytes)
    }

    /// Draws a fresh seed from a cryptographically secure RNG.
    #[cfg(feature = "rand")]
    pub fn generate<R: rand_core::CryptoRng + ?Sized>(rng: &mut R) -> Self {
        let mut bytes = [0u8; SEED_LEN];
        rng.fill_bytes(&mut bytes);
        Self(bytes)
    }

    /// The seed bytes. Key material: handle with care.
    #[must_use]
    pub const fn expose(&self) -> &[u8; SEED_LEN] {
        &self.0
    }
}

impl core::fmt::Debug for SecretSeed {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("SecretSeed(<redacted>)")
    }
}

/// A signing (secret) key for one [`SigAlg`]. Wiped on drop; `Debug` is redacted.
#[derive(Clone)]
pub struct SigningKey {
    alg: SigAlg,
    inner: MlDsaKey,
}

// The inner ML-DSA keys zeroize themselves on drop.
impl ZeroizeOnDrop for SigningKey {}

impl core::fmt::Debug for SigningKey {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("SigningKey")
            .field("alg", &self.alg)
            .finish_non_exhaustive()
    }
}

impl SigningKey {
    /// Deterministically derives a key pair from a seed (FIPS 204 `ML-DSA.KeyGen_internal`).
    ///
    /// # Errors
    ///
    /// [`Error::NotImplemented`] for reserved algorithms.
    pub fn from_seed(alg: SigAlg, seed: &SecretSeed) -> Result<Self, Error> {
        let inner = match alg {
            SigAlg::MlDsa44 => MlDsaKey::from_seed_44(seed.expose()),
            SigAlg::MlDsa65 => MlDsaKey::from_seed_65(seed.expose()),
            SigAlg::MlDsa87 => MlDsaKey::from_seed_87(seed.expose()),
            other => return Err(Error::NotImplemented(other.id())),
        };
        Ok(Self { alg, inner })
    }

    /// Generates a fresh key from a cryptographically secure RNG.
    ///
    /// # Errors
    ///
    /// [`Error::NotImplemented`] for reserved algorithms.
    #[cfg(feature = "rand")]
    pub fn generate<R: rand_core::CryptoRng + ?Sized>(
        alg: SigAlg,
        rng: &mut R,
    ) -> Result<Self, Error> {
        Self::from_seed(alg, &SecretSeed::generate(rng))
    }

    /// The algorithm of this key.
    #[must_use]
    pub const fn alg(&self) -> SigAlg {
        self.alg
    }

    /// The matching public key.
    ///
    /// # Errors
    ///
    /// [`Error::InvalidLength`] only if the backend produced a key of unexpected size.
    pub fn public_key(&self) -> Result<PqPublicKey, Error> {
        PqPublicKey::new(self.alg, &self.inner.public_key())
    }

    /// Signs `message` under `context` with fresh randomness (hedged signing, the default).
    ///
    /// # Errors
    ///
    /// [`Error::ContextTooLong`] if `context` exceeds 255 bytes; [`Error::Randomness`] if the
    /// backend fails.
    #[cfg(feature = "rand")]
    pub fn sign<R: rand_core::CryptoRng + ?Sized>(
        &self,
        message: &[u8],
        context: &[u8],
        rng: &mut R,
    ) -> Result<PqSignature, Error> {
        check_context(context)?;
        let raw = self
            .inner
            .sign_randomized(message, context, rng)
            .ok_or(Error::Randomness)?;
        PqSignature::new(self.alg, &raw)
    }

    /// Signs deterministically (FIPS 204 deterministic variant). For tests and tooling only.
    ///
    /// # Errors
    ///
    /// [`Error::ContextTooLong`] if `context` exceeds 255 bytes.
    #[cfg(feature = "deterministic")]
    pub fn sign_deterministic(&self, message: &[u8], context: &[u8]) -> Result<PqSignature, Error> {
        check_context(context)?;
        let raw = self
            .inner
            .sign_deterministic(message, context)
            .ok_or(Error::ContextTooLong)?;
        PqSignature::new(self.alg, &raw)
    }
}

fn check_context(context: &[u8]) -> Result<(), Error> {
    if context.len() > MAX_CONTEXT_LEN {
        Err(Error::ContextTooLong)
    } else {
        Ok(())
    }
}

/// Verifies `signature` over `message` under `context`.
///
/// Available without `std` and without an RNG (usable inside the WASM runtime).
///
/// # Errors
///
/// - [`Error::AlgorithmMismatch`] if key and signature algorithms differ;
/// - [`Error::ContextTooLong`] if `context` exceeds 255 bytes;
/// - [`Error::NotImplemented`] for reserved algorithms;
/// - [`Error::InvalidSignature`] if the signature does not verify.
pub fn verify(
    public_key: &PqPublicKey,
    message: &[u8],
    context: &[u8],
    signature: &PqSignature,
) -> Result<(), Error> {
    if public_key.alg() != signature.alg() {
        return Err(Error::AlgorithmMismatch);
    }
    check_context(context)?;
    let (pk, sig) = (public_key.as_bytes(), signature.as_bytes());
    let valid = match public_key.alg() {
        SigAlg::MlDsa44 => ml_dsa::verify_44(pk, message, context, sig),
        SigAlg::MlDsa65 => ml_dsa::verify_65(pk, message, context, sig),
        SigAlg::MlDsa87 => ml_dsa::verify_87(pk, message, context, sig),
        other => return Err(Error::NotImplemented(other.id())),
    };
    if valid {
        Ok(())
    } else {
        Err(Error::InvalidSignature)
    }
}

// Compile-time proof that verification is part of the no_std / wasm32 build (task 4.4).
#[cfg(target_arch = "wasm32")]
const _: fn(&PqPublicKey, &[u8], &[u8], &PqSignature) -> Result<(), Error> = verify;
