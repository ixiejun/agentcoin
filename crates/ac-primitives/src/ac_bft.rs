//! AC-BFT finality primitives shared by the runtime, the node and light clients (plan §4.1,
//! design D2/D5 of `m2-finality`, decision D38 pending).
//!
//! Every consensus message is signed by a validator's ML-DSA-65 key with the context
//! [`VOTE_CONTEXT`] over [`signing_payload`], a domain-separated BLAKE3 hash that binds the
//! message version, the genesis hash and the authority-set id. Finality proofs are sets of
//! commit votes worth more than two thirds of the set's weight and can be verified with
//! [`verify_finality_proof`] from the authority set alone.
//!
//! Messages and proofs travel in versioned envelopes whose SCALE enum index is the format
//! version, so an unknown version fails to decode instead of being read as another format
//! (full plan §11, "finality-proof version number").

use alloc::vec::Vec;

use ac_crypto::{PqPublicKey, PqSignature};
use parity_scale_codec::{Decode, DecodeWithMemTracking, Encode, MaxEncodedLen};
use scale_info::TypeInfo;
use sp_core::{ConstU32, H256};
use sp_runtime::{BoundedVec, ConsensusEngineId, DigestItem};

use crate::aura_pq::MAX_AUTHORITIES;

/// Consensus engine ID of AC-BFT digests and justifications.
pub const ENGINE_ID: ConsensusEngineId = *b"acbf";

/// ML-DSA signing context of every AC-BFT message. Never change it.
pub const VOTE_CONTEXT: &[u8] = b"agentcoin/bft-vote/v1";

/// Hashing context of [`signing_payload`]. Never change it.
pub const MESSAGE_HASH_CONTEXT: &str = "agentcoin 2026-09 bft-message v1";

/// Current version of messages and finality proofs.
pub const FORMAT_VERSION: u8 = 1;

/// Authority-set id: 0 at genesis, incremented on every change.
pub type SetId = u64;
/// Voting round within one authority set; restarts at 0 when the set changes.
pub type Round = u64;
/// Position of a member in its authority set.
pub type AuthorityIndex = u16;
/// Block number type of the chain.
pub type BlockNumber = u32;
/// Upper bound on authority-set sizes carried in proofs and digests.
pub type MaxAuthorities = ConstU32<MAX_AUTHORITIES>;

/// One authority-set member. Weights are 1 during PoA; the field is reserved for stake
/// weighting (plan §4.3).
#[derive(
    Clone, Debug, PartialEq, Eq, Encode, Decode, DecodeWithMemTracking, TypeInfo, MaxEncodedLen,
)]
pub struct Authority {
    /// ML-DSA-65 public key used for block seals and AC-BFT messages.
    pub key: PqPublicKey,
    /// Voting weight.
    pub weight: u64,
}

impl Authority {
    /// A PoA member with weight 1.
    #[must_use]
    pub fn poa(key: PqPublicKey) -> Self {
        Self { key, weight: 1 }
    }
}

/// A block by hash and number.
#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Encode,
    Decode,
    DecodeWithMemTracking,
    TypeInfo,
    MaxEncodedLen,
)]
pub struct BlockRef {
    /// Block hash.
    pub hash: H256,
    /// Block number.
    pub number: BlockNumber,
}

/// Reference to a certificate (q votes of one kind for one target in one round). Votes are
/// broadcast to everyone, so a reference is enough: each node checks it has seen the votes.
#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Encode,
    Decode,
    DecodeWithMemTracking,
    TypeInfo,
    MaxEncodedLen,
)]
pub struct CertRef {
    /// Round of the certificate.
    pub round: Round,
    /// Target of the certificate.
    pub target: BlockRef,
}

/// Phase of a vote. Wire format: explicit discriminants.
#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Encode,
    Decode,
    DecodeWithMemTracking,
    TypeInfo,
    MaxEncodedLen,
)]
pub enum VoteKind {
    /// First phase.
    #[codec(index = 0)]
    Prepare,
    /// Second phase; a certificate of commits finalizes the target.
    #[codec(index = 1)]
    Commit,
}

