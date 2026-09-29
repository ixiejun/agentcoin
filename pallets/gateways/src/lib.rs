//! # Inference gateways
//!
//! On-chain registration of inference gateways (plan §5.1, §7; m5-market-registry design D6):
//!
//! - Anyone may register a gateway: a service endpoint, a fee in basis points (at most 5%, plan
//!   §8) and a stake of at least $1,000 on live chains, converted at the reference rate rounding
//!   up.
//! - Users escrow credits with an active gateway (`pallet-credits`); an exiting gateway accepts
//!   no new escrow.
//! - Unbonded stake and the stake of an exiting gateway unlock after the unbonding period
//!   (7 days on live chains).
//!
//! Stake is held with [`HoldReason::Stake`].

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

use ac_primitives::market::MicroUsd;
use ac_primitives::market::records::MAX_GATEWAY_FEE_BPS;
use parity_scale_codec::{Decode, DecodeWithMemTracking, Encode, MaxEncodedLen};
use scale_info::TypeInfo;

/// Gateway parameters, set at genesis.
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
pub struct GatewayParams {
    /// Stake threshold.
    pub min_usd: MicroUsd,
    /// Highest fee, in basis points; at most [`MAX_GATEWAY_FEE_BPS`] (5%).
    pub max_fee_bps: u16,
    /// Blocks unbonding stake stays held.
    pub unbond_blocks: u64,
}

impl GatewayParams {
    /// Live-chain values: $1,000 threshold, 5% fee cap, 7-day unbonding.
    pub const LIVE: Self = Self {
        min_usd: MicroUsd(1_000_000_000),
        max_fee_bps: MAX_GATEWAY_FEE_BPS,
        unbond_blocks: 604_800,
    };
}

impl Default for GatewayParams {
    fn default() -> Self {
        Self::LIVE
    }
}

/// What the benchmarks need from the runtime: a reference rate.
#[cfg(feature = "runtime-benchmarks")]
pub trait BenchmarkHelper {
    /// Sets the reference rate to `rate` smallest units per dollar.
    fn set_rate(rate: u128);
}

#[cfg(feature = "runtime-benchmarks")]
impl BenchmarkHelper for () {
    fn set_rate(_: u128) {}
}

// FRAME's `pallet` macro expands to code (genesis defaults, storage metadata, call decoding)
// that uses `expect` / `unreachable!` on our spans; hand-written code here uses neither except
// the documented genesis check.
#[allow(clippy::expect_used, clippy::unreachable)]
#[frame_support::pallet]
pub mod pallet {
    use super::{GatewayParams, MAX_GATEWAY_FEE_BPS, WeightInfo};
    use ac_primitives::market::records::{
        Endpoint, GatewayStatus, UnlockingList, schedule_unlock, take_due, unlocking_total,
    };
    use ac_primitives::market::traits::{GatewayLookup, PriceSource};
    use ac_primitives::market::usd::Rounding;
    use ac_primitives::market::{GatewayRecord, PriceError};
    use frame_support::Identity;
    use frame_support::pallet_prelude::{
        BuildGenesisConfig, DispatchResult, IsType, OptionQuery, StorageMap, StorageValue,
        ValueQuery, ensure,
    };
    use frame_support::traits::fungible::{Inspect, InspectHold, Mutate, MutateHold};
    use frame_support::traits::tokens::Precision;
    use frame_system::pallet_prelude::{BlockNumberFor, OriginFor};
    use sp_runtime::{SaturatedConversion, Saturating};

    /// Balance of the native currency (smallest ATC units).
    pub type Balance = u128;

    /// A gateway record of this runtime.
    pub type RecordOf<T> = GatewayRecord<Balance, BlockNumberFor<T>>;

    #[pallet::pallet]
    pub struct Pallet<T>(_);

    #[pallet::config]
    pub trait Config: frame_system::Config {
        /// The overarching event type.
        #[allow(deprecated)] // Kept explicit for compatibility with the SDK's test tooling.
        type RuntimeEvent: From<Event<Self>> + IsType<<Self as frame_system::Config>::RuntimeEvent>;
        /// The overarching hold reason.
        type RuntimeHoldReason: From<HoldReason>;
        /// The native currency; stake is held with [`HoldReason::Stake`].
        type Currency: Inspect<Self::AccountId, Balance = Balance>
            + Mutate<Self::AccountId>
            + InspectHold<Self::AccountId, Reason = Self::RuntimeHoldReason>
            + MutateHold<Self::AccountId, Reason = Self::RuntimeHoldReason>;
        /// The reference rate.
        type Price: PriceSource;
        /// Weights.
        type WeightInfo: WeightInfo;
        /// Sets a rate for the benchmarks.
        #[cfg(feature = "runtime-benchmarks")]
        type BenchmarkHelper: super::BenchmarkHelper;
    }

