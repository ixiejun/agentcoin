//! Hybrid key encapsulation dispatched by [`KemAlg`] (feature `kem`).

mod xwing;

use subtle::ConstantTimeEq;
use zeroize::{Zeroize, ZeroizeOnDrop};

use crate::alg::KemAlg;
use crate::error::Error;
use crate::sig::SecretSeed;
use crate::tagged::{KemCiphertext, KemPublicKey};

/// Length of a KEM shared secret.
pub const SHARED_SECRET_LEN: usize = 32;

/// Length of the encapsulation randomness consumed by X-Wing.
pub const ENCAPSULATION_RANDOMNESS_LEN: usize = 64;

/// A 32-byte shared secret. Wiped on drop, compared in constant time, never serialized.
#[derive(Clone, Zeroize, ZeroizeOnDrop)]
pub struct SharedSecret([u8; SHARED_SECRET_LEN]);

impl SharedSecret {
    /// The secret bytes. Key material: handle with care.
    #[must_use]
    pub const fn expose(&self) -> &[u8; SHARED_SECRET_LEN] {
        &self.0
    }
}

impl ConstantTimeEq for SharedSecret {
    fn ct_eq(&self, other: &Self) -> subtle::Choice {
        self.0.ct_eq(&other.0)
    }
}

impl core::fmt::Debug for SharedSecret {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("SharedSecret(<redacted>)")
    }
}

/// A KEM decapsulation (secret) key. Wiped on drop; `Debug` is redacted.
pub struct KemSecretKey {
    alg: KemAlg,
    inner: xwing::XWingSecret,
}

// The inner X-Wing key zeroizes itself on drop.
impl ZeroizeOnDrop for KemSecretKey {}

impl core::fmt::Debug for KemSecretKey {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("KemSecretKey")
            .field("alg", &self.alg)
            .finish_non_exhaustive()
    }
}

impl KemSecretKey {
    /// Deterministically derives a key pair from a 32-byte seed.
    ///
    /// # Errors
    ///
    /// [`Error::NotImplemented`] for reserved algorithms.
    pub fn from_seed(alg: KemAlg, seed: &SecretSeed) -> Result<Self, Error> {
        match alg {
            KemAlg::XWing => Ok(Self {
                alg,
                inner: xwing::XWingSecret::from_seed(seed.expose()),
            }),
            other => Err(Error::NotImplemented(other.id())),
        }
    }

    /// Generates a fresh key pair.
    ///
    /// # Errors
    ///
    /// [`Error::NotImplemented`] for reserved algorithms.
    #[cfg(feature = "rand")]
    pub fn generate<R: rand_core::CryptoRng + ?Sized>(
        alg: KemAlg,
        rng: &mut R,
    ) -> Result<Self, Error> {
        Self::from_seed(alg, &SecretSeed::generate(rng))
    }

    /// The algorithm of this key.
    #[must_use]
    pub const fn alg(&self) -> KemAlg {
        self.alg
    }

    /// The matching encapsulation (public) key.
    ///
    /// # Errors
    ///
    /// [`Error::InvalidLength`] only if the backend produced a key of unexpected size.
    pub fn public_key(&self) -> Result<KemPublicKey, Error> {
        KemPublicKey::new(self.alg, &self.inner.public_key())
    }

    /// Recovers the shared secret from a ciphertext.
    ///
    /// Invalid or foreign ciphertexts yield a pseudorandom secret (implicit rejection), never
    /// a distinguishable error.
    ///
    /// # Errors
    ///
    /// [`Error::AlgorithmMismatch`] if the ciphertext uses a different algorithm.
    pub fn decapsulate(&self, ciphertext: &KemCiphertext) -> Result<SharedSecret, Error> {
        if ciphertext.alg() != self.alg {
            return Err(Error::AlgorithmMismatch);
        }
        self.inner
            .decapsulate(ciphertext.as_bytes())
            .map(SharedSecret)
    }
}

/// Encapsulates a fresh shared secret to `public_key`.
///
/// # Errors
///
/// [`Error::InvalidKey`] if the key bytes are rejected; [`Error::NotImplemented`] for
/// reserved algorithms.
#[cfg(feature = "rand")]
pub fn encapsulate<R: rand_core::CryptoRng + ?Sized>(
    public_key: &KemPublicKey,
    rng: &mut R,
) -> Result<(KemCiphertext, SharedSecret), Error> {
    let mut randomness = [0u8; ENCAPSULATION_RANDOMNESS_LEN];
    rng.fill_bytes(&mut randomness);
    let result = encapsulate_with(public_key, &randomness);
    randomness.zeroize();
    result
}

/// Encapsulates with caller-supplied randomness (64 bytes). For test vectors and tooling only:
/// reusing randomness is catastrophic.
///
/// # Errors
///
/// As [`encapsulate`].
#[cfg(feature = "deterministic")]
pub fn encapsulate_derandomized(
    public_key: &KemPublicKey,
    randomness: &[u8; ENCAPSULATION_RANDOMNESS_LEN],
) -> Result<(KemCiphertext, SharedSecret), Error> {
    encapsulate_with(public_key, randomness)
}

#[cfg(any(feature = "rand", feature = "deterministic"))]
fn encapsulate_with(
    public_key: &KemPublicKey,
    randomness: &[u8; ENCAPSULATION_RANDOMNESS_LEN],
) -> Result<(KemCiphertext, SharedSecret), Error> {
    match public_key.alg() {
        KemAlg::XWing => {
            let (ct, ss) = xwing::encapsulate(public_key.as_bytes(), randomness)?;
            Ok((KemCiphertext::new(KemAlg::XWing, &ct)?, SharedSecret(ss)))
        }
        other => Err(Error::NotImplemented(other.id())),
    }
}
