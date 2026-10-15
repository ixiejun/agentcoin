//! The deterrence check of provider stakes (OpenSpec change m6-audit-hardening design D3, D4;
//! spec `market/providers` "质押门槛", `market/work-settlement` "近期结算额").
//!
//! A provider that cheats on a share `p` of its requests gains `g × p × r` per epoch (revenue
//! `r`, a gain ratio `g` of the revenue) until the statistical judgment finds it, `T(p)` epochs
//! later at p95; the exposure horizon `H` bounds `p × T(p)` over the shares it can find. When
//! found, the provider loses the slash `f × S` of its stake and its shares of the `C` epochs
//! still in their challenge period. The loss is at least the gain when
//! `S ≥ r × (g × H − C) / f = κ × r`, so a provider's required stake is the larger of the tier
//! threshold and `κ` times its recent revenue per epoch.

use sp_runtime::Perbill;

use crate::emission::EpochIndex;
use crate::market::usd::MicroUsd;

/// Lowest and highest assumed gain ratio `g` (genesis guardrail).
pub const GAIN_BOUNDS: (Perbill, Perbill) = (Perbill::from_percent(10), Perbill::one());
/// Highest exposure horizon `H` in epochs (genesis guardrail; the lowest is 1).
pub const MAX_HORIZON_EPOCHS: u32 = 720;
/// Highest window `W` of the recent revenue in epochs (genesis guardrail; the lowest is 1).
pub const MAX_SETTLED_WINDOW: u32 = 168;

/// Basis points in one.
const BPS: u128 = 10_000;
/// Parts per billion in one.
const PPB: u128 = 1_000_000_000;

/// `κ = (g × H − C) / f` epochs, in basis points of an epoch, rounded up; zero when `g × H ≤ C`
/// or `f` is zero (no deterrence is computable then: callers keep the tier threshold).
#[must_use]
pub fn kappa_bps(gain: Perbill, horizon: u32, challenge: u32, slash: Perbill) -> u128 {
    let slash = u128::from(slash.deconstruct());
    // In parts per billion of an epoch: `g × H − C`.
    let exposed = u128::from(gain.deconstruct())
        .saturating_mul(u128::from(horizon))
        .saturating_sub(u128::from(challenge).saturating_mul(PPB));
    if slash == 0 || exposed == 0 {
        return 0;
    }
    exposed.saturating_mul(BPS).div_ceil(slash)
}

/// A provider's required stake: the larger of the tier threshold `floor` and `κ` times its
/// recent revenue per epoch `avg`, rounded up to a micro-dollar.
#[must_use]
pub fn required_stake(floor: MicroUsd, kappa_bps: u128, avg: MicroUsd) -> MicroUsd {
    let deterrent = MicroUsd(avg.0.saturating_mul(kappa_bps).div_ceil(BPS));
    if deterrent.0 > floor.0 {
        deterrent
    } else {
        floor
    }
}

/// The recent revenue per epoch at epoch `now` over a window of `window` epochs: the revenue of
/// the epochs `now − window + 1 ..= now` in `settled` (epoch, revenue) divided by `window`,
/// rounded up; epochs without an entry count as zero. A zero window gives zero.
#[must_use]
pub fn avg_per_epoch(settled: &[(EpochIndex, MicroUsd)], now: EpochIndex, window: u32) -> MicroUsd {
    if window == 0 {
        return MicroUsd(0);
    }
    let total = settled
        .iter()
        .filter(|(e, _)| in_window(*e, now, window))
        .fold(0u128, |acc, (_, usd)| acc.saturating_add(usd.0));
    MicroUsd(total.div_ceil(u128::from(window)))
}

/// Whether epoch `epoch` is one of the last `window` epochs at `now` (`epoch + window > now`,
/// and not after `now`).
#[must_use]
pub fn in_window(epoch: EpochIndex, now: EpochIndex, window: u32) -> bool {
    epoch <= now && epoch.saturating_add(u64::from(window)) > now
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::market::usd::MICRO_USD_PER_USD;
    use proptest::prelude::*;

    fn usd(n: u128) -> MicroUsd {
        MicroUsd(n.saturating_mul(MICRO_USD_PER_USD))
    }

    const T1: MicroUsd = MicroUsd(1_000 * MICRO_USD_PER_USD);

    /// Live draft values: g = 50%, H = 11, C = 2, f = 10%.
    fn live_kappa() -> u128 {
        kappa_bps(Perbill::from_percent(50), 11, 2, Perbill::from_percent(10))
    }

    #[test]
    fn live_kappa_is_35_epochs() {
        assert_eq!(live_kappa(), 350_000);
    }

    // Spec `market/providers` "质押门槛", scenario "营收增长提高所需质押".
    #[test]
    fn revenue_raises_the_required_stake() {
        assert_eq!(required_stake(T1, live_kappa(), usd(100)), usd(3_500));
    }

    // Scenario "小提供者只需门槛".
    #[test]
    fn small_providers_need_the_threshold() {
        assert_eq!(required_stake(T1, live_kappa(), usd(10)), T1);
    }

    #[test]
    fn no_deterrence_when_the_challenge_period_covers_the_exposure() {
        // g × H = 1.5 epochs < C = 2.
        assert_eq!(
            kappa_bps(Perbill::from_percent(50), 3, 2, Perbill::from_percent(10)),
            0
        );
        assert_eq!(
            kappa_bps(Perbill::from_percent(50), 11, 2, Perbill::zero()),
            0
        );
    }

    #[test]
    fn rounding_is_up() {
        // (0.5 × 11 − 2) / 0.3 = 11.666… epochs.
        assert_eq!(
            kappa_bps(Perbill::from_percent(50), 11, 2, Perbill::from_percent(30)),
            116_667
        );
        assert_eq!(
            required_stake(MicroUsd(0), 116_667, MicroUsd(1)),
            MicroUsd(12)
        );
    }

    // Spec `market/work-settlement` "近期结算额", scenarios "平均结算额" and "旧纪元移出窗口".
    #[test]
    fn average_over_the_window() {
        assert_eq!(
            avg_per_epoch(&[(3, usd(120)), (9, usd(120))], 20, 24),
            usd(10)
        );
        assert_eq!(avg_per_epoch(&[(5, usd(240))], 28, 24), usd(10));
        assert_eq!(avg_per_epoch(&[(5, usd(240))], 29, 24), MicroUsd(0));
        assert_eq!(avg_per_epoch(&[(5, usd(240))], 5, 0), MicroUsd(0));
        assert_eq!(avg_per_epoch(&[(1, MicroUsd(1))], 1, 24), MicroUsd(1));
    }

    proptest! {
        // The required stake never falls when the revenue rises.
        #[test]
        fn required_stake_is_monotone(a in 0u128..1_000_000_000_000, b in 0u128..1_000_000_000_000,
                                      g in 10u32..=100, h in 1u32..=720, c in 1u32..=24,
                                      f in 1u32..=100) {
            let k = kappa_bps(Perbill::from_percent(g), h, c, Perbill::from_percent(f));
            let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
            prop_assert!(required_stake(T1, k, MicroUsd(lo)).0 <= required_stake(T1, k, MicroUsd(hi)).0);
            prop_assert!(required_stake(T1, k, MicroUsd(lo)).0 >= T1.0);
        }
    }
}