    /// Why this pallet holds funds.
    #[pallet::composite_enum]
    pub enum HoldReason {
        /// Bonded or unbonding gateway stake.
        #[codec(index = 0)]
        Stake,
    }

    /// Registered gateways.
    #[pallet::storage]
    pub type Gateways<T: Config> = StorageMap<_, Identity, T::AccountId, RecordOf<T>, OptionQuery>;

    /// Parameters set at genesis.
    #[pallet::storage]
    pub type Params<T: Config> = StorageValue<_, GatewayParams, ValueQuery>;

    #[pallet::genesis_config]
    pub struct GenesisConfig<T: Config> {
        /// Parameters.
        pub params: GatewayParams,
        /// Unused type marker.
        #[serde(skip)]
        pub _marker: core::marker::PhantomData<T>,
    }

    impl<T: Config> Default for GenesisConfig<T> {
        fn default() -> Self {
            Self {
                params: GatewayParams::LIVE,
                _marker: core::marker::PhantomData,
            }
        }
    }

    #[pallet::genesis_build]
    impl<T: Config> BuildGenesisConfig for GenesisConfig<T> {
        fn build(&self) {
            let p = self.params;
            if p.max_fee_bps > MAX_GATEWAY_FEE_BPS || p.unbond_blocks == 0 || p.min_usd.0 == 0 {
                // Genesis is built once, off chain: a bad parameter must stop the chain from
                // being created at all.
                #[allow(clippy::panic)]
                {
                    panic!("invalid gateway genesis parameters: {p:?}");
                }
            }
            Params::<T>::put(p);
        }
    }

    #[pallet::event]
    #[pallet::generate_deposit(pub(super) fn deposit_event)]
    pub enum Event<T: Config> {
        /// A gateway registered.
        Registered {
            /// The gateway.
            who: T::AccountId,
            /// Its fee, in basis points.
            fee_bps: u16,
            /// Initial stake.
            stake: Balance,
        },
        /// A gateway changed its endpoint or fee.
        Updated {
            /// The gateway.
            who: T::AccountId,
        },
        /// Stake was added.
        Bonded {
            /// The gateway.
            who: T::AccountId,
            /// Amount added.
            amount: Balance,
        },
        /// Stake started unbonding.
        Unbonding {
            /// The gateway.
            who: T::AccountId,
            /// Amount.
            amount: Balance,
            /// Block it unlocks at.
            unlock_at: BlockNumberFor<T>,
        },
        /// A gateway is leaving.
        Exiting {
            /// The gateway.
            who: T::AccountId,
        },
        /// Unlocked stake was released.
        Withdrawn {
            /// The gateway.
            who: T::AccountId,
            /// Amount released.
            amount: Balance,
        },
        /// An exited gateway's record was removed.
        Removed {
            /// The former gateway.
            who: T::AccountId,
        },
    }

    #[pallet::error]
    pub enum Error<T> {
        /// The account is already a gateway.
        AlreadyRegistered,
        /// The account is not a gateway.
        NotGateway,
        /// The endpoint is empty or not UTF-8.
        InvalidEndpoint,
        /// The fee exceeds the cap (5%).
        FeeTooHigh,
        /// No reference rate: the dollar threshold cannot be converted.
        RateNotSet,
        /// The threshold overflows when converted.
        PriceOverflow,
        /// The stake would be below the threshold.
        BelowThreshold,
        /// The account cannot cover the stake.
        InsufficientBalance,
        /// An exiting gateway cannot change.
        Exiting,
        /// A zero amount.
        ZeroAmount,
        /// More than the bonded stake.
        AmountTooLarge,
        /// No unbonding stake is due.
        NothingToWithdraw,
    }

    impl<T> From<PriceError> for Error<T> {
        fn from(e: PriceError) -> Self {
            match e {
                PriceError::NotSet => Self::RateNotSet,
                _ => Self::PriceOverflow,
            }
        }
    }

