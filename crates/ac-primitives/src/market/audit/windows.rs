//! The coupling of the audit time windows (OpenSpec change m6-audit-hardening design D9; spec
//! `market/audit` "时间窗口的耦合约束").
//!
//! A verdict submitted in round `r` can be cited until a dispute or spot check that opens at the
//! end of round `r + 1` closes, `V` blocks later: within `2R + V` blocks of its submission
//! ([`citation_window`]). Stake that unbonds sooner could leave before it is slashed, and records
//! pruned sooner could not be reviewed, so both must cover that window, and reviewers must vote
//! within a round. The chain checks these at genesis; wallets and tests use the same check.

/// The windows [`check_windows`] relates, in blocks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WindowParams {
    /// Blocks per audit round (`R`).
    pub round_blocks: u64,
    /// Blocks reviewers have to vote (`V`).
    pub vote_blocks: u64,
    /// Blocks an auditor's stake unbonds for.
    pub auditor_unbond: u64,
    /// Blocks a provider's stake unbonds for.
    pub provider_unbond: u64,
    /// Blocks verdict and dispute records are kept at least.
    pub retention: u64,
}

/// The window [`check_windows`] found broken.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WindowError {
    /// The vote period is longer than a round.
    VoteLongerThanRound,
    /// An auditor's stake unbonds before the verdicts it submitted can no longer be cited.
    AuditorUnbond,
    /// A provider's stake unbonds before a dispute on its last verdicts closes.
    ProviderUnbond,
    /// Records are pruned before the verdicts can no longer be cited.
    Retention,
}

impl core::fmt::Display for WindowError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match self {
            Self::VoteLongerThanRound => "the vote period must not exceed a round",
            Self::AuditorUnbond => {
                "auditor unbonding must be at least two rounds plus the vote period"
            }
            Self::ProviderUnbond => {
                "provider unbonding must be at least two rounds plus the vote period"
            }
            Self::Retention => "record retention must be at least two rounds plus the vote period",
        })
    }
}

/// Blocks a verdict can be cited by a dispute or a spot check after its submission: `2R + V`
/// (saturating).
#[must_use]
pub fn citation_window(round_blocks: u64, vote_blocks: u64) -> u64 {
    round_blocks.saturating_mul(2).saturating_add(vote_blocks)
}

/// Checks the windows: `V ≤ R`, and both unbonding periods and the record retention at least
/// [`citation_window`].
///
/// # Errors
///
/// The first window broken, in the order of [`WindowError`].
pub fn check_windows(p: &WindowParams) -> Result<(), WindowError> {
    if p.vote_blocks > p.round_blocks {
        return Err(WindowError::VoteLongerThanRound);
    }
    let window = citation_window(p.round_blocks, p.vote_blocks);
    if p.auditor_unbond < window {
        return Err(WindowError::AuditorUnbond);
    }
    if p.provider_unbond < window {
        return Err(WindowError::ProviderUnbond);
    }
    if p.retention < window {
        return Err(WindowError::Retention);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Live chain draft values: rounds of 1,200 blocks, 600 to vote, 7 days of unbonding and
    /// retention.
    const LIVE: WindowParams = WindowParams {
        round_blocks: 1_200,
        vote_blocks: 600,
        auditor_unbond: 604_800,
        provider_unbond: 604_800,
        retention: 604_800,
    };

    #[test]
    fn live_values_hold() {
        assert_eq!(check_windows(&LIVE), Ok(()));
        assert_eq!(citation_window(1_200, 600), 3_000);
    }

    // Spec "时间窗口的耦合约束", scenario "解绑期过短".
    #[test]
    fn short_auditor_unbonding_fails() {
        let p = WindowParams {
            auditor_unbond: 1_000,
            ..LIVE
        };
        assert_eq!(check_windows(&p), Err(WindowError::AuditorUnbond));
    }

    /// Sets one window of the parameters.
    type Setter = fn(&mut WindowParams, u64);

    // Each window: equal to `2R + V` passes, one block less fails.
    #[test]
    fn boundaries() {
        let at = |f: Setter, v: u64| {
            let mut p = LIVE;
            f(&mut p, v);
            check_windows(&p)
        };
        let cases: [(Setter, WindowError); 3] = [
            (|p, v| p.auditor_unbond = v, WindowError::AuditorUnbond),
            (|p, v| p.provider_unbond = v, WindowError::ProviderUnbond),
            (|p, v| p.retention = v, WindowError::Retention),
        ];
        for (set, err) in cases {
            assert_eq!(at(set, 3_000), Ok(()));
            assert_eq!(at(set, 2_999), Err(err));
        }
        let vote = |v| {
            check_windows(&WindowParams {
                vote_blocks: v,
                ..LIVE
            })
        };
        assert_eq!(vote(1_200), Ok(()));
        assert_eq!(vote(1_201), Err(WindowError::VoteLongerThanRound));
    }

    #[test]
    fn local_test_values_hold() {
        // The dev and local presets after this change: rounds and votes of 20 blocks, 60 blocks
        // of unbonding.
        let p = WindowParams {
            round_blocks: 20,
            vote_blocks: 20,
            auditor_unbond: 60,
            provider_unbond: 60,
            retention: 60,
        };
        assert_eq!(check_windows(&p), Ok(()));
    }
}
