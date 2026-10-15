//! Spot checks of verdicts and the light penalty's parameters (OpenSpec change
//! m6-audit-hardening design D5–D7; spec `market/audit` "裁决抽查", "处罚").
//!
//! A spot check re-checks one verdict judged by the thresholds. Its reviewers vote to uphold or
//! overturn it and say whether they obtained its evidence; they compare the outcome only, never
//! the statistics, which differ between hardware (design D5). An overturned verdict's auditor is
//! slashed like a rejected accuser, or only lightly when the evidence was unavailable; a spot
//! check never touches the provider.

use parity_scale_codec::{Decode, DecodeWithMemTracking, Encode, MaxEncodedLen};
use scale_info::TypeInfo;
use sp_core::ConstU32;
use sp_runtime::{BoundedVec, Perbill};

use super::evidence_availability::MAX_UNAVAILABLE_LIMIT;
use super::{MAX_REVIEWERS, RoundIndex};

/// A spot-check reviewer's vote.
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
pub enum ReviewVote {
    /// The re-check gives the verdict's outcome.
    #[codec(index = 0)]
    Uphold,
    /// It does not, or the evidence was not obtained or does not match.
    #[codec(index = 1)]
    Overturn,
}

/// How a spot check ended.
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
pub enum ReviewOutcome {
    /// `Q` reviewers upheld the verdict.
    #[codec(index = 0)]
    Upheld,
    /// `Q` reviewers overturned it.
    #[codec(index = 1)]
    Overturned,
    /// The deadline passed without `Q` votes on either side.
    #[codec(index = 2)]
    Undecided,
}

/// A reviewer's ballot: the vote and whether the reviewer obtained the verdict's evidence.
pub type ReviewBallot = (ReviewVote, bool);

/// A spot check, open or closed.
#[derive(
    Clone, Debug, PartialEq, Eq, Encode, Decode, DecodeWithMemTracking, MaxEncodedLen, TypeInfo,
)]
pub struct ReviewRecord<AccountId, BlockNumber> {
    /// The provider of the checked verdict.
    pub provider: AccountId,
    /// The round of the checked verdict.
    pub round: RoundIndex,
    /// The auditor of the checked verdict.
    pub auditor: AccountId,
    /// Drawn reviewers and their ballots, in draw order.
    pub reviewers:
        BoundedVec<(AccountId, Option<ReviewBallot>), ConstU32<{ MAX_REVIEWERS as u32 }>>,
    /// Last block ballots are accepted in.
    pub deadline: BlockNumber,
    /// How it ended; `None` while open.
    pub outcome: Option<ReviewOutcome>,
    /// Block it was closed in; `None` while open.
    pub closed_at: Option<BlockNumber>,
}

impl<AccountId, BlockNumber> ReviewRecord<AccountId, BlockNumber> {
    /// The outcome `quorum` ballots on one side decide, if any.
    #[must_use]
    pub fn decided(&self, quorum: u8) -> Option<ReviewOutcome> {
        let count = |side: ReviewVote| {
            self.reviewers
                .iter()
                .filter(|(_, b)| b.is_some_and(|(v, _)| v == side))
                .count()
        };
        let quorum = usize::from(quorum);
        if quorum == 0 {
            None
        } else if count(ReviewVote::Uphold) >= quorum {
            Some(ReviewOutcome::Upheld)
        } else if count(ReviewVote::Overturn) >= quorum {
            Some(ReviewOutcome::Overturned)
        } else {
            None
        }
    }

    /// Whether at least `quorum` ballots say the evidence was not obtained.
    #[must_use]
    pub fn evidence_unavailable(&self, quorum: u8) -> bool {
        let marked = self
            .reviewers
            .iter()
            .filter(|(_, b)| b.is_some_and(|(_, obtained)| !obtained))
            .count();
        quorum > 0 && marked >= usize::from(quorum)
    }
}

/// Spot-check counts of an auditor (kept apart from [`super::AuditorStats`], whose encoding
/// stays).
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
pub struct ReviewStats {
    /// Its verdicts drawn for a spot check.
    pub spot_checked: u32,
    /// Its verdicts overturned.
    pub overturned: u32,
    /// Times its evidence was found unavailable, in disputes and spot checks.
    pub unavailable: u32,
}

/// The light penalty's parameters, fixed at genesis.
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
pub struct PenaltyParams {
    /// Share of the stake an auditor whose evidence was unavailable loses.
    pub light_slash: Perbill,
    /// Findings of unavailable evidence within the retention at which the penalty is the full
    /// one.
    pub unavailable_limit: u8,
}

impl PenaltyParams {
    /// Live-chain values: 1%, the third finding is full.
    pub const LIVE: Self = Self {
        light_slash: Perbill::from_percent(1),
        unavailable_limit: 3,
    };

    /// Whether the guardrails hold: the light slash at most `auditor_slash`, the limit 1 to
    /// [`MAX_UNAVAILABLE_LIMIT`].
    #[must_use]
    pub fn is_valid(&self, auditor_slash: Perbill) -> bool {
        self.light_slash <= auditor_slash
            && (1..=MAX_UNAVAILABLE_LIMIT).contains(&self.unavailable_limit)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(ballots: &[Option<ReviewBallot>]) -> ReviewRecord<u8, u32> {
        ReviewRecord {
            provider: 0,
            round: 1,
            auditor: 1,
            reviewers: BoundedVec::truncate_from(
                ballots
                    .iter()
                    .enumerate()
                    .map(|(i, b)| (u8::try_from(i).unwrap().saturating_add(10), *b))
                    .collect(),
            ),
            deadline: 100,
            outcome: None,
            closed_at: None,
        }
    }

    // Spec "裁决抽查", scenarios "抽查维持" and "放水的通过被推翻".
    #[test]
    fn the_quorum_decides() {
        let up = Some((ReviewVote::Uphold, true));
        let over = Some((ReviewVote::Overturn, true));
        assert_eq!(
            record(&[up, up, up, over, None]).decided(3),
            Some(ReviewOutcome::Upheld)
        );
        assert_eq!(
            record(&[over, over, over, None, None]).decided(3),
            Some(ReviewOutcome::Overturned)
        );
        assert_eq!(record(&[up, up, over, over, None]).decided(3), None);
        assert_eq!(record(&[up]).decided(0), None);
    }

    #[test]
    fn unavailable_evidence_needs_the_quorum() {
        let lost = Some((ReviewVote::Overturn, false));
        let over = Some((ReviewVote::Overturn, true));
        assert!(record(&[lost, lost, lost, None, None]).evidence_unavailable(3));
        assert!(!record(&[lost, lost, over, None, None]).evidence_unavailable(3));
    }

    #[test]
    fn penalty_guardrails() {
        let ten = Perbill::from_percent(10);
        assert!(PenaltyParams::LIVE.is_valid(ten));
        assert!(
            !PenaltyParams {
                light_slash: Perbill::from_percent(11),
                ..PenaltyParams::LIVE
            }
            .is_valid(ten)
        );
        for limit in [0, 11] {
            assert!(
                !PenaltyParams {
                    unavailable_limit: limit,
                    ..PenaltyParams::LIVE
                }
                .is_valid(ten)
            );
        }
    }

    #[test]
    fn encodings_are_fixed() {
        assert_eq!(ReviewVote::Overturn.encode(), [1]);
        assert_eq!(ReviewOutcome::Undecided.encode(), [2]);
    }
}