    #[pallet::call]
    impl<T: Config> Pallet<T> {
        /// Registers the caller as a gateway and bonds `stake`.
        #[pallet::call_index(0)]
        #[pallet::weight(T::WeightInfo::register())]
        pub fn register(
            origin: OriginFor<T>,
            endpoint: Endpoint,
            fee_bps: u16,
            stake: Balance,
        ) -> DispatchResult {
            let who = frame_system::ensure_signed(origin)?;
            ensure!(
                !Gateways::<T>::contains_key(&who),
                Error::<T>::AlreadyRegistered
            );
            Self::check_endpoint(&endpoint)?;
            Self::check_fee(fee_bps)?;
            ensure!(
                stake >= Self::threshold().map_err(Error::<T>::from)?,
                Error::<T>::BelowThreshold
            );
            T::Currency::hold(&HoldReason::Stake.into(), &who, stake)
                .map_err(|_| Error::<T>::InsufficientBalance)?;
            Gateways::<T>::insert(
                &who,
                RecordOf::<T> {
                    endpoint,
                    fee_bps,
                    stake,
                    unlocking: UnlockingList::default(),
                    status: GatewayStatus::Active,
                    registered_at: frame_system::Pallet::<T>::block_number(),
                },
            );
            Self::deposit_event(Event::Registered {
                who,
                fee_bps,
                stake,
            });
            Ok(())
        }

        /// Changes the endpoint and/or the fee.
        #[pallet::call_index(1)]
        #[pallet::weight(T::WeightInfo::update())]
        pub fn update(
            origin: OriginFor<T>,
            endpoint: Option<Endpoint>,
            fee_bps: Option<u16>,
        ) -> DispatchResult {
            let who = frame_system::ensure_signed(origin)?;
            let mut g = Gateways::<T>::get(&who).ok_or(Error::<T>::NotGateway)?;
            ensure!(g.status == GatewayStatus::Active, Error::<T>::Exiting);
            if let Some(e) = endpoint {
                Self::check_endpoint(&e)?;
                g.endpoint = e;
            }
            if let Some(f) = fee_bps {
                Self::check_fee(f)?;
                g.fee_bps = f;
            }
            Gateways::<T>::insert(&who, g);
            Self::deposit_event(Event::Updated { who });
            Ok(())
        }

        /// Adds `amount` to the stake.
        #[pallet::call_index(2)]
        #[pallet::weight(T::WeightInfo::bond_extra())]
        pub fn bond_extra(origin: OriginFor<T>, amount: Balance) -> DispatchResult {
            let who = frame_system::ensure_signed(origin)?;
            ensure!(amount > 0, Error::<T>::ZeroAmount);
            let mut g = Gateways::<T>::get(&who).ok_or(Error::<T>::NotGateway)?;
            ensure!(g.status == GatewayStatus::Active, Error::<T>::Exiting);
            T::Currency::hold(&HoldReason::Stake.into(), &who, amount)
                .map_err(|_| Error::<T>::InsufficientBalance)?;
            g.stake = g.stake.saturating_add(amount);
            Gateways::<T>::insert(&who, g);
            Self::deposit_event(Event::Bonded { who, amount });
            Ok(())
        }

        /// Starts unbonding `amount`; the rest must stay at or above the threshold.
        #[pallet::call_index(3)]
        #[pallet::weight(T::WeightInfo::unbond())]
        pub fn unbond(origin: OriginFor<T>, amount: Balance) -> DispatchResult {
            let who = frame_system::ensure_signed(origin)?;
            ensure!(amount > 0, Error::<T>::ZeroAmount);
            let mut g = Gateways::<T>::get(&who).ok_or(Error::<T>::NotGateway)?;
            ensure!(g.status == GatewayStatus::Active, Error::<T>::Exiting);
            ensure!(amount <= g.stake, Error::<T>::AmountTooLarge);
            let rest = g.stake.saturating_sub(amount);
            ensure!(
                rest >= Self::threshold().map_err(Error::<T>::from)?,
                Error::<T>::BelowThreshold
            );
            g.stake = rest;
            let unlock_at = Self::unlock_block();
            schedule_unlock(&mut g.unlocking, amount, unlock_at);
            Gateways::<T>::insert(&who, g);
            Self::deposit_event(Event::Unbonding {
                who,
                amount,
                unlock_at,
            });
            Ok(())
        }

