//! # Reference rate
//!
//! The ATC/USD reference rate of the inference market (decisions D23, D29; plan §5.7;
//! m5-market-registry design D3): prices are set in US dollars and settled in ATC at this rate.
//!
//! - The rate is the number of smallest ATC units per US dollar ([`AtcPerUsd`]); it may be unset,
//!   and then every dollar conversion fails.
//! - Only the PoA administration ([`Config::AdminOrigin`]) sets it. Once set, each change stays
//!   within ±20% of the current rate and comes at least [`RefRateParams::min_interval`] blocks
//!   after the previous one (1 day on live chains). M8 moves it to token governance and
//!   guardrails.
//! - Other pallets read it only through [`PriceSource`] (\[reserved\] a multi-source oracle in
//!   the full version).

#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

// Deliberate glob re-export (G.MOD.03 deviation): `construct_runtime` resolves hidden items that
// the `#[pallet]` macro generates inside `pallet`, so they cannot be listed explicitly.
pub use pallet::*;

/// Runs the README example as a doctest.
#[cfg(doctest)]
#[doc = include_str!("../README.md")]
pub struct ReadmeDoctests;

#[cfg(feature = "runtime-benchmarks")]
mod benchmarking;
#[cfg(test)]
mod mock;
#[cfg(test)]
mod tests;
pub mod weights;

pub use weights::WeightInfo;

use ac_primitives::market::AtcPerUsd;
use ac_primitives::market::traits::PriceSource;
use parity_scale_codec::{Decode, DecodeWithMemTracking, Encode, MaxEncodedLen};
use scale_info::TypeInfo;

/// Reference-rate parameters, set at genesis.
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
    serde::Serialize,
    serde::Deserialize,
)]
#[serde(rename_all = "camelCase")]
pub struct RefRateParams {
    /// Fewest blocks between two changes of a set rate.
    pub min_interval: u64,
}

impl RefRateParams {
    /// Live-chain value: one day of one-second blocks.
    pub const LIVE: Self = Self {
        min_interval: 86_400,
    };
}

impl Default for RefRateParams {
    fn default() -> Self {
        Self::LIVE
    }
}

/// Whether `new` is within ±20% of `old`: `0.8 × old ≤ new ≤ 1.2 × old`, compared in integers
/// (`10 × new` against `8 × old` and `12 × old`; widened to avoid overflow).
#[must_use]
pub fn within_band(old: AtcPerUsd, new: AtcPerUsd) -> bool {
    // 10 × u128::MAX needs at most 132 bits; compare the quotient and remainder instead of
    // multiplying: new ≥ 0.8·old ⇔ 10·new ≥ 8·old, done by division with rounding up/down.
    let lower = sp_runtime::helpers_128bit::multiply_by_rational_with_rounding(
        old.0,
        8,
        10,
        sp_runtime::Rounding::Up,
    );
    let upper = sp_runtime::helpers_128bit::multiply_by_rational_with_rounding(
        old.0,
        12,
        10,
        sp_runtime::Rounding::Down,
    );
    let above = lower.is_some_and(|l| new.0 >= l);
    // An upper bound above u128::MAX admits every representable value.
    let below = upper.is_none_or(|u| new.0 <= u);
    above && below
}

// FRAME's `pallet` macro expands to code (genesis defaults, storage metadata) that uses
// `expect` / `unreachable!` on our spans; hand-written code here uses neither except the
// documented genesis check.
#[allow(clippy::expect_used, clippy::unreachable)]
#[frame_support::pallet]
pub mod pallet {
    use super::{AtcPerUsd, RefRateParams, WeightInfo, within_band};
    use frame_support::pallet_prelude::{
        BuildGenesisConfig, DispatchResult, EnsureOrigin, IsType, OptionQuery, StorageValue,
        ValueQuery, ensure,
    };
    use frame_system::pallet_prelude::{BlockNumberFor, OriginFor};
    use sp_runtime::SaturatedConversion;

    #[pallet::pallet]
    pub struct Pallet<T>(_);

