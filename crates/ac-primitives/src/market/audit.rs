//! On-chain audit types (m6-audit-chain design D2–D10; spec `market/audit`).
//!
//! The audit pallet stores these types directly and `AuditApi` returns them, so clients decode
//! exactly what the chain stores. [`sample`] is the single implementation of the assignment and
//! reviewer draws, used by the runtime, the runtime API and wallets alike.
//!
//! A verdict never holds the prompt, the answer or the proofs (red line 6): only the receipt and,
//! for a failure, the commitment to the evidence, which stays off chain.

use alloc::vec::Vec;

use parity_scale_codec::{Decode, DecodeWithMemTracking, Encode, MaxEncodedLen};
use scale_info::TypeInfo;
use sp_core::{ConstU32, H256};
use sp_runtime::{BoundedVec, Perbill};

use super::records::UnlockingList;
use super::usd::{MICRO_USD_PER_USD, MicroUsd};

/// Index of an audit round: round `r` covers blocks `r × L + 1 ..= (r + 1) × L`.
pub type RoundIndex = u32;

/// Hashing context of the assignment draw: `seed ‖ provider ‖ u32_le(i)`.
pub const ASSIGN_CONTEXT: &str = "agentcoin 2026-10 audit-assign v1";
/// Hashing context of the reviewer draw: `seed ‖ provider ‖ u64_le(dispute) ‖ u32_le(i)`.
pub const REVIEW_CONTEXT: &str = "agentcoin 2026-10 audit-review v1";
/// Subject of a round's seed in the chain's randomness: this prefix ‖ `u32_le(round)`.
pub const ROUND_SEED_SUBJECT: &[u8] = b"agentcoin/audit-round";

/// Most registered auditors (design D2).
pub const MAX_AUDITORS: u32 = 1_000;
/// Most auditors drawn per provider and round (guardrail of `assign`).
pub const MAX_ASSIGN: u8 = 8;
/// Most reviewers of a dispute (guardrail of `reviewers`).
pub const MAX_REVIEWERS: u8 = 15;
/// Most accusers of a dispute: the failing verdicts of two rounds.
pub const MAX_ACCUSERS: u32 = 2 * MAX_ASSIGN as u32;

/// Lowest and highest auditor stake an administration may set, in micro-dollars.
pub const STAKE_USD_BOUNDS: (MicroUsd, MicroUsd) = (
    MicroUsd(100 * MICRO_USD_PER_USD),
    MicroUsd(100_000 * MICRO_USD_PER_USD),
);
/// Highest payment per verdict or vote an administration may set, in micro-dollars.
pub const MAX_PAYMENT_USD: MicroUsd = MicroUsd(MICRO_USD_PER_USD);
/// Lowest slash ratio of the genesis guardrails.
pub const MIN_SLASH: Perbill = Perbill::from_percent(1);

/// The bound a chunk exceeded (mirrors `ac-market-proto`'s `Metric`).
///
/// Wire-format enum: discriminants are explicit and never reused.
#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    Encode,
    Decode,
    DecodeWithMemTracking,
    MaxEncodedLen,
    TypeInfo,
)]
#[repr(u8)]
pub enum AuditMetric {
    /// Too many exponent mismatches.
    #[codec(index = 0)]
    ExpMismatches = 0,
    /// No top-k position has the proof's exponent.
    #[codec(index = 1)]
    NoMatchingExponent = 1,
    /// The mean mantissa error is too large.
    #[codec(index = 2)]
    MantissaMean = 2,
    /// The median mantissa error is too large.
    #[codec(index = 3)]
    MantissaMedian = 3,
    /// There was no chunk to judge.
    #[codec(index = 4)]
    NoChunks = 4,
}

/// Why a re-check failed.
///
/// Wire-format enum: variant indices are explicit and never reused.
#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    Encode,
    Decode,
    DecodeWithMemTracking,
    MaxEncodedLen,
    TypeInfo,
)]
pub enum FailReason {
    /// The receipt commits to no proof.
    #[codec(index = 0)]
    NoProof,
    /// The proofs do not match the receipt's commitment.
    #[codec(index = 1)]
    CommitmentMismatch,
    /// Proof parameters or chunk count do not match.
    #[codec(index = 2)]
    ParamsOrChunks,
    /// Chunk `chunk` exceeded `metric`.
    #[codec(index = 3)]
    Threshold {
        /// Index of the first failing chunk.
        chunk: u32,
        /// The bound it exceeded.
        metric: AuditMetric,
    },
}

