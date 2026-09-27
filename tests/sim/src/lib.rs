//! Economic simulation of ATC emission (task 1.3 of `m3-economics`; spec economics/emission
//! Scenario "8 年模拟").
//!
//! [`run`] replays epoch settlement with the runtime's own [`settle`] for a number of epochs
//! and a work pattern; the tests compare every epoch against [`reference`], an independent
//! restatement of the plan §5.1 formula written from the spec rather than from the
//! implementation.

/// Runs the README example as a doctest.
#[cfg(doctest)]
#[doc = include_str!("../README.md")]
pub struct ReadmeDoctests;

use ac_primitives::emission::{EmissionSchedule, EpochInput, EpochOutcome, Phase, settle};

/// Work entering an epoch: `(market, public)` in smallest ATC units.
pub type Work = (u128, u128);

/// Totals of a simulation run.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Totals {
    /// Sum of the scheduled amounts.
    pub scheduled: u128,
    /// Sum of the minted amounts.
    pub minted: u128,
    /// Reserve at the end.
    pub reserve: u128,
}

/// Settles `epochs` epochs of `schedule` in `phase`, with the work of each epoch given by
/// `work`; `check` sees every epoch's input and outcome. Returns the totals.
pub fn run(
    schedule: &EmissionSchedule,
    epochs: u64,
    phase: Phase,
    mut work: impl FnMut(u64) -> Work,
    mut check: impl FnMut(u64, &EpochInput, &EpochOutcome),
) -> Totals {
    let mut totals = Totals::default();
    for epoch in 0..epochs {
        let input = EpochInput::new(
            schedule.scheduled(epoch),
            totals.reserve,
            work(epoch),
            phase,
        );
        let out = settle(&input);
        check(epoch, &input, &out);
        totals.scheduled = totals.scheduled.saturating_add(input.scheduled);
        totals.minted = totals.minted.saturating_add(out.total);
        totals.reserve = out.reserve;
    }
    totals
}

/// Deterministic pseudo-random work (xorshift64*), so runs are reproducible without a seed
/// crate.
#[derive(Clone, Debug)]
pub struct RandomWork(u64);

impl RandomWork {
    /// A generator seeded with `seed` (0 is replaced by a fixed non-zero value).
    #[must_use]
    pub fn new(seed: u64) -> Self {
        Self(if seed == 0 {
            0x9e37_79b9_7f4a_7c15
        } else {
            seed
        })
    }

    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    /// Work between 0 and `max` for each of market and public.
    pub fn sample(&mut self, max: u128) -> Work {
        let mut one = || {
            let r = u128::from(self.next() % 10_001);
            max.saturating_mul(r) / 10_000
        };
        (one(), one())
    }
}

/// Independent restatement of the settlement formula (plan §5.1 and design D3 of
/// `m3-economics`), written as plainly as possible: every share in basis points of 10,000,
/// rounded down.
#[must_use]
pub fn reference(s: u128, reserve: u128, work: Work, pos: bool) -> (u128, u128) {
    let avail = s + if reserve < s { reserve } else { s };
    let security = if pos { s * 1_000 / 10_000 } else { 0 };
    let market = core::cmp::min(avail * 5_000 / 10_000, work.0);
    let public = core::cmp::min(avail * 2_000 / 10_000, work.1);
    let proportional = (market + public) * 20 / 70;
    let floor = s * 500 / 10_000;
    let treasury = if proportional > floor {
        proportional
    } else {
        floor
    };
    let total = security + market + public + treasury;
    // With the D16 shares the total never exceeds `avail`, so no scaling is needed here.
    assert!(total <= avail, "reference total above avail");
    let reserve_after = if total > s {
        reserve - (total - s)
    } else {
        reserve + (s - total)
    };
    (total, reserve_after)
}
