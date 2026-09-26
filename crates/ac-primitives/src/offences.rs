//! Double-signing evidence shared by the runtime and the node (plan §4.1, design D7 of
//! `m2-finality`).
//!
//! Two kinds of evidence prove an offence on their own, without trusting the reporter:
//! - [`Evidence::AuraEquivocation`]: two different headers sealed by one key in one slot;
//! - [`Evidence::BftEquivocation`]: two different AC-BFT messages signed by one member of one
//!   set in one round, of the same kind (proposal, prepare or commit — timeouts are harmless
//!   and never count).
//!
//! [`verify_evidence`] checks every signature; the caller supplies the historical authority
//! sets it still keeps and decides how old evidence may be.

use alloc::boxed::Box;

use ac_crypto::{PqPublicKey, PqSignature};
use parity_scale_codec::{Decode, DecodeWithMemTracking, Encode, MaxEncodedLen};
use scale_info::TypeInfo;
use sp_core::{ConstU32, H256};
use sp_runtime::traits::Header as HeaderT;
use sp_runtime::{BoundedVec, DigestItem, generic};

use crate::Blake3Hasher;
use crate::ac_bft::{self, Authority, AuthorityIndex, MessageKind, Round, SetId, SignedMessage};
use crate::aura_pq::{self, SEAL_CONTEXT, Slot};

/// Maximum encoded size of a header carried in evidence.
pub const MAX_HEADER_LEN: u32 = 16 * 1024;

/// SCALE-encoded, sealed chain header.
pub type EncodedHeader = BoundedVec<u8, ConstU32<MAX_HEADER_LEN>>;

/// The chain's header type (block number `u32`, BLAKE3 hashing).
pub type ChainHeader = generic::Header<u32, Blake3Hasher>;

/// Proof that a validator signed two conflicting things. Wire format: explicit discriminants.
#[derive(
    Clone, Debug, PartialEq, Eq, Encode, Decode, DecodeWithMemTracking, TypeInfo, MaxEncodedLen,
)]
pub enum Evidence {
    /// Two different headers sealed by `offender` in the same slot.
    #[codec(index = 0)]
    AuraEquivocation {
        /// Key that sealed both headers.
        offender: PqPublicKey,
        /// First sealed header.
        first: EncodedHeader,
        /// Second sealed header.
        second: EncodedHeader,
    },
    /// Two different AC-BFT messages from one member in one set, round and kind.
    #[codec(index = 1)]
    BftEquivocation {
        /// First message.
        first: Box<SignedMessage>,
        /// Second message.
        second: Box<SignedMessage>,
    },
}

/// Identifies one offence, so it is recorded once. Wire format: explicit discriminants.
#[derive(
    Clone, Debug, PartialEq, Eq, Encode, Decode, DecodeWithMemTracking, TypeInfo, MaxEncodedLen,
)]
pub enum OffenceKey {
    /// Block-seal double signing.
    #[codec(index = 0)]
    Aura {
        /// Offender's key.
        offender: PqPublicKey,
        /// Slot of both headers.
        slot: u64,
    },
    /// AC-BFT double signing.
    #[codec(index = 1)]
    Bft {
        /// Authority set.
        set_id: SetId,
        /// Member index in that set.
        signer: AuthorityIndex,
        /// Round.
        round: Round,
        /// Message type.
        kind: MessageKind,
    },
}

/// A verified offence.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Offence {
    /// Key of the validator that double-signed.
    pub offender: PqPublicKey,
    /// Identity of the offence.
    pub key: OffenceKey,
    /// Set the offence happened in (`None` for seal double signing, which carries no set id).
    pub set_id: Option<SetId>,
}

/// Why evidence is rejected.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EvidenceError {
    /// A header does not decode completely.
    MalformedHeader,
    /// A header has no valid seal or slot.
    MalformedSeal,
    /// The two headers are in different slots.
    DifferentSlots,
    /// Both items are identical, so nothing conflicts.
    NotConflicting,
    /// A signature does not verify.
    BadSignature,
    /// The key or set is unknown or no longer kept.
    UnknownAuthority,
    /// The messages differ in set, signer, round or kind.
    Mismatch,
    /// Timeout messages never constitute an offence.
    TimeoutMessages,
}

impl core::fmt::Display for EvidenceError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match self {
            Self::MalformedHeader => "malformed header",
            Self::MalformedSeal => "header seal or slot missing or malformed",
            Self::DifferentSlots => "headers are in different slots",
            Self::NotConflicting => "items are identical",
            Self::BadSignature => "invalid signature",
            Self::UnknownAuthority => "unknown or expired authority set",
            Self::Mismatch => "messages differ in set, signer, round or kind",
            Self::TimeoutMessages => "timeout messages are not an offence",
        })
    }
}