/// Why a re-check could not decide.
///
/// Wire-format enum: discriminants are explicit and never reused.
#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    Encode,
    Decode,
    DecodeWithMemTracking,
    MaxEncodedLen,
    TypeInfo,
)]
#[repr(u8)]
pub enum InconclusiveReason {
    /// The receipt does not match the request or the answer.
    #[codec(index = 0)]
    InputMismatch = 0,
    /// The model's registered precision is not re-checked.
    #[codec(index = 1)]
    Precision = 1,
    /// The answer's tokens could not be re-created.
    #[codec(index = 2)]
    Tokens = 2,
    /// The re-check engine failed.
    #[codec(index = 3)]
    Engine = 3,
}

/// An auditor's verdict on one inference.
///
/// Wire-format enum: variant indices are explicit and never reused.
#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    Encode,
    Decode,
    DecodeWithMemTracking,
    MaxEncodedLen,
    TypeInfo,
)]
pub enum VerdictOutcome {
    /// Every chunk passed.
    #[codec(index = 0)]
    Pass,
    /// The re-check failed.
    #[codec(index = 1)]
    Fail(FailReason),
    /// The re-check could not decide.
    #[codec(index = 2)]
    Inconclusive(InconclusiveReason),
}

/// A reviewer's vote in a dispute.
///
/// Wire-format enum: discriminants are explicit and never reused.
#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    Encode,
    Decode,
    DecodeWithMemTracking,
    MaxEncodedLen,
    TypeInfo,
)]
#[repr(u8)]
pub enum Vote {
    /// The evidence re-checks as a failure.
    #[codec(index = 0)]
    Confirm = 0,
    /// The evidence is missing, does not match or does not re-check as a failure.
    #[codec(index = 1)]
    Reject = 1,
}

/// How a dispute ended.
///
/// Wire-format enum: discriminants are explicit and never reused.
#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    Encode,
    Decode,
    DecodeWithMemTracking,
    MaxEncodedLen,
    TypeInfo,
)]
#[repr(u8)]
pub enum DisputeOutcome {
    /// Confirmed: the provider was slashed and jailed.
    #[codec(index = 0)]
    Confirmed = 0,
    /// Rejected: the accusers were slashed and made to exit.
    #[codec(index = 1)]
    Rejected = 1,
    /// Neither side reached the quorum before the deadline: nobody is punished.
    #[codec(index = 2)]
    Undecided = 2,
}

/// Lifecycle of an auditor.
///
/// Wire-format enum: discriminants are explicit and never reused.
#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    Encode,
    Decode,
    DecodeWithMemTracking,
    MaxEncodedLen,
    TypeInfo,
)]
#[repr(u8)]
pub enum AuditorStatus {
    /// Registered; on the rosters while staked at or above the threshold.
    #[codec(index = 1)]
    Active = 1,
    /// Leaving (by request or after a rejected dispute): the stake is unbonding.
    #[codec(index = 2)]
    Exiting = 2,
}

/// A registered auditor.
#[derive(
    Clone, Debug, PartialEq, Eq, Encode, Decode, DecodeWithMemTracking, MaxEncodedLen, TypeInfo,
)]
pub struct AuditorRecord<BlockNumber> {
    /// Lifecycle.
    pub status: AuditorStatus,
    /// Bonded stake, in smallest ATC units.
    pub stake: u128,
    /// Stake that is unbonding, by unlock block (still held and slashable).
    pub unlocking: UnlockingList<BlockNumber>,
}

/// Parameters fixed at genesis (design D10).
#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    Encode,
    Decode,
    DecodeWithMemTracking,
    MaxEncodedLen,
    TypeInfo,
)]
pub struct AuditParams {
    /// Blocks per round.
    pub round_blocks: u32,
    /// Auditors drawn per provider and round (`m`).
    pub assign: u8,
    /// Reviewers of a dispute (`N`).
    pub reviewers: u8,
    /// Votes that decide a dispute (`Q`).
    pub quorum: u8,
    /// Blocks reviewers have to vote.
    pub vote_blocks: u32,
    /// Share of a confirmed provider's stake slashed.
    pub provider_slash: Perbill,
    /// Share of a rejected accuser's stake slashed.
    pub auditor_slash: Perbill,
    /// Blocks an auditor's stake unbonds for.
    pub unbond_blocks: u32,
}