    #[pallet::config]
    pub trait Config: frame_system::Config {
        /// The overarching event type.
        #[allow(deprecated)] // Kept explicit for compatibility with the SDK's test tooling.
        type RuntimeEvent: From<Event<Self>> + IsType<<Self as frame_system::Config>::RuntimeEvent>;
        /// Who may set the rate: the PoA administration.
        type AdminOrigin: EnsureOrigin<Self::RuntimeOrigin>;
        /// Weights.
        type WeightInfo: WeightInfo;
    }

    /// The rate and the block it was set at; `None` until first set.
    #[pallet::storage]
    pub type Rate<T: Config> = StorageValue<_, (AtcPerUsd, BlockNumberFor<T>), OptionQuery>;

    /// Parameters set at genesis.
    #[pallet::storage]
    pub type Params<T: Config> = StorageValue<_, RefRateParams, ValueQuery>;

    #[pallet::genesis_config]
    pub struct GenesisConfig<T: Config> {
        /// Initial rate (dev chains); `None` leaves it unset.
        pub initial: Option<AtcPerUsd>,
        /// Parameters.
        pub params: RefRateParams,
        /// Unused type marker.
        #[serde(skip)]
        pub _marker: core::marker::PhantomData<T>,
    }

    impl<T: Config> Default for GenesisConfig<T> {
        fn default() -> Self {
            Self {
                initial: None,
                params: RefRateParams::LIVE,
                _marker: core::marker::PhantomData,
            }
        }
    }

    #[pallet::genesis_build]
    impl<T: Config> BuildGenesisConfig for GenesisConfig<T> {
        fn build(&self) {
            if self.params.min_interval == 0 || self.initial.is_some_and(|r| r.0 == 0) {
                // Genesis is built once, off chain: a bad parameter must stop the chain from
                // being created at all.
                #[allow(clippy::panic)]
                {
                    panic!(
                        "invalid reference-rate genesis: {self:?}",
                        self = self.params
                    );
                }
            }
            Params::<T>::put(self.params);
            if let Some(rate) = self.initial {
                Rate::<T>::put((rate, BlockNumberFor::<T>::default()));
            }
        }
    }

    #[pallet::event]
    #[pallet::generate_deposit(pub(super) fn deposit_event)]
    pub enum Event<T: Config> {
        /// The reference rate was set.
        RateSet {
            /// New rate.
            rate: AtcPerUsd,
            /// Block it was set at.
            at: BlockNumberFor<T>,
        },
    }

    #[pallet::error]
    pub enum Error<T> {
        /// A rate of zero.
        ZeroRate,
        /// More than ±20% away from the current rate.
        ChangeTooLarge,
        /// Less than the minimum interval since the last change.
        TooSoon,
    }

    #[pallet::call]
    impl<T: Config> Pallet<T> {
        /// Sets the reference rate (administration only).
        #[pallet::call_index(0)]
        #[pallet::weight(T::WeightInfo::set_rate())]
        pub fn set_rate(origin: OriginFor<T>, rate: AtcPerUsd) -> DispatchResult {
            T::AdminOrigin::ensure_origin(origin)?;
            ensure!(rate.0 > 0, Error::<T>::ZeroRate);
            let now = frame_system::Pallet::<T>::block_number();
            if let Some((old, at)) = Rate::<T>::get() {
                ensure!(within_band(old, rate), Error::<T>::ChangeTooLarge);
                let elapsed = now
                    .saturated_into::<u64>()
                    .saturating_sub(at.saturated_into());
                ensure!(
                    elapsed >= Params::<T>::get().min_interval,
                    Error::<T>::TooSoon
                );
            }
            Rate::<T>::put((rate, now));
            Self::deposit_event(Event::RateSet { rate, at: now });
            Ok(())
        }
    }
}

impl<T: Config> Pallet<T> {
    /// The rate and the block it was set at.
    #[must_use]
    pub fn rate() -> Option<(AtcPerUsd, frame_system::pallet_prelude::BlockNumberFor<T>)> {
        Rate::<T>::get()
    }
}

impl<T: Config> PriceSource for Pallet<T> {
    fn atc_per_usd() -> Option<AtcPerUsd> {
        Rate::<T>::get().map(|(rate, _)| rate)
    }
}