        /// Leaves: no new escrow is accepted and the whole stake starts unbonding.
        #[pallet::call_index(4)]
        #[pallet::weight(T::WeightInfo::exit())]
        pub fn exit(origin: OriginFor<T>) -> DispatchResult {
            let who = frame_system::ensure_signed(origin)?;
            let mut g = Gateways::<T>::get(&who).ok_or(Error::<T>::NotGateway)?;
            ensure!(g.status == GatewayStatus::Active, Error::<T>::Exiting);
            let amount = core::mem::take(&mut g.stake);
            let unlock_at = Self::unlock_block();
            if amount > 0 {
                schedule_unlock(&mut g.unlocking, amount, unlock_at);
            }
            g.status = GatewayStatus::Exiting;
            Gateways::<T>::insert(&who, g);
            Self::deposit_event(Event::Exiting { who: who.clone() });
            Self::deposit_event(Event::Unbonding {
                who,
                amount,
                unlock_at,
            });
            Ok(())
        }

        /// Releases unbonding stake that is due; removes an exited gateway with nothing left.
        #[pallet::call_index(5)]
        #[pallet::weight(T::WeightInfo::withdraw_unbonded())]
        pub fn withdraw_unbonded(origin: OriginFor<T>) -> DispatchResult {
            let who = frame_system::ensure_signed(origin)?;
            let mut g = Gateways::<T>::get(&who).ok_or(Error::<T>::NotGateway)?;
            let due = take_due(&mut g.unlocking, frame_system::Pallet::<T>::block_number());
            ensure!(due > 0, Error::<T>::NothingToWithdraw);
            T::Currency::release(&HoldReason::Stake.into(), &who, due, Precision::BestEffort)?;
            let gone = g.status == GatewayStatus::Exiting && g.stake == 0 && g.unlocking.is_empty();
            if gone {
                Gateways::<T>::remove(&who);
            } else {
                Gateways::<T>::insert(&who, g);
            }
            Self::deposit_event(Event::Withdrawn {
                who: who.clone(),
                amount: due,
            });
            if gone {
                Self::deposit_event(Event::Removed { who });
            }
            Ok(())
        }
    }

    impl<T: Config> Pallet<T> {
        fn check_endpoint(endpoint: &Endpoint) -> DispatchResult {
            ensure!(
                !endpoint.is_empty() && core::str::from_utf8(endpoint).is_ok(),
                Error::<T>::InvalidEndpoint
            );
            Ok(())
        }

        fn check_fee(fee_bps: u16) -> DispatchResult {
            ensure!(
                fee_bps <= Params::<T>::get().max_fee_bps,
                Error::<T>::FeeTooHigh
            );
            Ok(())
        }

        fn unlock_block() -> BlockNumberFor<T> {
            frame_system::Pallet::<T>::block_number()
                .saturating_add(Params::<T>::get().unbond_blocks.saturated_into())
        }

        /// Stake a gateway needs now, rounded up.
        ///
        /// # Errors
        ///
        /// [`PriceError::NotSet`] without a rate; [`PriceError::Overflow`].
        pub fn threshold() -> Result<Balance, PriceError> {
            T::Price::to_atc(Params::<T>::get().min_usd, Rounding::Threshold)
        }

        /// A registered gateway.
        #[must_use]
        pub fn gateway(who: &T::AccountId) -> Option<RecordOf<T>> {
            Gateways::<T>::get(who)
        }

        /// Total stake of `who` (bonded plus unbonding), as held.
        #[must_use]
        pub fn total_stake(who: &T::AccountId) -> Balance {
            Gateways::<T>::get(who)
                .map(|g| g.stake.saturating_add(unlocking_total(&g.unlocking)))
                .unwrap_or(0)
        }
    }

    impl<T: Config> GatewayLookup<T::AccountId> for Pallet<T> {
        fn is_active(who: &T::AccountId) -> bool {
            Gateways::<T>::get(who).is_some_and(|g| g.status == GatewayStatus::Active)
        }

        fn fee_bps(who: &T::AccountId) -> Option<u16> {
            Gateways::<T>::get(who).map(|g| g.fee_bps)
        }

        fn is_registered(who: &T::AccountId) -> bool {
            Gateways::<T>::contains_key(who)
        }
    }
}