/// Historical authority sets available to the verifier.
pub trait AuthoritySets {
    /// Members of set `set_id`, if still kept.
    fn set(&self, set_id: SetId) -> Option<alloc::vec::Vec<Authority>>;
    /// Whether `key` belongs to any kept set.
    fn contains(&self, key: &PqPublicKey) -> bool;
}

/// Verifies evidence and returns the offence it proves. Age limits (how old a slot or set may
/// be) are enforced by the caller through what [`AuthoritySets`] still keeps.
///
/// # Errors
///
/// [`EvidenceError`] describing the first failed check.
pub fn verify_evidence(
    genesis: &H256,
    evidence: &Evidence,
    sets: &impl AuthoritySets,
) -> Result<Offence, EvidenceError> {
    match evidence {
        Evidence::AuraEquivocation {
            offender,
            first,
            second,
        } => {
            if first == second {
                return Err(EvidenceError::NotConflicting);
            }
            if !sets.contains(offender) {
                return Err(EvidenceError::UnknownAuthority);
            }
            let (slot_a, hash_a) = checked_seal(first, offender)?;
            let (slot_b, hash_b) = checked_seal(second, offender)?;
            if slot_a != slot_b {
                return Err(EvidenceError::DifferentSlots);
            }
            // A different seal over the same pre-seal header is not two blocks.
            if hash_a == hash_b {
                return Err(EvidenceError::NotConflicting);
            }
            Ok(Offence {
                offender: offender.clone(),
                key: OffenceKey::Aura {
                    offender: offender.clone(),
                    slot: slot_a.into(),
                },
                set_id: None,
            })
        }
        Evidence::BftEquivocation { first, second } => {
            if first.set_id != second.set_id
                || first.signer != second.signer
                || first.message.round() != second.message.round()
                || first.message.kind() != second.message.kind()
            {
                return Err(EvidenceError::Mismatch);
            }
            if first.message.kind() == MessageKind::Timeout {
                return Err(EvidenceError::TimeoutMessages);
            }
            if first.message == second.message {
                return Err(EvidenceError::NotConflicting);
            }
            let set = sets
                .set(first.set_id)
                .ok_or(EvidenceError::UnknownAuthority)?;
            for signed in [first.as_ref(), second.as_ref()] {
                ac_bft::verify_message(genesis, first.set_id, &set, signed)
                    .map_err(|_| EvidenceError::BadSignature)?;
            }
            let offender = set
                .get(usize::from(first.signer))
                .ok_or(EvidenceError::UnknownAuthority)?
                .key
                .clone();
            Ok(Offence {
                offender,
                key: OffenceKey::Bft {
                    set_id: first.set_id,
                    signer: first.signer,
                    round: first.message.round(),
                    kind: first.message.kind(),
                },
                set_id: Some(first.set_id),
            })
        }
    }
}

