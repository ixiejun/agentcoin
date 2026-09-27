//! Scenario "8 年模拟" (economics/emission): two four-year periods at the live epoch length,
//! with no work, full load and deterministic random work; every epoch matches the reference
//! formula exactly and minting never exceeds the schedule or the cap.

#![allow(
    clippy::arithmetic_side_effects,
    clippy::unwrap_used,
    clippy::panic,
    missing_docs
)]

use ac_primitives::emission::{
    BLOCKS_PER_PERIOD, CAP, EmissionSchedule, FIRST_PERIOD_TOTAL, LIVE_EPOCH_LENGTH, Phase,
};
use ac_sim::{RandomWork, Totals, reference, run};

fn live() -> EmissionSchedule {
    EmissionSchedule::new(LIVE_EPOCH_LENGTH).unwrap()
}

fn two_periods() -> u64 {
    2 * live().epochs_per_period()
}

/// Runs and checks every epoch against the reference; returns the totals.
fn checked(phase: Phase, work: impl FnMut(u64) -> (u128, u128)) -> Totals {
    let schedule = live();
    let mut cumulative_scheduled = 0u128;
    let mut cumulative_minted = 0u128;
    run(
        &schedule,
        two_periods(),
        phase,
        work,
        |epoch, input, out| {
            let (total, reserve) = reference(
                input.scheduled,
                input.reserve,
                (input.market_work, input.public_work),
                phase == Phase::Pos,
            );
            assert_eq!((out.total, out.reserve), (total, reserve), "epoch {epoch}");
            cumulative_scheduled += input.scheduled;
            cumulative_minted += out.total;
            assert!(cumulative_minted <= cumulative_scheduled, "epoch {epoch}");
            assert_eq!(
                schedule.cumulative_scheduled(epoch + 1),
                cumulative_scheduled,
                "epoch {epoch}"
            );
        },
    )
}

/// Rounding loss of the two periods' schedule: each epoch rounds down once.
fn expected_schedule() -> u128 {
    let per_period = u128::from(live().epochs_per_period());
    let first = FIRST_PERIOD_TOTAL * u128::from(LIVE_EPOCH_LENGTH) / u128::from(BLOCKS_PER_PERIOD);
    let second =
        (FIRST_PERIOD_TOTAL / 2) * u128::from(LIVE_EPOCH_LENGTH) / u128::from(BLOCKS_PER_PERIOD);
    (first + second) * per_period
}

#[test]
fn no_work() {
    let totals = checked(Phase::Poa, |_| (0, 0));
    assert_eq!(totals.scheduled, expected_schedule());
    // Only the 5% floor is minted; everything else rolls over.
    assert_eq!(totals.minted + totals.reserve, totals.scheduled);
    assert!(totals.minted * 100 <= totals.scheduled * 5);
    // Rounding loses less than one smallest unit per epoch.
    let loss = FIRST_PERIOD_TOTAL * 3 / 2 - totals.scheduled;
    assert!(loss < u128::from(two_periods()), "loss {loss}");
    assert!(totals.scheduled <= CAP);
}

#[test]
fn full_load() {
    let totals = checked(Phase::Pos, |_| (u128::MAX / 4, u128::MAX / 4));
    assert!(totals.minted <= totals.scheduled);
    assert_eq!(totals.minted + totals.reserve, totals.scheduled);
}

#[test]
fn random_work() {
    let s0 = live().scheduled(0);
    let mut rng = RandomWork::new(2026);
    let totals = checked(Phase::Pos, |_| rng.sample(2 * s0));
    assert!(totals.minted <= totals.scheduled);
    assert_eq!(totals.minted + totals.reserve, totals.scheduled);
}