/// An AC-BFT message (format version 1). Wire format: explicit discriminants.
#[derive(
    Clone, Debug, PartialEq, Eq, Encode, Decode, DecodeWithMemTracking, TypeInfo, MaxEncodedLen,
)]
pub enum Message {
    /// Leader's proposal for a round.
    #[codec(index = 0)]
    Proposal {
        /// Round.
        round: Round,
        /// Proposed block.
        target: BlockRef,
        /// Highest prepare certificate the leader knows, which `target` extends.
        justify: Option<CertRef>,
    },
    /// A prepare or commit vote.
    #[codec(index = 1)]
    Vote {
        /// Phase.
        kind: VoteKind,
        /// Round.
        round: Round,
        /// Voted block.
        target: BlockRef,
    },
    /// Round timeout, carrying the sender's highest prepare certificate.
    #[codec(index = 2)]
    Timeout {
        /// Round that timed out.
        round: Round,
        /// Highest prepare certificate the sender knows.
        high: Option<CertRef>,
    },
}

/// Message type, as used for equivocation checks.
#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Encode,
    Decode,
    DecodeWithMemTracking,
    TypeInfo,
    MaxEncodedLen,
)]
pub enum MessageKind {
    /// [`Message::Proposal`].
    #[codec(index = 0)]
    Proposal,
    /// Prepare vote.
    #[codec(index = 1)]
    Prepare,
    /// Commit vote.
    #[codec(index = 2)]
    Commit,
    /// [`Message::Timeout`].
    #[codec(index = 3)]
    Timeout,
}

impl Message {
    /// The message's round.
    #[must_use]
    pub fn round(&self) -> Round {
        match self {
            Self::Proposal { round, .. }
            | Self::Vote { round, .. }
            | Self::Timeout { round, .. } => *round,
        }
    }

    /// The message's type.
    #[must_use]
    pub fn kind(&self) -> MessageKind {
        match self {
            Self::Proposal { .. } => MessageKind::Proposal,
            Self::Vote {
                kind: VoteKind::Prepare,
                ..
            } => MessageKind::Prepare,
            Self::Vote {
                kind: VoteKind::Commit,
                ..
            } => MessageKind::Commit,
            Self::Timeout { .. } => MessageKind::Timeout,
        }
    }

    /// The block the message is about, if any.
    #[must_use]
    pub fn target(&self) -> Option<BlockRef> {
        match self {
            Self::Proposal { target, .. } | Self::Vote { target, .. } => Some(*target),
            Self::Timeout { .. } => None,
        }
    }
}

/// A signed AC-BFT message (format version 1).
#[derive(
    Clone, Debug, PartialEq, Eq, Encode, Decode, DecodeWithMemTracking, TypeInfo, MaxEncodedLen,
)]
pub struct SignedMessage {
    /// Authority set the message belongs to.
    pub set_id: SetId,
    /// Signer's index in that set.
    pub signer: AuthorityIndex,
    /// The message.
    pub message: Message,
    /// ML-DSA signature over [`signing_payload`] with context [`VOTE_CONTEXT`].
    pub signature: PqSignature,
}

/// Versioned wire envelope of a signed message; the enum index is the format version.
#[derive(
    Clone, Debug, PartialEq, Eq, Encode, Decode, DecodeWithMemTracking, TypeInfo, MaxEncodedLen,
)]
pub enum VersionedMessage {
    /// Format version 1.
    #[codec(index = 1)]
    V1(SignedMessage),
}

/// Why an AC-BFT message or proof is invalid.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BftError {
    /// The authority-set id does not match.
    WrongSet,
    /// The signer index is outside the set.
    UnknownSigner,
    /// A signature does not verify.
    BadSignature,
    /// A proof lists the same signer twice or out of order.
    DuplicateSigner,
    /// The signers' weight does not exceed two thirds of the set's weight.
    InsufficientWeight,
    /// The set is empty or its weights overflow.
    InvalidSet,
    /// A hashing context was rejected (never happens with the fixed contexts).
    Hashing,
}