/// Why [`AuditParams`] break the guardrails.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParamsError {
    /// A block count is zero.
    ZeroBlocks,
    /// `assign` is not in `1..=MAX_ASSIGN`.
    Assign,
    /// `1 ≤ Q ≤ N ≤ MAX_REVIEWERS` and `2Q > N` do not hold.
    Quorum,
    /// A slash ratio is below 1%.
    Slash,
}

impl core::fmt::Display for ParamsError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match self {
            Self::ZeroBlocks => "round, vote and unbonding blocks must be positive",
            Self::Assign => "auditors per provider must be between 1 and 8",
            Self::Quorum => "the quorum must be a majority of at most 15 reviewers",
            Self::Slash => "slash ratios must be at least 1%",
        })
    }
}

#[cfg(feature = "std")]
impl std::error::Error for ParamsError {}

impl AuditParams {
    /// Draft values of live chains.
    pub const LIVE: Self = Self {
        round_blocks: 1_800,
        assign: 2,
        reviewers: 5,
        quorum: 3,
        vote_blocks: 600,
        provider_slash: Perbill::from_percent(10),
        auditor_slash: Perbill::from_percent(10),
        unbond_blocks: 604_800,
    };

    /// Checks the guardrails.
    ///
    /// # Errors
    ///
    /// The first guardrail broken.
    pub fn check(&self) -> Result<(), ParamsError> {
        if self.round_blocks == 0 || self.vote_blocks == 0 || self.unbond_blocks == 0 {
            return Err(ParamsError::ZeroBlocks);
        }
        if self.assign == 0 || self.assign > MAX_ASSIGN {
            return Err(ParamsError::Assign);
        }
        let (n, q) = (u16::from(self.reviewers), u16::from(self.quorum));
        if q == 0 || q > n || n > u16::from(MAX_REVIEWERS) || q.saturating_mul(2) <= n {
            return Err(ParamsError::Quorum);
        }
        if self.provider_slash < MIN_SLASH || self.auditor_slash < MIN_SLASH {
            return Err(ParamsError::Slash);
        }
        Ok(())
    }
}

/// Parameters the administration may change within guardrails (design D10).
#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    Encode,
    Decode,
    DecodeWithMemTracking,
    MaxEncodedLen,
    TypeInfo,
)]
pub struct AdjustableParams {
    /// Auditor stake threshold, converted at the reference rate rounding up.
    pub stake_usd: MicroUsd,
    /// Payment per accepted verdict or winning vote, converted rounding down.
    pub payment_usd: MicroUsd,
    /// The re-check thresholds version verdicts must use.
    pub thresholds_version: u16,
}

impl AdjustableParams {
    /// `true` if the stake and payment are within their guardrails.
    #[must_use]
    pub fn within_bounds(&self) -> bool {
        self.stake_usd >= STAKE_USD_BOUNDS.0
            && self.stake_usd <= STAKE_USD_BOUNDS.1
            && self.payment_usd <= MAX_PAYMENT_USD
    }
}

/// A verdict as the chain keeps it.
#[derive(
    Clone, Debug, PartialEq, Eq, Encode, Decode, DecodeWithMemTracking, MaxEncodedLen, TypeInfo,
)]
pub struct VerdictRecord<AccountId> {
    /// Who submitted it.
    pub auditor: AccountId,
    /// The outcome.
    pub outcome: VerdictOutcome,
    /// Thresholds version used.
    pub thresholds_version: u16,
    /// Commitment to the evidence (failures only).
    pub evidence: Option<[u8; 32]>,
    /// BLAKE3 of the SCALE-encoded signed receipt.
    pub receipt_hash: H256,
    /// The receipt's request ID.
    pub request_id: [u8; 32],
    /// The receipt's gateway.
    pub gateway: AccountId,
}

/// One accuser of a dispute: an auditor and the round of its failing verdict.
#[derive(
    Clone, Debug, PartialEq, Eq, Encode, Decode, DecodeWithMemTracking, MaxEncodedLen, TypeInfo,
)]
pub struct Accuser<AccountId> {
    /// The auditor.
    pub auditor: AccountId,
    /// Round of its failing verdict.
    pub round: RoundIndex,
}