/// Decodes a sealed header, checks its seal against `key` and returns its slot and pre-seal
/// hash.
fn checked_seal(encoded: &[u8], key: &PqPublicKey) -> Result<(Slot, H256), EvidenceError> {
    let mut input = encoded;
    let mut header = ChainHeader::decode(&mut input).map_err(|_| EvidenceError::MalformedHeader)?;
    if !input.is_empty() {
        return Err(EvidenceError::MalformedHeader);
    }
    let signature = match header.digest_mut().pop() {
        Some(DigestItem::Seal(id, data)) if id == aura_pq::ENGINE_ID => {
            PqSignature::decode(&mut &data[..]).map_err(|_| EvidenceError::MalformedSeal)?
        }
        _ => return Err(EvidenceError::MalformedSeal),
    };
    let slot = aura_pq::find_slot(header.digest()).map_err(|_| EvidenceError::MalformedSeal)?;
    let pre_hash = header.hash();
    ac_crypto::sig::verify(key, pre_hash.as_ref(), SEAL_CONTEXT, &signature)
        .map_err(|_| EvidenceError::BadSignature)?;
    Ok((slot, pre_hash))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ac_bft::{BlockRef, Message, VOTE_CONTEXT, VoteKind, signing_payload};
    use ac_crypto::SigAlg;
    use ac_crypto::sig::{SecretSeed, SigningKey};
    use alloc::vec::Vec;
    use sp_runtime::Digest;

    struct Sets(Vec<Authority>);

    impl AuthoritySets for Sets {
        fn set(&self, set_id: SetId) -> Option<Vec<Authority>> {
            (set_id == 0).then(|| self.0.clone())
        }
        fn contains(&self, key: &PqPublicKey) -> bool {
            self.0.iter().any(|a| &a.key == key)
        }
    }

    fn keys() -> Vec<SigningKey> {
        (1u8..=4)
            .map(|i| SigningKey::from_seed(SigAlg::MlDsa65, &SecretSeed::new([i; 32])).unwrap())
            .collect()
    }

    fn sets(keys: &[SigningKey]) -> Sets {
        Sets(
            keys.iter()
                .map(|k| Authority::poa(k.public_key().unwrap()))
                .collect(),
        )
    }

    fn sealed(slot: u64, extrinsics_root: u8, key: &SigningKey) -> EncodedHeader {
        let mut digest = Digest::default();
        digest.push(aura_pq::pre_digest(Slot::from(slot)));
        let mut header = ChainHeader::new(
            1,
            H256::repeat_byte(extrinsics_root),
            H256::zero(),
            H256::zero(),
            digest,
        );
        let pre_hash = header.hash();
        let sig = key
            .sign_deterministic(pre_hash.as_ref(), SEAL_CONTEXT)
            .unwrap();
        header.digest_mut().push(aura_pq::seal_digest(&sig));
        EncodedHeader::try_from(header.encode()).unwrap()
    }

    fn vote(
        key: &SigningKey,
        signer: u16,
        kind: VoteKind,
        round: u64,
        target: u8,
    ) -> SignedMessage {
        let message = Message::Vote {
            kind,
            round,
            target: BlockRef {
                hash: H256::repeat_byte(target),
                number: 1,
            },
        };
        let payload = signing_payload(&H256::zero(), 0, &message).unwrap();
        SignedMessage {
            set_id: 0,
            signer,
            message,
            signature: key.sign_deterministic(&payload, VOTE_CONTEXT).unwrap(),
        }
    }

    // consensus/offences Scenario "有效的出块双签证据".
    #[test]
    fn aura_equivocation() {
        let keys = keys();
        let sets = sets(&keys);
        let offender = keys[1].public_key().unwrap();
        let evidence = Evidence::AuraEquivocation {
            offender: offender.clone(),
            first: sealed(9, 1, &keys[1]),
            second: sealed(9, 2, &keys[1]),
        };
        let offence = verify_evidence(&H256::zero(), &evidence, &sets).unwrap();
        assert_eq!(offence.offender, offender);
        assert_eq!(offence.key, OffenceKey::Aura { offender, slot: 9 });
    }

    // Scenarios "相同区块头不构成证据" and "伪造签名", plus other rejected shapes.
    #[test]
    fn invalid_aura_evidence() {
        let keys = keys();
        let sets = sets(&keys);
        let g = H256::zero();
        let pk = |i: usize| keys[i].public_key().unwrap();
        let ev = |offender, first, second| Evidence::AuraEquivocation {
            offender,
            first,
            second,
        };
        let a = sealed(9, 1, &keys[1]);
        assert_eq!(
            verify_evidence(&g, &ev(pk(1), a.clone(), a.clone()), &sets),
            Err(EvidenceError::NotConflicting)
        );
        assert_eq!(
            verify_evidence(&g, &ev(pk(1), a.clone(), sealed(10, 2, &keys[1])), &sets),
            Err(EvidenceError::DifferentSlots)
        );
        assert_eq!(
            verify_evidence(&g, &ev(pk(1), a.clone(), sealed(9, 2, &keys[2])), &sets),
            Err(EvidenceError::BadSignature)
        );
        let stranger = SigningKey::from_seed(SigAlg::MlDsa65, &SecretSeed::new([77; 32])).unwrap();
        assert_eq!(
            verify_evidence(
                &g,
                &ev(
                    stranger.public_key().unwrap(),
                    sealed(9, 1, &stranger),
                    sealed(9, 2, &stranger)
                ),
                &sets
            ),
            Err(EvidenceError::UnknownAuthority)
        );
        let mut trailing = a.clone().into_inner();
        trailing.push(0);
        assert_eq!(
            verify_evidence(
                &g,
                &ev(
                    pk(1),
                    EncodedHeader::try_from(trailing).unwrap(),
                    sealed(9, 2, &keys[1])
                ),
                &sets
            ),
            Err(EvidenceError::MalformedHeader)
        );
        // Same pre-seal header, two different seals (ML-DSA hedged signing): one block only.
        let mut digest = Digest::default();
        digest.push(aura_pq::pre_digest(Slot::from(9)));
        let base = ChainHeader::new(1, H256::zero(), H256::zero(), H256::zero(), digest);
        let reseal = |seed: u8| {
            let mut h = base.clone();
            let mut rng = TestRng(seed);
            let sig = keys[1]
                .sign(h.hash().as_ref(), SEAL_CONTEXT, &mut rng)
                .unwrap();
            h.digest_mut().push(aura_pq::seal_digest(&sig));
            EncodedHeader::try_from(h.encode()).unwrap()
        };
        let (x, y) = (reseal(1), reseal(2));
        assert_ne!(x, y);
        assert_eq!(
            verify_evidence(&g, &ev(pk(1), x, y), &sets),
            Err(EvidenceError::NotConflicting)
        );
    }

    // Scenario "有效的投票双签证据" and rejected BFT shapes.
    #[test]
    fn bft_equivocation() {
        let keys = keys();
        let sets = sets(&keys);
        let g = H256::zero();
        let a = vote(&keys[2], 2, VoteKind::Commit, 4, 1);
        let b = vote(&keys[2], 2, VoteKind::Commit, 4, 2);
        let ev = |first: &SignedMessage, second: &SignedMessage| Evidence::BftEquivocation {
            first: Box::new(first.clone()),
            second: Box::new(second.clone()),
        };
        let offence = verify_evidence(&g, &ev(&a, &b), &sets).unwrap();
        assert_eq!(offence.offender, keys[2].public_key().unwrap());
        assert_eq!(
            offence.key,
            OffenceKey::Bft {
                set_id: 0,
                signer: 2,
                round: 4,
                kind: MessageKind::Commit
            }
        );
        assert_eq!(
            verify_evidence(&g, &ev(&a, &a), &sets),
            Err(EvidenceError::NotConflicting)
        );
        let other_round = vote(&keys[2], 2, VoteKind::Commit, 5, 2);
        assert_eq!(
            verify_evidence(&g, &ev(&a, &other_round), &sets),
            Err(EvidenceError::Mismatch)
        );
        let prepare = vote(&keys[2], 2, VoteKind::Prepare, 4, 2);
        assert_eq!(
            verify_evidence(&g, &ev(&a, &prepare), &sets),
            Err(EvidenceError::Mismatch)
        );
        let mut forged = b.clone();
        forged.signature = a.signature.clone();
        assert_eq!(
            verify_evidence(&g, &ev(&a, &forged), &sets),
            Err(EvidenceError::BadSignature)
        );
        let mut old_set = a.clone();
        old_set.set_id = 7;
        let mut old_set_b = b.clone();
        old_set_b.set_id = 7;
        assert_eq!(
            verify_evidence(&g, &ev(&old_set, &old_set_b), &sets),
            Err(EvidenceError::UnknownAuthority)
        );
    }

    // Timeouts with different content are harmless and never an offence.
    #[test]
    fn timeouts_are_not_offences() {
        let keys = keys();
        let sets = sets(&keys);
        let timeout = |high: Option<u8>| {
            let message = Message::Timeout {
                round: 3,
                high: high.map(|t| ac_bft::CertRef {
                    round: 1,
                    target: BlockRef {
                        hash: H256::repeat_byte(t),
                        number: 1,
                    },
                }),
            };
            let payload = signing_payload(&H256::zero(), 0, &message).unwrap();
            SignedMessage {
                set_id: 0,
                signer: 0,
                message,
                signature: keys[0].sign_deterministic(&payload, VOTE_CONTEXT).unwrap(),
            }
        };
        let evidence = Evidence::BftEquivocation {
            first: Box::new(timeout(None)),
            second: Box::new(timeout(Some(1))),
        };
        assert_eq!(
            verify_evidence(&H256::zero(), &evidence, &sets),
            Err(EvidenceError::TimeoutMessages)
        );
    }

    /// Deterministic RNG for the hedged-signing test only.
    struct TestRng(u8);
    impl rand_core::TryRng for TestRng {
        type Error = core::convert::Infallible;
        fn try_next_u32(&mut self) -> Result<u32, Self::Error> {
            Ok(u32::from(self.0))
        }
        fn try_next_u64(&mut self) -> Result<u64, Self::Error> {
            Ok(u64::from(self.0))
        }
        fn try_fill_bytes(&mut self, dst: &mut [u8]) -> Result<(), Self::Error> {
            dst.fill(self.0);
            Ok(())
        }
    }
    impl rand_core::TryCryptoRng for TestRng {}
}