impl core::fmt::Display for BftError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match self {
            Self::WrongSet => "authority-set id mismatch",
            Self::UnknownSigner => "signer is not in the authority set",
            Self::BadSignature => "invalid signature",
            Self::DuplicateSigner => "signers must be listed once, in increasing order",
            Self::InsufficientWeight => "not enough voting weight",
            Self::InvalidSet => "invalid authority set",
            Self::Hashing => "hashing context rejected",
        })
    }
}

/// Minimum weight that exceeds two thirds of `total`: `⌊2·total/3⌋ + 1`. For unit weights this
/// is the vote threshold q; `n ≥ 3f + 1` members tolerate `f` faulty ones.
#[must_use]
pub fn threshold(total: u128) -> u128 {
    // u128 cannot overflow here: weights are u64 and there are at most 2^16 members.
    total
        .saturating_mul(2)
        .checked_div(3)
        .unwrap_or(0)
        .saturating_add(1)
}

/// Total weight of a set; `None` if empty.
#[must_use]
pub fn total_weight(authorities: &[Authority]) -> Option<u128> {
    if authorities.is_empty() {
        return None;
    }
    Some(
        authorities
            .iter()
            .fold(0u128, |acc, a| acc.saturating_add(u128::from(a.weight))),
    )
}

/// The 32-byte payload signed for `message` in set `set_id` of the chain `genesis`.
///
/// # Errors
///
/// [`BftError::Hashing`] if the hashing context were rejected (it is a fixed, valid context).
pub fn signing_payload(
    genesis: &H256,
    set_id: SetId,
    message: &Message,
) -> Result<[u8; 32], BftError> {
    let data = (FORMAT_VERSION, genesis, set_id, message).encode();
    ac_crypto::hash::derive(MESSAGE_HASH_CONTEXT, &data).map_err(|_| BftError::Hashing)
}

/// Checks that `signed` belongs to set `set_id` and carries a valid signature by its signer.
///
/// # Errors
///
/// [`BftError::WrongSet`], [`BftError::UnknownSigner`] or [`BftError::BadSignature`].
pub fn verify_message(
    genesis: &H256,
    set_id: SetId,
    authorities: &[Authority],
    signed: &SignedMessage,
) -> Result<(), BftError> {
    if signed.set_id != set_id {
        return Err(BftError::WrongSet);
    }
    let signer = authorities
        .get(usize::from(signed.signer))
        .ok_or(BftError::UnknownSigner)?;
    let payload = signing_payload(genesis, set_id, &signed.message)?;
    ac_crypto::sig::verify(&signer.key, &payload, VOTE_CONTEXT, &signed.signature)
        .map_err(|_| BftError::BadSignature)
}

/// Finality proof (format version 1): commit votes on `target` in `round`, worth more than
/// two thirds of set `set_id`'s weight.
#[derive(
    Clone, Debug, PartialEq, Eq, Encode, Decode, DecodeWithMemTracking, TypeInfo, MaxEncodedLen,
)]
pub struct FinalityProof {
    /// Authority set that voted.
    pub set_id: SetId,
    /// Round of the commit votes.
    pub round: Round,
    /// Finalized block.
    pub target: BlockRef,
    /// `(signer index, signature)` in strictly increasing index order.
    pub commits: BoundedVec<(AuthorityIndex, PqSignature), MaxAuthorities>,
}

/// Versioned finality proof; the enum index is the format version. This is the justification
/// stored under [`ENGINE_ID`].
#[derive(
    Clone, Debug, PartialEq, Eq, Encode, Decode, DecodeWithMemTracking, TypeInfo, MaxEncodedLen,
)]
pub enum VersionedFinalityProof {
    /// Format version 1.
    #[codec(index = 1)]
    V1(FinalityProof),
}

