//! Aura-PQ consensus primitives shared by the runtime and the node (decision D13, plan §4.1).
//!
//! Blocks carry a pre-runtime digest with their slot and a seal digest with an ML-DSA-65
//! signature (context [`SEAL_CONTEXT`]) over the header hash without the seal.

use alloc::vec::Vec;

use ac_crypto::{PqPublicKey, PqSignature, SigAlg};
use parity_scale_codec::{Decode, Encode};
use sp_runtime::{ConsensusEngineId, Digest, DigestItem};

pub use sp_consensus_slots::Slot;

/// Consensus engine ID of Aura-PQ digests.
pub const ENGINE_ID: ConsensusEngineId = *b"acpq";

/// ML-DSA signing context of block seals. Never change it.
pub const SEAL_CONTEXT: &[u8] = b"agentcoin/aura-seal/v1";

/// Algorithm required for authority keys in M1.
pub const AUTHORITY_ALG: SigAlg = SigAlg::MlDsa65;

/// Default maximum number of authorities (plan §4.1: at most 100 validators in the MVP).
pub const MAX_AUTHORITIES: u32 = 100;

/// Why an authority set is invalid.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuthorityError {
    /// The set is empty.
    Empty,
    /// The set has more members than allowed.
    TooMany,
    /// A key appears twice.
    Duplicate,
    /// A key does not use [`AUTHORITY_ALG`].
    WrongAlgorithm,
}

impl core::fmt::Display for AuthorityError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match self {
            Self::Empty => "the authority set is empty",
            Self::TooMany => "the authority set exceeds the maximum size",
            Self::Duplicate => "the authority set contains a duplicate key",
            Self::WrongAlgorithm => "authority keys must be ML-DSA-65",
        })
    }
}

/// Checks that an authority set is non-empty, within `max`, free of duplicates and ML-DSA-65.
///
/// # Errors
///
/// The first violated rule, as an [`AuthorityError`].
pub fn validate_authorities(authorities: &[PqPublicKey], max: u32) -> Result<(), AuthorityError> {
    if authorities.is_empty() {
        return Err(AuthorityError::Empty);
    }
    if u32::try_from(authorities.len()).map_or(true, |n| n > max) {
        return Err(AuthorityError::TooMany);
    }
    if authorities.iter().any(|k| k.alg() != AUTHORITY_ALG) {
        return Err(AuthorityError::WrongAlgorithm);
    }
    for (i, key) in authorities.iter().enumerate() {
        if authorities
            .iter()
            .skip(i.saturating_add(1))
            .any(|k| k == key)
        {
            return Err(AuthorityError::Duplicate);
        }
    }
    Ok(())
}

/// The authority entitled to author in `slot`: index `slot mod N`.
#[must_use]
pub fn slot_author(slot: Slot, authorities: &[PqPublicKey]) -> Option<&PqPublicKey> {
    let n = u64::try_from(authorities.len()).ok().filter(|n| *n > 0)?;
    let index = usize::try_from(u64::from(slot).checked_rem(n)?).ok()?;
    authorities.get(index)
}

/// Pre-runtime digest announcing the block's slot.
#[must_use]
pub fn pre_digest(slot: Slot) -> DigestItem {
    DigestItem::PreRuntime(ENGINE_ID, slot.encode())
}

/// Seal digest carrying the author's signature.
#[must_use]
pub fn seal_digest(signature: &PqSignature) -> DigestItem {
    DigestItem::Seal(ENGINE_ID, signature.encode())
}

/// Why a digest does not carry a usable Aura-PQ pre-runtime item.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DigestError {
    /// No Aura-PQ pre-runtime digest.
    Missing,
    /// More than one Aura-PQ pre-runtime digest.
    Multiple,
    /// The slot does not decode.
    Malformed,
}

/// Finds the unique Aura-PQ slot in a digest.
///
/// # Errors
///
/// [`DigestError`] if the slot is missing, duplicated or malformed.
pub fn find_slot(digest: &Digest) -> Result<Slot, DigestError> {
    let mut found = None;
    for item in digest.logs() {
        if let DigestItem::PreRuntime(id, data) = item
            && *id == ENGINE_ID
        {
            if found.is_some() {
                return Err(DigestError::Multiple);
            }
            found = Some(Slot::decode(&mut &data[..]).map_err(|_| DigestError::Malformed)?);
        }
    }
    found.ok_or(DigestError::Missing)
}

sp_api::decl_runtime_apis! {
    /// Aura-PQ parameters read by the node.
    pub trait AuraPqApi {
        /// Slot duration in milliseconds.
        fn slot_duration() -> u64;
        /// Current authority set, in slot-assignment order.
        fn authorities() -> Vec<PqPublicKey>;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ac_crypto::sig::{SecretSeed, SigningKey};

    fn key(alg: SigAlg, seed: u8) -> PqPublicKey {
        SigningKey::from_seed(alg, &SecretSeed::new([seed; 32]))
            .unwrap()
            .public_key()
            .unwrap()
    }

    // consensus/aura-pq Requirement "授权节点集合来自创世": validation rules.
    #[test]
    fn authority_validation() {
        let a = key(SigAlg::MlDsa65, 1);
        let b = key(SigAlg::MlDsa65, 2);
        assert_eq!(validate_authorities(&[a.clone(), b.clone()], 100), Ok(()));
        assert_eq!(validate_authorities(&[], 100), Err(AuthorityError::Empty));
        assert_eq!(
            validate_authorities(&[a.clone(), b.clone()], 1),
            Err(AuthorityError::TooMany)
        );
        assert_eq!(
            validate_authorities(&[a.clone(), b, a.clone()], 100),
            Err(AuthorityError::Duplicate)
        );
        assert_eq!(
            validate_authorities(&[a, key(SigAlg::MlDsa44, 3)], 100),
            Err(AuthorityError::WrongAlgorithm)
        );
    }

    // Requirement "时隙与出块人": slot s belongs to authority s mod N.
    #[test]
    fn round_robin_assignment() {
        let set: Vec<_> = (1..=3).map(|i| key(SigAlg::MlDsa65, i)).collect();
        for s in 0u64..30 {
            let expected = &set[usize::try_from(s % 3).unwrap()];
            assert_eq!(slot_author(Slot::from(s), &set), Some(expected));
        }
        assert_eq!(slot_author(Slot::from(5), &[]), None);
    }

    #[test]
    fn digest_round_trip_and_errors() {
        let mut digest = Digest::default();
        assert_eq!(find_slot(&digest), Err(DigestError::Missing));
        digest.push(pre_digest(Slot::from(42)));
        assert_eq!(find_slot(&digest), Ok(Slot::from(42)));
        digest.push(pre_digest(Slot::from(43)));
        assert_eq!(find_slot(&digest), Err(DigestError::Multiple));
        let mut bad = Digest::default();
        bad.push(DigestItem::PreRuntime(ENGINE_ID, alloc::vec![1]));
        assert_eq!(find_slot(&bad), Err(DigestError::Malformed));
    }
}