/// A dispute, open or closed.
#[derive(
    Clone, Debug, PartialEq, Eq, Encode, Decode, DecodeWithMemTracking, MaxEncodedLen, TypeInfo,
)]
pub struct DisputeRecord<AccountId, BlockNumber> {
    /// The accused provider.
    pub provider: AccountId,
    /// Round the dispute was opened in.
    pub round: RoundIndex,
    /// The auditors whose failing verdicts opened it.
    pub accusers: BoundedVec<Accuser<AccountId>, ConstU32<MAX_ACCUSERS>>,
    /// Drawn reviewers and their votes, in draw order.
    pub reviewers: BoundedVec<(AccountId, Option<Vote>), ConstU32<{ MAX_REVIEWERS as u32 }>>,
    /// Last block votes are accepted in.
    pub deadline: BlockNumber,
    /// How it ended; `None` while open.
    pub outcome: Option<DisputeOutcome>,
    /// Block it was closed in; `None` while open.
    pub closed_at: Option<BlockNumber>,
}

/// Verdict counts of a provider.
#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    PartialEq,
    Eq,
    Encode,
    Decode,
    DecodeWithMemTracking,
    MaxEncodedLen,
    TypeInfo,
)]
pub struct ProviderAuditStats {
    /// Passing verdicts.
    pub pass: u32,
    /// Failing verdicts.
    pub fail: u32,
    /// Inconclusive verdicts (issue I-013).
    pub inconclusive: u32,
    /// Disputes confirmed against it.
    pub confirmed: u32,
}

/// Activity counts of an auditor.
#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    PartialEq,
    Eq,
    Encode,
    Decode,
    DecodeWithMemTracking,
    MaxEncodedLen,
    TypeInfo,
)]
pub struct AuditorStats {
    /// Accepted verdicts.
    pub verdicts: u32,
    /// Votes cast.
    pub votes: u32,
    /// Disputes it was drawn for and closed without its vote.
    pub missed_votes: u32,
}

/// What [`sample`] draws for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Draw {
    /// The auditors assigned to a provider in a round.
    Assign,
    /// The reviewers of dispute `0`.
    Review(u64),
}

/// The accounts a draw picks from.
#[derive(Clone, Copy, Debug)]
pub struct Pool<'a, A> {
    /// The round's roster, in account order.
    pub roster: &'a [A],
    /// Members that must not be drawn.
    pub excluded: &'a [A],
}

/// Draws up to `count` distinct members of the pool's roster, skipping the excluded ones
/// (design D4).
///
/// Draw `i` hashes `seed ‖ provider ‖ [u64_le(dispute)] ‖ u32_le(i)` under the draw's context
/// and takes the first 8 bytes, little endian, modulo the roster length; a member already drawn
/// or excluded is replaced by the next eligible one after it, wrapping around. The result is in
/// draw order; it is shorter than `count` only when fewer members are eligible.
#[must_use]
pub fn sample<A: Encode + PartialEq + Clone>(
    pool: Pool<'_, A>,
    seed: &H256,
    provider: &A,
    draw: Draw,
    count: usize,
) -> Vec<A> {
    let Pool { roster, excluded } = pool;
    let eligible = roster.iter().filter(|a| !excluded.contains(a)).count();
    let want = count.min(eligible);
    let mut out: Vec<A> = Vec::with_capacity(want);
    let n = roster.len() as u64;
    let mut i: u32 = 0;
    while out.len() < want {
        let mut data = Vec::with_capacity(32 + 40 + 12);
        data.extend_from_slice(seed.as_bytes());
        provider.encode_to(&mut data);
        let context = match draw {
            Draw::Assign => ASSIGN_CONTEXT,
            Draw::Review(dispute) => {
                data.extend_from_slice(&dispute.to_le_bytes());
                REVIEW_CONTEXT
            }
        };
        data.extend_from_slice(&i.to_le_bytes());
        // Both contexts are well formed, so `derive` cannot fail; an error ends the draw early
        // rather than panicking.
        let Ok(h) = ac_crypto::hash::derive(context, &data) else {
            break;
        };
        let mut word = [0u8; 8];
        if let Some(head) = h.get(..8) {
            word.copy_from_slice(head);
        }
        let start = u64::from_le_bytes(word).checked_rem(n).unwrap_or(0);
        // Probe from `start`: `want <= eligible` guarantees an eligible member is found.
        let mut k = 0u64;
        while k < n {
            let idx = start.saturating_add(k).checked_rem(n).unwrap_or(0);
            if let Some(a) = usize::try_from(idx).ok().and_then(|j| roster.get(j))
                && !excluded.contains(a)
                && !out.contains(a)
            {
                out.push(a.clone());
                break;
            }
            k = k.saturating_add(1);
        }
        i = i.saturating_add(1);
    }
    out
}

