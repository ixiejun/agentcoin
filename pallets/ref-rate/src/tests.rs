//! Unit tests; each names the spec `economics/ref-rate` scenario it covers.

use frame_support::{assert_noop, assert_ok};
use sp_runtime::DispatchError;

use ac_primitives::market::traits::PriceSource;
use ac_primitives::market::usd::Rounding;
use ac_primitives::market::{AtcPerUsd, MicroUsd, PriceError};

use crate::mock::{INTERVAL, RefRate, RuntimeOrigin, System, Test, ext};
use crate::{Error, Event, Rate, within_band};

fn set(rate: u128) -> sp_runtime::DispatchResult {
    RefRate::set_rate(RuntimeOrigin::root(), AtcPerUsd(rate))
}

// Requirement "参考汇率的表示", Scenario "查询汇率".
#[test]
fn a_set_rate_is_readable_with_its_block() {
    ext(None).execute_with(|| {
        System::set_block_number(7);
        assert_ok!(set(500_000_000_000_000_000));
        assert_eq!(
            RefRate::rate(),
            Some((AtcPerUsd(500_000_000_000_000_000), 7))
        );
        System::assert_last_event(
            Event::RateSet {
                rate: AtcPerUsd(500_000_000_000_000_000),
                at: 7,
            }
            .into(),
        );
    });
}

// Scenario "未设置时查询".
#[test]
fn an_unset_rate_reads_none() {
    ext(None).execute_with(|| {
        assert_eq!(RefRate::rate(), None);
        assert_eq!(<RefRate as PriceSource>::atc_per_usd(), None);
    });
}

// Requirement "设置来源与约束", Scenario "普通账户不能设置".
#[test]
fn a_signed_account_cannot_set_the_rate() {
    ext(Some(100)).execute_with(|| {
        assert_noop!(
            RefRate::set_rate(RuntimeOrigin::signed(1), AtcPerUsd(110)),
            DispatchError::BadOrigin
        );
        assert_eq!(
            <RefRate as PriceSource>::atc_per_usd(),
            Some(AtcPerUsd(100))
        );
    });
}

// Scenario "幅度内的调整".
#[test]
fn a_change_within_the_band_is_accepted() {
    ext(Some(100)).execute_with(|| {
        System::set_block_number(1 + INTERVAL);
        assert_ok!(set(120));
        assert_eq!(RefRate::rate(), Some((AtcPerUsd(120), 1 + INTERVAL)));
        System::set_block_number(1 + 2 * INTERVAL);
        assert_ok!(set(96)); // exactly −20%
    });
}

// Scenario "超出幅度".
#[test]
fn a_change_beyond_the_band_is_refused() {
    ext(Some(100)).execute_with(|| {
        System::set_block_number(1 + INTERVAL);
        assert_noop!(set(121), Error::<Test>::ChangeTooLarge);
        assert_noop!(set(79), Error::<Test>::ChangeTooLarge);
    });
}

// Scenario "间隔不足".
#[test]
fn a_change_too_soon_is_refused() {
    ext(None).execute_with(|| {
        System::set_block_number(5);
        assert_ok!(set(100));
        System::set_block_number(5 + INTERVAL - 1);
        assert_noop!(set(110), Error::<Test>::TooSoon);
        System::set_block_number(5 + INTERVAL);
        assert_ok!(set(110));
    });
}

// Scenario "首次设置", and a zero rate is always refused.
#[test]
fn the_first_setting_takes_any_positive_rate() {
    ext(None).execute_with(|| {
        assert_noop!(set(0), Error::<Test>::ZeroRate);
        assert_ok!(set(u128::MAX));
        assert_eq!(Rate::<Test>::get(), Some((AtcPerUsd(u128::MAX), 1)));
    });
}

// Requirement "美元与 ATC 的换算", Scenario "未设置时换算失败" (the pallet side; the provider
// registration that depends on it is tested in pallet-providers).
#[test]
fn conversions_fail_without_a_rate() {
    ext(None).execute_with(|| {
        assert_eq!(
            <RefRate as PriceSource>::to_atc(MicroUsd(1), Rounding::Payment),
            Err(PriceError::NotSet)
        );
    });
    ext(Some(3)).execute_with(|| {
        assert_eq!(
            <RefRate as PriceSource>::to_atc(MicroUsd(1), Rounding::Payment),
            Ok(0)
        );
        assert_eq!(
            <RefRate as PriceSource>::to_atc(MicroUsd(1), Rounding::Threshold),
            Ok(1)
        );
    });
}

#[test]
fn the_band_is_exact_at_large_rates() {
    // 1.2 × old overflows 128 bits: every larger representable rate is within the band.
    let big = 306_254_130_228_844_617_117_037_146_688_591_390_309; // ≈ 0.9 × u128::MAX
    assert!(within_band(AtcPerUsd(big), AtcPerUsd(u128::MAX)));
    // ceil(0.8 × u128::MAX) is exactly the lowest accepted value.
    let lowest = 272_225_893_536_750_770_770_699_685_945_414_569_164;
    assert!(within_band(AtcPerUsd(u128::MAX), AtcPerUsd(lowest)));
    assert!(!within_band(AtcPerUsd(u128::MAX), AtcPerUsd(lowest - 1)));
    assert!(within_band(AtcPerUsd(10), AtcPerUsd(8)));
    assert!(!within_band(AtcPerUsd(10), AtcPerUsd(7)));
    assert!(within_band(AtcPerUsd(10), AtcPerUsd(12)));
    assert!(!within_band(AtcPerUsd(10), AtcPerUsd(13)));
}

#[test]
#[should_panic(expected = "invalid reference-rate genesis")]
fn a_zero_interval_genesis_is_refused() {
    use sp_runtime::BuildStorage;
    let _ = crate::mock::RuntimeGenesisConfig {
        system: Default::default(),
        ref_rate: crate::GenesisConfig {
            params: crate::RefRateParams { min_interval: 0 },
            ..Default::default()
        },
    }
    .build_storage();
}