/// Verifies a finality proof against the authority set it names and returns the finalized
/// block. Needs nothing but the genesis hash and the set, so light clients can use it.
///
/// # Errors
///
/// [`BftError`] describing the first failed check.
pub fn verify_finality_proof(
    genesis: &H256,
    set_id: SetId,
    authorities: &[Authority],
    proof: &VersionedFinalityProof,
) -> Result<BlockRef, BftError> {
    let VersionedFinalityProof::V1(proof) = proof;
    if proof.set_id != set_id {
        return Err(BftError::WrongSet);
    }
    let total = total_weight(authorities).ok_or(BftError::InvalidSet)?;
    let message = Message::Vote {
        kind: VoteKind::Commit,
        round: proof.round,
        target: proof.target,
    };
    let payload = signing_payload(genesis, set_id, &message)?;
    let mut weight = 0u128;
    let mut previous: Option<AuthorityIndex> = None;
    for (index, signature) in &proof.commits {
        if previous.is_some_and(|p| p >= *index) {
            return Err(BftError::DuplicateSigner);
        }
        previous = Some(*index);
        let signer = authorities
            .get(usize::from(*index))
            .ok_or(BftError::UnknownSigner)?;
        ac_crypto::sig::verify(&signer.key, &payload, VOTE_CONTEXT, signature)
            .map_err(|_| BftError::BadSignature)?;
        weight = weight.saturating_add(u128::from(signer.weight));
    }
    if weight < threshold(total) {
        return Err(BftError::InsufficientWeight);
    }
    Ok(proof.target)
}

/// Authority-set change announced in a boundary block's header.
#[derive(
    Clone, Debug, PartialEq, Eq, Encode, Decode, DecodeWithMemTracking, TypeInfo, MaxEncodedLen,
)]
pub struct ScheduledChange {
    /// Id of the new set.
    pub set_id: SetId,
    /// Members of the new set, in order.
    pub authorities: BoundedVec<Authority, MaxAuthorities>,
}

/// AC-BFT header digests. Wire format: explicit discriminants.
#[derive(
    Clone, Debug, PartialEq, Eq, Encode, Decode, DecodeWithMemTracking, TypeInfo, MaxEncodedLen,
)]
pub enum ConsensusLog {
    /// The authority set changes after this block.
    #[codec(index = 1)]
    ScheduledChange(ScheduledChange),
}

impl ConsensusLog {
    /// As a header digest item.
    #[must_use]
    pub fn to_digest(&self) -> DigestItem {
        DigestItem::Consensus(ENGINE_ID, self.encode())
    }

    /// Finds the AC-BFT scheduled change in a digest item list, ignoring malformed items.
    #[must_use]
    pub fn find_change<'a>(
        logs: impl IntoIterator<Item = &'a DigestItem>,
    ) -> Option<ScheduledChange> {
        logs.into_iter().find_map(|item| match item {
            DigestItem::Consensus(id, data) if *id == ENGINE_ID => {
                match Self::decode(&mut &data[..]) {
                    Ok(Self::ScheduledChange(change)) => Some(change),
                    Err(_) => None,
                }
            }
            _ => None,
        })
    }
}