/// Round of `block` for rounds of `round_blocks` blocks: `(block − 1) / round_blocks`; block 0
/// (genesis) is in round 0.
#[must_use]
pub fn round_of(block: u32, round_blocks: u32) -> RoundIndex {
    block
        .saturating_sub(1)
        .checked_div(round_blocks)
        .unwrap_or(0)
}

/// First block of round `round`.
#[must_use]
pub fn round_start(round: RoundIndex, round_blocks: u32) -> u32 {
    round.saturating_mul(round_blocks).saturating_add(1)
}

#[cfg(test)]
mod tests {
    // Test code: AGENT.md §5.3 permits unwrap, indexing and plain arithmetic.
    #![allow(
        clippy::unwrap_used,
        clippy::indexing_slicing,
        clippy::arithmetic_side_effects
    )]

    use super::*;

    fn roster(n: u8) -> Vec<[u8; 32]> {
        (0..n).map(|i| [i; 32]).collect()
    }

    #[test]
    fn live_params_pass_the_guardrails() {
        assert_eq!(AuditParams::LIVE.check(), Ok(()));
    }

    #[test]
    fn genesis_rejects_quorum_not_majority() {
        // Spec market/audit "参数与护栏", scenario "创世参数不满足护栏": N = 5, Q = 2.
        let p = AuditParams {
            quorum: 2,
            ..AuditParams::LIVE
        };
        assert_eq!(p.check(), Err(ParamsError::Quorum));
        let p = AuditParams {
            quorum: 6,
            ..AuditParams::LIVE
        };
        assert_eq!(p.check(), Err(ParamsError::Quorum));
        let p = AuditParams {
            reviewers: 16,
            quorum: 9,
            ..AuditParams::LIVE
        };
        assert_eq!(p.check(), Err(ParamsError::Quorum));
    }

    #[test]
    fn genesis_rejects_other_broken_guardrails() {
        let base = AuditParams::LIVE;
        assert_eq!(
            AuditParams {
                round_blocks: 0,
                ..base
            }
            .check(),
            Err(ParamsError::ZeroBlocks)
        );
        assert_eq!(
            AuditParams { assign: 0, ..base }.check(),
            Err(ParamsError::Assign)
        );
        assert_eq!(
            AuditParams { assign: 9, ..base }.check(),
            Err(ParamsError::Assign)
        );
        assert_eq!(
            AuditParams {
                auditor_slash: Perbill::from_parts(9_999_999),
                ..base
            }
            .check(),
            Err(ParamsError::Slash)
        );
    }

    #[test]
    fn adjustable_bounds() {
        let ok = AdjustableParams {
            stake_usd: MicroUsd(1_000 * MICRO_USD_PER_USD),
            payment_usd: MicroUsd(50_000),
            thresholds_version: 2,
        };
        assert!(ok.within_bounds());
        assert!(
            !AdjustableParams {
                payment_usd: MicroUsd(2 * MICRO_USD_PER_USD),
                ..ok
            }
            .within_bounds()
        );
        assert!(
            !AdjustableParams {
                stake_usd: MicroUsd(99 * MICRO_USD_PER_USD),
                ..ok
            }
            .within_bounds()
        );
    }

    /// Reference implementation of the draw, written independently from the spec text.
    fn reference(
        r: &[[u8; 32]],
        seed: &H256,
        p: &[u8; 32],
        count: usize,
        ex: &[[u8; 32]],
    ) -> Vec<[u8; 32]> {
        let mut out = Vec::new();
        let eligible: Vec<_> = r.iter().filter(|a| !ex.contains(a)).collect();
        let mut i = 0u32;
        while out.len() < count.min(eligible.len()) {
            let mut d = seed.as_bytes().to_vec();
            d.extend_from_slice(p);
            d.extend_from_slice(&i.to_le_bytes());
            let h = ac_crypto::hash::derive(ASSIGN_CONTEXT, &d).unwrap();
            let mut j = (u64::from_le_bytes(h[..8].try_into().unwrap()) % r.len() as u64) as usize;
            while ex.contains(&r[j]) || out.contains(&r[j]) {
                j = (j + 1) % r.len();
            }
            out.push(r[j]);
            i += 1;
        }
        out
    }

    #[test]
    fn draw_matches_the_reference() {
        let r = roster(20);
        for s in 0..50u8 {
            let seed = H256([s; 32]);
            let p = [200 + (s % 7); 32];
            let ex = [r[3], r[s as usize % 20]];
            assert_eq!(
                sample(
                    Pool {
                        roster: &r,
                        excluded: &ex
                    },
                    &seed,
                    &p,
                    Draw::Assign,
                    5
                ),
                reference(&r, &seed, &p, 5, &ex)
            );
        }
    }

    #[test]
    fn short_roster_gives_everyone_eligible() {
        let r = roster(3);
        let got = sample(
            Pool {
                roster: &r,
                excluded: &[r[1]],
            },
            &H256([1; 32]),
            &[9; 32],
            Draw::Assign,
            5,
        );
        assert_eq!(got.len(), 2);
        assert!(!got.contains(&r[1]));
        assert!(
            sample::<[u8; 32]>(
                Pool {
                    roster: &[],
                    excluded: &[]
                },
                &H256([1; 32]),
                &[9; 32],
                Draw::Assign,
                2
            )
            .is_empty()
        );
    }

    #[test]
    fn review_draw_differs_from_assignment() {
        let r = roster(30);
        let seed = H256([7; 32]);
        let a = sample(
            Pool {
                roster: &r,
                excluded: &[],
            },
            &seed,
            &[1; 32],
            Draw::Assign,
            5,
        );
        let b = sample(
            Pool {
                roster: &r,
                excluded: &[],
            },
            &seed,
            &[1; 32],
            Draw::Review(0),
            5,
        );
        let c = sample(
            Pool {
                roster: &r,
                excluded: &[],
            },
            &seed,
            &[1; 32],
            Draw::Review(1),
            5,
        );
        assert_ne!(a, b);
        assert_ne!(b, c);
    }

    proptest::proptest! {
        #[test]
        fn draw_is_distinct_sized_and_deterministic(
            n in 0u8..40, count in 0usize..10, s in proptest::array::uniform32(0u8..),
            ex in proptest::collection::vec(0u8..40, 0..5),
        ) {
            let r = roster(n);
            let ex: Vec<[u8; 32]> = ex.into_iter().map(|i| [i; 32]).collect();
            let seed = H256(s);
            let got = sample(Pool { roster: &r, excluded: &ex }, &seed, &[255; 32], Draw::Assign, count);
            let eligible = r.iter().filter(|a| !ex.contains(a)).count();
            proptest::prop_assert_eq!(got.len(), count.min(eligible));
            for (i, a) in got.iter().enumerate() {
                proptest::prop_assert!(!ex.contains(a));
                proptest::prop_assert!(!got[..i].contains(a));
            }
            proptest::prop_assert_eq!(got, sample(Pool { roster: &r, excluded: &ex }, &seed, &[255; 32], Draw::Assign, count));
        }
    }

    #[test]
    fn rounds() {
        assert_eq!(round_of(0, 10), 0);
        assert_eq!(round_of(1, 10), 0);
        assert_eq!(round_of(10, 10), 0);
        assert_eq!(round_of(11, 10), 1);
        assert_eq!(round_start(1, 10), 11);
        assert_eq!(round_of(round_start(7, 1_800), 1_800), 7);
    }

    #[test]
    fn verdict_outcome_encoding_is_stable() {
        let fail = VerdictOutcome::Fail(FailReason::Threshold {
            chunk: 2,
            metric: AuditMetric::MantissaMean,
        });
        assert_eq!(fail.encode(), [1, 3, 2, 0, 0, 0, 2]);
        assert_eq!(VerdictOutcome::Pass.encode(), [0]);
        assert_eq!(
            VerdictOutcome::Inconclusive(InconclusiveReason::Tokens).encode(),
            [2, 2]
        );
        assert_eq!(Vote::Reject.encode(), [1]);
    }
}
