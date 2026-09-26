//! ML-DSA (FIPS 204) backend. The only place that touches the `ml-dsa` crate.

use ml_dsa::{
    B32, EncodedVerifyingKey, Keypair, MlDsa44, MlDsa65, MlDsa87, MlDsaParams, Signature,
    SigningKey, VerifyingKey,
};
use zeroize::Zeroize;

use alloc::boxed::Box;
use alloc::vec::Vec;

/// An ML-DSA signing key of one of the three parameter sets.
///
/// Boxed: an expanded ML-DSA-87 key is ~100 KB and must not live on the stack (G.VAR.04).
#[derive(Clone)]
pub(crate) enum MlDsaKey {
    L44(Box<SigningKey<MlDsa44>>),
    L65(Box<SigningKey<MlDsa65>>),
    L87(Box<SigningKey<MlDsa87>>),
}

fn from_seed<P: MlDsaParams>(seed: &[u8; 32]) -> Box<SigningKey<P>> {
    let mut xi = B32::from(*seed);
    let key = Box::new(SigningKey::<P>::from_seed(&xi));
    xi.zeroize();
    key
}

fn public_key<P: MlDsaParams>(key: &SigningKey<P>) -> Vec<u8> {
    key.verifying_key().encode().as_slice().to_vec()
}

#[cfg(feature = "deterministic")]
fn sign_det<P: MlDsaParams>(key: &SigningKey<P>, msg: &[u8], ctx: &[u8]) -> Option<Vec<u8>> {
    let sig = key.expanded_key().sign_deterministic(msg, ctx).ok()?;
    Some(sig.encode().as_slice().to_vec())
}

#[cfg(feature = "rand")]
fn sign_rand<P: MlDsaParams, R: rand_core::CryptoRng + ?Sized>(
    key: &SigningKey<P>,
    msg: &[u8],
    ctx: &[u8],
    rng: &mut R,
) -> Option<Vec<u8>> {
    let sig = key.expanded_key().sign_randomized(msg, ctx, rng).ok()?;
    Some(sig.encode().as_slice().to_vec())
}

fn verify_raw<P: MlDsaParams>(pk: &[u8], msg: &[u8], ctx: &[u8], sig: &[u8]) -> bool {
    let Ok(enc) = EncodedVerifyingKey::<P>::try_from(pk) else {
        return false;
    };
    let Ok(sig) = Signature::<P>::try_from(sig) else {
        return false;
    };
    VerifyingKey::<P>::decode(&enc).verify_with_context(msg, ctx, &sig)
}

impl MlDsaKey {
    pub(crate) fn from_seed_44(seed: &[u8; 32]) -> Self {
        Self::L44(from_seed(seed))
    }

    pub(crate) fn from_seed_65(seed: &[u8; 32]) -> Self {
        Self::L65(from_seed(seed))
    }

    pub(crate) fn from_seed_87(seed: &[u8; 32]) -> Self {
        Self::L87(from_seed(seed))
    }

    pub(crate) fn public_key(&self) -> Vec<u8> {
        match self {
            Self::L44(k) => public_key(k),
            Self::L65(k) => public_key(k),
            Self::L87(k) => public_key(k),
        }
    }

    #[cfg(feature = "deterministic")]
    pub(crate) fn sign_deterministic(&self, msg: &[u8], ctx: &[u8]) -> Option<Vec<u8>> {
        match self {
            Self::L44(k) => sign_det(k, msg, ctx),
            Self::L65(k) => sign_det(k, msg, ctx),
            Self::L87(k) => sign_det(k, msg, ctx),
        }
    }

    #[cfg(feature = "rand")]
    pub(crate) fn sign_randomized<R: rand_core::CryptoRng + ?Sized>(
        &self,
        msg: &[u8],
        ctx: &[u8],
        rng: &mut R,
    ) -> Option<Vec<u8>> {
        match self {
            Self::L44(k) => sign_rand(k, msg, ctx, rng),
            Self::L65(k) => sign_rand(k, msg, ctx, rng),
            Self::L87(k) => sign_rand(k, msg, ctx, rng),
        }
    }
}

pub(crate) fn verify_44(pk: &[u8], msg: &[u8], ctx: &[u8], sig: &[u8]) -> bool {
    verify_raw::<MlDsa44>(pk, msg, ctx, sig)
}

pub(crate) fn verify_65(pk: &[u8], msg: &[u8], ctx: &[u8], sig: &[u8]) -> bool {
    verify_raw::<MlDsa65>(pk, msg, ctx, sig)
}

pub(crate) fn verify_87(pk: &[u8], msg: &[u8], ctx: &[u8], sig: &[u8]) -> bool {
    verify_raw::<MlDsa87>(pk, msg, ctx, sig)
}