/// Keys of a set, in order.
#[must_use]
pub fn keys(authorities: &[Authority]) -> Vec<PqPublicKey> {
    authorities.iter().map(|a| a.key.clone()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ac_crypto::SigAlg;
    use ac_crypto::sig::{SecretSeed, SigningKey};

    fn signer(seed: u8) -> SigningKey {
        SigningKey::from_seed(SigAlg::MlDsa65, &SecretSeed::new([seed; 32])).unwrap()
    }

    fn set(n: u8) -> (Vec<SigningKey>, Vec<Authority>) {
        let keys: Vec<_> = (1..=n).map(signer).collect();
        let set = keys
            .iter()
            .map(|k| Authority::poa(k.public_key().unwrap()))
            .collect();
        (keys, set)
    }

    fn block(n: u8) -> BlockRef {
        BlockRef {
            hash: H256::repeat_byte(n),
            number: u32::from(n),
        }
    }

    fn sign(
        key: &SigningKey,
        genesis: &H256,
        set_id: SetId,
        index: u16,
        message: Message,
    ) -> SignedMessage {
        let payload = signing_payload(genesis, set_id, &message).unwrap();
        SignedMessage {
            set_id,
            signer: index,
            message,
            signature: key.sign_deterministic(&payload, VOTE_CONTEXT).unwrap(),
        }
    }

    fn proof(
        keys: &[SigningKey],
        signers: &[u16],
        genesis: &H256,
        round: Round,
        target: BlockRef,
    ) -> VersionedFinalityProof {
        let message = Message::Vote {
            kind: VoteKind::Commit,
            round,
            target,
        };
        let payload = signing_payload(genesis, 0, &message).unwrap();
        let commits = signers
            .iter()
            .map(|i| {
                let key = &keys[usize::from(*i)];
                (*i, key.sign_deterministic(&payload, VOTE_CONTEXT).unwrap())
            })
            .collect::<Vec<_>>();
        VersionedFinalityProof::V1(FinalityProof {
            set_id: 0,
            round,
            target,
            commits: BoundedVec::try_from(commits).unwrap(),
        })
    }

    // consensus/ac-bft Requirement "两阶段投票与阈值": q = ⌊2n/3⌋ + 1.
    #[test]
    fn thresholds() {
        for (n, q) in [(1u128, 1u128), (3, 3), (4, 3), (7, 5), (10, 7), (100, 67)] {
            assert_eq!(threshold(n), q, "n = {n}");
        }
        assert_eq!(total_weight(&[]), None);
    }

    // Requirement "签名的共识消息": the payload binds every field.
    #[test]
    fn payload_binds_every_field() {
        let g = H256::repeat_byte(9);
        let vote = |kind, round, target| Message::Vote {
            kind,
            round,
            target,
        };
        let base = signing_payload(&g, 0, &vote(VoteKind::Prepare, 3, block(1))).unwrap();
        let variants = [
            signing_payload(
                &H256::repeat_byte(8),
                0,
                &vote(VoteKind::Prepare, 3, block(1)),
            ),
            signing_payload(&g, 1, &vote(VoteKind::Prepare, 3, block(1))),
            signing_payload(&g, 0, &vote(VoteKind::Commit, 3, block(1))),
            signing_payload(&g, 0, &vote(VoteKind::Prepare, 4, block(1))),
            signing_payload(&g, 0, &vote(VoteKind::Prepare, 3, block(2))),
            signing_payload(
                &g,
                0,
                &Message::Proposal {
                    round: 3,
                    target: block(1),
                    justify: None,
                },
            ),
        ];
        for v in variants {
            assert_ne!(v.unwrap(), base);
        }
    }

    // Scenarios "有效投票被接受和转发" / "篡改的投票被丢弃" / "其他链的投票无效" (verification part).
    #[test]
    fn message_verification() {
        let (keys, set) = set(4);
        let g = H256::repeat_byte(9);
        let vote = Message::Vote {
            kind: VoteKind::Prepare,
            round: 0,
            target: block(1),
        };
        let signed = sign(&keys[2], &g, 0, 2, vote);
        assert_eq!(verify_message(&g, 0, &set, &signed), Ok(()));

        let mut tampered = signed.clone();
        tampered.message = Message::Vote {
            kind: VoteKind::Prepare,
            round: 0,
            target: block(2),
        };
        assert_eq!(
            verify_message(&g, 0, &set, &tampered),
            Err(BftError::BadSignature)
        );
        assert_eq!(
            verify_message(&H256::repeat_byte(1), 0, &set, &signed),
            Err(BftError::BadSignature)
        );
        assert_eq!(
            verify_message(&g, 1, &set, &signed),
            Err(BftError::WrongSet)
        );
        let mut wrong_signer = signed.clone();
        wrong_signer.signer = 1;
        assert_eq!(
            verify_message(&g, 0, &set, &wrong_signer),
            Err(BftError::BadSignature)
        );
        wrong_signer.signer = 7;
        assert_eq!(
            verify_message(&g, 0, &set, &wrong_signer),
            Err(BftError::UnknownSigner)
        );
    }

    // Scenario "未知版本号": unknown versions fail to decode, never fall back.
    #[test]
    fn unknown_versions_do_not_decode() {
        let (keys, set) = set(4);
        let g = H256::zero();
        let signed = sign(
            &keys[0],
            &g,
            0,
            0,
            Message::Timeout {
                round: 1,
                high: None,
            },
        );
        let mut wire = VersionedMessage::V1(signed.clone()).encode();
        assert_eq!(wire[0], FORMAT_VERSION);
        assert_eq!(
            VersionedMessage::decode(&mut &wire[..]).unwrap(),
            VersionedMessage::V1(signed)
        );
        wire[0] = 2;
        assert!(VersionedMessage::decode(&mut &wire[..]).is_err());

        let mut wire = proof(&keys, &[0, 1, 2], &g, 0, block(1)).encode();
        assert!(
            verify_finality_proof(
                &g,
                0,
                &set,
                &VersionedFinalityProof::decode(&mut &wire[..]).unwrap()
            )
            .is_ok()
        );
        wire[0] = 0;
        assert!(VersionedFinalityProof::decode(&mut &wire[..]).is_err());
    }

    // Requirement "终局性证明": valid proofs and Scenario "票数不足的证明无效".
    #[test]
    fn finality_proofs() {
        let (keys, set) = set(4);
        let g = H256::repeat_byte(3);
        let good = proof(&keys, &[0, 2, 3], &g, 5, block(7));
        assert_eq!(verify_finality_proof(&g, 0, &set, &good), Ok(block(7)));
        assert_eq!(
            verify_finality_proof(&g, 0, &set, &proof(&keys, &[0, 1, 2, 3], &g, 5, block(7))),
            Ok(block(7))
        );

        let short = proof(&keys, &[0, 3], &g, 5, block(7));
        assert_eq!(
            verify_finality_proof(&g, 0, &set, &short),
            Err(BftError::InsufficientWeight)
        );
        let dup = proof(&keys, &[0, 0, 3], &g, 5, block(7));
        assert_eq!(
            verify_finality_proof(&g, 0, &set, &dup),
            Err(BftError::DuplicateSigner)
        );
        let unordered = proof(&keys, &[3, 0, 2], &g, 5, block(7));
        assert_eq!(
            verify_finality_proof(&g, 0, &set, &unordered),
            Err(BftError::DuplicateSigner)
        );
        assert_eq!(
            verify_finality_proof(&g, 1, &set, &good),
            Err(BftError::WrongSet)
        );
        assert_eq!(
            verify_finality_proof(&H256::zero(), 0, &set, &good),
            Err(BftError::BadSignature)
        );
        let VersionedFinalityProof::V1(mut changed) = good.clone();
        changed.target = block(8);
        assert_eq!(
            verify_finality_proof(&g, 0, &set, &VersionedFinalityProof::V1(changed)),
            Err(BftError::BadSignature)
        );
        let VersionedFinalityProof::V1(mut outside) = good;
        outside.commits =
            BoundedVec::try_from(alloc::vec![(9u16, outside.commits[0].1.clone())]).unwrap();
        assert_eq!(
            verify_finality_proof(&g, 0, &set, &VersionedFinalityProof::V1(outside)),
            Err(BftError::UnknownSigner)
        );
        assert_eq!(
            verify_finality_proof(&g, 0, &[], &short),
            Err(BftError::InvalidSet)
        );
    }

    #[test]
    fn scheduled_change_digest() {
        let (_, set) = set(3);
        let change = ScheduledChange {
            set_id: 1,
            authorities: BoundedVec::try_from(set).unwrap(),
        };
        let item = ConsensusLog::ScheduledChange(change.clone()).to_digest();
        let other = DigestItem::Consensus(*b"xxxx", alloc::vec![1]);
        let malformed = DigestItem::Consensus(ENGINE_ID, alloc::vec![9]);
        assert_eq!(
            ConsensusLog::find_change([&other, &malformed, &item]),
            Some(change)
        );
        assert_eq!(ConsensusLog::find_change([&other]), None);
    }
}
