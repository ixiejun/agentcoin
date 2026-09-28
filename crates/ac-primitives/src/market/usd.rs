//! US-dollar amounts and their conversion to ATC (m5-market-registry design D2).
//!
//! Prices are set in US dollars and settled in ATC (D23): dollar amounts are integers of
//! micro-dollars, the reference rate is the number of smallest ATC units per dollar, and every
//! conversion states its rounding direction.

use parity_scale_codec::{Decode, DecodeWithMemTracking, Encode, MaxEncodedLen};
use scale_info::TypeInfo;
use sp_runtime::helpers_128bit::multiply_by_rational_with_rounding;

/// Micro-dollars in one US dollar.
pub const MICRO_USD_PER_USD: u128 = 1_000_000;

/// A US-dollar amount in micro-dollars (10^-6 USD).
#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Encode,
    Decode,
    DecodeWithMemTracking,
    MaxEncodedLen,
    TypeInfo,
    serde::Serialize,
    serde::Deserialize,
)]
pub struct MicroUsd(pub u128);

impl MicroUsd {
    /// Zero dollars.
    pub const ZERO: Self = Self(0);

    /// `amount` whole dollars, or `None` on overflow.
    #[must_use]
    pub const fn from_usd(amount: u128) -> Option<Self> {
        match amount.checked_mul(MICRO_USD_PER_USD) {
            Some(v) => Some(Self(v)),
            None => None,
        }
    }

    /// `self − other`, or zero when `other` is larger (a cumulative amount that did not grow
    /// authorizes nothing).
    #[must_use]
    pub const fn saturating_sub(self, other: Self) -> Self {
        Self(self.0.saturating_sub(other.0))
    }
}

/// Price of one million tokens, input and output priced separately.
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
pub struct PricePerMTok {
    /// Price of one million input (prompt) tokens.
    pub input: MicroUsd,
    /// Price of one million output (generated) tokens.
    pub output: MicroUsd,
}

impl PricePerMTok {
    /// Both prices are positive.
    #[must_use]
    pub const fn is_positive(&self) -> bool {
        self.input.0 > 0 && self.output.0 > 0
    }
}

/// Reference rate: smallest ATC units per US dollar (1 ATC = 10^18 units; 1 ATC = 2 USD is
/// `5 × 10^17`).
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
    MaxEncodedLen,
    TypeInfo,
    serde::Serialize,
    serde::Deserialize,
)]
pub struct AtcPerUsd(pub u128);

/// Why a dollar amount could not be converted to ATC.
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
#[non_exhaustive]
pub enum PriceError {
    /// No reference rate has been set.
    NotSet,
    /// The result does not fit in 128 bits.
    Overflow,
}

impl core::fmt::Display for PriceError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match self {
            Self::NotSet => "the reference rate is not set",
            Self::Overflow => "the ATC amount overflows",
        })
    }
}

#[cfg(feature = "std")]
impl std::error::Error for PriceError {}

/// Rounding direction of a dollar → ATC conversion.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rounding {
    /// Amounts taken from a payer: rounded down, the payer is never over-charged.
    Payment,
    /// Minimum requirements such as stake thresholds: rounded up, never lowered by rounding.
    Threshold,
}

/// Converts `amount` to smallest ATC units at `rate`.
///
/// The intermediate product is computed in 256 bits, so only a result above `u128::MAX` fails.
///
/// # Errors
///
/// [`PriceError::Overflow`] if the result does not fit in 128 bits.
pub fn to_atc(amount: MicroUsd, rate: AtcPerUsd, rounding: Rounding) -> Result<u128, PriceError> {
    let r = match rounding {
        Rounding::Payment => sp_runtime::Rounding::Down,
        Rounding::Threshold => sp_runtime::Rounding::Up,
    };
    multiply_by_rational_with_rounding(amount.0, rate.0, MICRO_USD_PER_USD, r)
        .ok_or(PriceError::Overflow)
}

/// [`to_atc`] rounding down (payments).
///
/// # Errors
///
/// [`PriceError::Overflow`] if the result does not fit in 128 bits.
pub fn to_atc_payment(amount: MicroUsd, rate: AtcPerUsd) -> Result<u128, PriceError> {
    to_atc(amount, rate, Rounding::Payment)
}

/// [`to_atc`] rounding up (thresholds).
///
/// # Errors
///
/// [`PriceError::Overflow`] if the result does not fit in 128 bits.
pub fn to_atc_threshold(amount: MicroUsd, rate: AtcPerUsd) -> Result<u128, PriceError> {
    to_atc(amount, rate, Rounding::Threshold)
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    // Spec economics/ref-rate, Requirement "美元与 ATC 的换算", Scenario "付款向下取整".
    #[test]
    fn payment_rounds_down() {
        assert_eq!(to_atc_payment(MicroUsd(1), AtcPerUsd(3)), Ok(0));
        assert_eq!(to_atc_payment(MicroUsd(1_000_000), AtcPerUsd(3)), Ok(3));
    }

    // Scenario "门槛向上取整".
    #[test]
    fn threshold_rounds_up() {
        assert_eq!(to_atc_threshold(MicroUsd(1), AtcPerUsd(3)), Ok(1));
        assert_eq!(to_atc_threshold(MicroUsd(1_000_000), AtcPerUsd(3)), Ok(3));
    }

    #[test]
    fn half_an_atc_per_dollar() {
        // 250 micro-dollars at 1 ATC per dollar = 2.5 × 10^14 units.
        let rate = AtcPerUsd(1_000_000_000_000_000_000);
        assert_eq!(to_atc_payment(MicroUsd(250), rate), Ok(250_000_000_000_000));
    }

    #[test]
    fn overflow_is_an_error() {
        assert_eq!(
            to_atc_payment(MicroUsd(u128::MAX), AtcPerUsd(2_000_000)),
            Err(PriceError::Overflow)
        );
        // Large intermediates that still fit are fine.
        assert_eq!(
            to_atc_payment(MicroUsd(u128::MAX), AtcPerUsd(1_000_000)),
            Ok(u128::MAX)
        );
    }

    #[test]
    fn from_usd_and_saturating_sub() {
        assert_eq!(MicroUsd::from_usd(100), Some(MicroUsd(100_000_000)));
        assert_eq!(MicroUsd::from_usd(u128::MAX), None);
        assert_eq!(MicroUsd(5).saturating_sub(MicroUsd(7)), MicroUsd::ZERO);
    }

    proptest! {
        #[test]
        fn threshold_is_at_most_one_above_payment(amount in any::<u64>(), rate in any::<u64>()) {
            let (a, r) = (MicroUsd(u128::from(amount)), AtcPerUsd(u128::from(rate)));
            let down = to_atc_payment(a, r).unwrap();
            let up = to_atc_threshold(a, r).unwrap();
            prop_assert!(up >= down);
            prop_assert!(up.checked_sub(down).is_some_and(|d| d <= 1));
        }
    }
}
