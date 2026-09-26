//! Commit–reveal randomness shared by the runtime, the node and verifiers (plan §4.2, design D8
//! of `m2-finality`).
//!
//! Each validator commits to a secret in epoch `e` and reveals it in epoch `e + 1`; at the
//! boundary of epoch `e + 2` the chain publishes
//! `R(e) = derive_key(RANDOMNESS_CONTEXT, u64_le(e) ‖ reveals sorted by account ID)`.
//! Anyone can recompute `R(e)` from the published reveals with [`epoch_randomness`].
//!
//! **Known bias**: the last validator to reveal may withhold its reveal and so choose between
//! two outcomes; colluding validators amplify this. Use only for low-value purposes such as
//! audit sampling (full plan §2.4 removes the bias with a hash-based VDF).

use alloc::vec::Vec;

use parity_scale_codec::{Decode, DecodeWithMemTracking, Encode};
use scale_info::TypeInfo;
use sp_core::H256;
use sp_inherents::InherentIdentifier;

use crate::epoch::EpochIndex;

/// Inherent identifier of the author's commit / reveal data.
pub const INHERENT_IDENTIFIER: InherentIdentifier = *b"acrandcr";

/// Hashing context of epoch randomness. Never change it.
pub const RANDOMNESS_CONTEXT: &str = "agentcoin 2026-09 randomness v1";

/// Hashing context of per-subject values. Never change it.
pub const SUBJECT_CONTEXT: &str = "agentcoin 2026-09 randomness-subject v1";

/// Local inherent data of a block author: its secrets for the block's epoch and the previous
/// one. Only the commitment of `current` and the (already due) `previous` reach the block.
#[derive(Clone, PartialEq, Eq, Encode, Decode, DecodeWithMemTracking, TypeInfo)]
pub struct InherentSecrets {
    /// Epoch of the block being built.
    pub epoch: EpochIndex,
    /// Secret for `epoch`.
    pub current: [u8; 32],
    /// Secret for `epoch − 1`, if `epoch > 0`.
    pub previous: Option<[u8; 32]>,
}

impl core::fmt::Debug for InherentSecrets {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "InherentSecrets {{ epoch: {}, <redacted> }}", self.epoch)
    }
}

/// Why randomness cannot be computed.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RandomnessError {
    /// A hashing context was rejected (never happens with the fixed contexts).
    Hashing,
}

/// `R(e)` from the epoch's reveals, each paired with the revealer's 32-byte account ID. The
/// reveals are sorted by account ID here, so callers may pass them in any order. `None` when
/// there are no reveals: an epoch without reveals has no randomness.
///
/// # Errors
///
/// [`RandomnessError::Hashing`] only if the fixed context were rejected.
pub fn epoch_randomness(
    epoch: EpochIndex,
    reveals: &[([u8; 32], [u8; 32])],
) -> Result<Option<H256>, RandomnessError> {
    if reveals.is_empty() {
        return Ok(None);
    }
    let mut sorted: Vec<&([u8; 32], [u8; 32])> = reveals.iter().collect();
    sorted.sort_by(|a, b| a.0.cmp(&b.0));
    let mut data = Vec::with_capacity(8usize.saturating_add(sorted.len().saturating_mul(32)));
    data.extend_from_slice(&epoch.to_le_bytes());
    for (_, secret) in sorted {
        data.extend_from_slice(secret);
    }
    ac_crypto::hash::derive(RANDOMNESS_CONTEXT, &data)
        .map(|h| Some(H256(h)))
        .map_err(|_| RandomnessError::Hashing)
}

/// Value for `subject` derived from epoch randomness `r`; independent across subjects.
///
/// # Errors
///
/// [`RandomnessError::Hashing`] only if the fixed context were rejected.
pub fn subject_value(r: &H256, subject: &[u8]) -> Result<H256, RandomnessError> {
    let mut data = Vec::with_capacity(32usize.saturating_add(subject.len()));
    data.extend_from_slice(r.as_bytes());
    data.extend_from_slice(subject);
    ac_crypto::hash::derive(SUBJECT_CONTEXT, &data)
        .map(H256)
        .map_err(|_| RandomnessError::Hashing)
}

sp_api::decl_runtime_apis! {
    /// Epoch randomness queries.
    pub trait RandomnessApi {
        /// Latest available randomness and its epoch; `None` before the first one.
        fn latest() -> Option<(EpochIndex, H256)>;
        /// Value for `subject` from the latest randomness, with its epoch.
        fn random(subject: Vec<u8>) -> Option<(EpochIndex, H256)>;
        /// Randomness of `epoch`, if still kept.
        fn epoch_randomness(epoch: EpochIndex) -> Option<H256>;
        /// Published reveals of `epoch` (account ID, secret), while the epoch is still open.
        fn reveals(epoch: EpochIndex) -> Vec<([u8; 32], [u8; 32])>;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // chain/randomness Scenario "独立复算": order-independent, epoch-bound, none without reveals.
    #[test]
    fn randomness_is_recomputable() {
        let a = ([1u8; 32], [7u8; 32]);
        let b = ([2u8; 32], [8u8; 32]);
        let r = epoch_randomness(3, &[a, b]).unwrap().unwrap();
        assert_eq!(epoch_randomness(3, &[b, a]).unwrap(), Some(r));
        assert_ne!(epoch_randomness(4, &[a, b]).unwrap(), Some(r));
        assert_ne!(epoch_randomness(3, &[a]).unwrap(), Some(r));
        assert_eq!(epoch_randomness(3, &[]).unwrap(), None);
        assert!(ac_crypto::hash::validate_context(RANDOMNESS_CONTEXT).is_ok());
        assert!(ac_crypto::hash::validate_context(SUBJECT_CONTEXT).is_ok());
    }

    // Scenario "按主题派生".
    #[test]
    fn subjects_are_independent() {
        let r = H256::repeat_byte(5);
        let x = subject_value(&r, b"audit").unwrap();
        assert_ne!(x, subject_value(&r, b"sample").unwrap());
        assert_ne!(x, r);
        assert_eq!(x, subject_value(&r, b"audit").unwrap());
    }

    #[test]
    fn secrets_are_redacted_in_debug() {
        let s = InherentSecrets {
            epoch: 1,
            current: [0xAB; 32],
            previous: None,
        };
        let text = alloc::format!("{s:?}");
        assert!(!text.contains("ab") && !text.contains("171"));
    }
}
