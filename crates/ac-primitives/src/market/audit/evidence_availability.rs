//! Evidence availability and the light penalty (OpenSpec change m6-audit-hardening design D6,
//! D7; spec `market/audit` "争议与升级复核", "处罚").
//!
//! A reviewer's ballot marks, per verdict a dispute or spot check cites, whether it obtained the
//! evidence (bit `i` set: the evidence of verdict `i` was not obtained). A verdict marked so by
//! at least the quorum of ballots is *evidence unavailable*: its auditor is not punished as if
//! the evidence had been refuted, but takes a light penalty, which turns into the full one once
//! the auditor has been found so too often within the record retention.

use alloc::vec::Vec;

use super::RoundIndex;

/// Most verdicts a ballot can mark: the bits of its `u128` mask.
pub const MAX_MARKED: usize = 128;

/// Highest limit of evidence-unavailable findings before the full penalty (guardrail 1–10);
/// also the most findings an auditor record keeps.
pub const MAX_UNAVAILABLE_LIMIT: u8 = 10;

/// Which of the `cited` verdicts are evidence unavailable: marked in at least `quorum` of the
/// `masks`. Bits beyond `cited` (or beyond [`MAX_MARKED`]) are ignored; a zero quorum finds
/// nothing.
#[must_use]
pub fn unavailable(masks: &[u128], cited: usize, quorum: u8) -> Vec<bool> {
    let quorum = usize::from(quorum);
    (0..cited)
        .map(|i| {
            let marked = match u32::try_from(i).ok().and_then(|b| 1u128.checked_shl(b)) {
                Some(bit) => masks.iter().filter(|m| *m & bit != 0).count(),
                None => 0,
            };
            quorum > 0 && marked >= quorum
        })
        .collect()
}

/// The penalty of an auditor found evidence unavailable.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnavailablePenalty {
    /// The light slash, without exit.
    Light,
    /// The full slash and exit: the limit is reached.
    Full,
}

/// The penalty of a finding in round `now` for an auditor with earlier findings in rounds
/// `history`: full when, counting this one, the findings within the last `retention_rounds`
/// rounds reach `limit`, light otherwise.
#[must_use]
pub fn unavailable_penalty(
    history: &[RoundIndex],
    now: RoundIndex,
    retention_rounds: u32,
    limit: u8,
) -> UnavailablePenalty {
    let recent = recent_findings(history, now, retention_rounds).count();
    if recent.saturating_add(1) >= usize::from(limit) {
        UnavailablePenalty::Full
    } else {
        UnavailablePenalty::Light
    }
}

/// The findings of `history` still within the last `retention_rounds` rounds of `now` (round
/// `r` is kept while `r + retention_rounds > now`): what an auditor record keeps, oldest first.
pub fn recent_findings(
    history: &[RoundIndex],
    now: RoundIndex,
    retention_rounds: u32,
) -> impl Iterator<Item = RoundIndex> + '_ {
    history
        .iter()
        .copied()
        .filter(move |r| r.saturating_add(retention_rounds) > now)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Spec "争议与升级复核", scenario "投票标明证据", and "处罚", scenario "证据不可用只受轻罚":
    // three ballots, all marking verdict 0 (accuser A), none marking verdict 1 (accuser B).
    #[test]
    fn quorum_of_marks() {
        let masks = [0b01, 0b01, 0b01, 0b10];
        assert_eq!(unavailable(&masks, 2, 3), [true, false]);
        assert_eq!(unavailable(&masks, 2, 4), [false, false]);
        // A mark the quorum does not reach.
        assert_eq!(unavailable(&[0b10, 0b10], 2, 3), [false, false]);
    }

    #[test]
    fn bits_beyond_the_cited_verdicts_are_ignored() {
        assert_eq!(unavailable(&[u128::MAX; 3], 1, 3), [true]);
        assert!(unavailable(&[1 << 127; 3], 128, 3)[127]);
        assert_eq!(unavailable(&[1; 3], 1, 0), [false]);
        assert_eq!(unavailable(&[], 0, 3), Vec::<bool>::new());
    }

    // Scenario "多次证据不可用升级为完整处罚": 2 earlier findings, a third one is full.
    #[test]
    fn third_finding_is_full() {
        assert_eq!(
            unavailable_penalty(&[], 100, 504, 3),
            UnavailablePenalty::Light
        );
        assert_eq!(
            unavailable_penalty(&[90], 100, 504, 3),
            UnavailablePenalty::Light
        );
        assert_eq!(
            unavailable_penalty(&[80, 90], 100, 504, 3),
            UnavailablePenalty::Full
        );
        // A limit of 1 makes every finding full.
        assert_eq!(
            unavailable_penalty(&[], 0, 504, 1),
            UnavailablePenalty::Full
        );
    }

    #[test]
    fn old_findings_expire() {
        // Retention of 504 rounds: round 0 is gone at round 504, round 1 is kept.
        assert_eq!(recent_findings(&[0, 1], 504, 504).collect::<Vec<_>>(), [1]);
        assert_eq!(
            unavailable_penalty(&[0, 1], 504, 504, 3),
            UnavailablePenalty::Light
        );
    }
}
