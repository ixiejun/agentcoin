//! # Inference providers
//!
//! On-chain registration of inference providers (plan §5.3; m5-market-registry design D5):
//!
//! - A provider registers its tier (T1 data center or T2 consumer; T0 and TEE attestations are
//!   reserved), a service endpoint, an X-Wing key gateways encrypt requests to, and 1–16
//!   registered models with dollar prices per million tokens.
//! - Stake thresholds are in US dollars (T1 $1,000, T2 $100 on live chains), converted at the
//!   reference rate rounding up. After a rate change a provider below its threshold stays
//!   registered but is not *serviceable* until it bonds more.
//! - Heartbeats keep a provider serviceable: at most two intervals may pass (600 blocks each on
//!   live chains). A heartbeat at least half an interval after the last one is free.
//! - Unbonded stake and the stake of an exiting provider unlock after the unbonding period
//!   (7 days on live chains) and stay slashable until then.
//! - [`ProviderPenalty`] (slash and jail) is for the audit module (M6); no call reaches it, not
//!   even the PoA administration (D6).
//!
//! Stake is held with [`HoldReason::Stake`]; slashed stake is burned through [`Config::Slash`].

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
use parity_scale_codec::{Decode, DecodeWithMemTracking, Encode, MaxEncodedLen};
use scale_info::TypeInfo;

/// Provider parameters, set at genesis.
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
pub struct ProviderParams {
    /// Stake threshold of T1 providers.
    pub t1_min_usd: MicroUsd,
    /// Stake threshold of T2 providers.
    pub t2_min_usd: MicroUsd,
    /// Heartbeat interval in blocks; two intervals without one make a provider unserviceable.
    pub heartbeat_interval: u64,
    /// Blocks unbonding stake stays held (and slashable).
    pub unbond_blocks: u64,
}

impl ProviderParams {
    /// Live-chain values: $1,000 / $100 thresholds, 10-minute heartbeats, 7-day unbonding.
    pub const LIVE: Self = Self {
        t1_min_usd: MicroUsd(1_000_000_000),
        t2_min_usd: MicroUsd(100_000_000),
        heartbeat_interval: 600,
        unbond_blocks: 604_800,
    };
}

impl Default for ProviderParams {
    fn default() -> Self {
        Self::LIVE
    }
}

/// A provider registration: everything `register` needs besides the caller (one argument
/// instead of six, G.FUD.01).
#[derive(
    Clone, Debug, PartialEq, Eq, Encode, Decode, DecodeWithMemTracking, MaxEncodedLen, TypeInfo,
)]
pub struct Registration {
    /// Tier (T1 or T2; T0 is reserved).
    pub tier: ac_primitives::market::records::Tier,
    /// Where gateways connect.
    pub endpoint: ac_primitives::market::records::Endpoint,
    /// X-Wing key gateways encrypt requests to.
    pub kem_pk: ac_crypto::KemPublicKey,
    /// Served models and prices.
    pub models: frame_support::BoundedVec<
        ac_primitives::market::records::ModelPrice,
        frame_support::pallet_prelude::ConstU32<
            { ac_primitives::market::records::MAX_PROVIDER_MODELS },
        >,
    >,
    /// Initial stake, in smallest ATC units.
    pub stake: u128,
    /// \[Reserved\] TEE attestation; must be `None`.
    pub attestation: Option<ac_primitives::market::records::AttestationRef>,
}

/// What the benchmarks need from the runtime: registered models and a reference rate.
#[cfg(feature = "runtime-benchmarks")]
pub trait BenchmarkHelper {
    /// Makes `id` a registered model.
    fn register_model(id: ac_primitives::market::ModelId);
    /// Sets the reference rate to `rate` smallest units per dollar.
    fn set_rate(rate: u128);
}

#[cfg(feature = "runtime-benchmarks")]
impl BenchmarkHelper for () {
    fn register_model(_: ac_primitives::market::ModelId) {}
    fn set_rate(_: u128) {}
}

// FRAME's `pallet` macro expands to code (genesis defaults, storage metadata, call decoding)
// that uses `expect` / `unreachable!` on our spans; hand-written code here uses neither except
// the documented genesis check.
#[allow(clippy::expect_used, clippy::unreachable)]
#[frame_support::pallet]
pub mod pallet {
    use super::{ProviderParams, Registration, WeightInfo};
    use ac_crypto::{KemAlg, KemPublicKey};
    use ac_primitives::market::records::{
        Endpoint, MAX_PROVIDER_MODELS, ModelPrice, ProviderStatus, SlaMetrics, Tier, UnlockingList,
        schedule_unlock, take_due, take_for_slash, unlocking_total,
    };
    use ac_primitives::market::traits::{ModelLookup, OnJail, PriceSource};
    use ac_primitives::market::usd::Rounding;
    use ac_primitives::market::{ModelId, PriceError, ProviderRecord};
    use alloc::collections::BTreeSet;
    use alloc::vec::Vec;
    use frame_support::dispatch::{DispatchResultWithPostInfo, Pays};
    use frame_support::pallet_prelude::{
        BuildGenesisConfig, ConstU32, DispatchResult, IsType, OptionQuery, StorageDoubleMap,
        StorageMap, StorageValue, ValueQuery, ensure,
    };
    use frame_support::traits::fungible::{
        BalancedHold, Credit, Inspect, InspectHold, Mutate, MutateHold,
    };
    use frame_support::traits::tokens::Precision;
    use frame_support::traits::{Imbalance, OnUnbalanced};
    use frame_support::{BoundedVec, Identity};
    use frame_system::pallet_prelude::{BlockNumberFor, OriginFor};
    use sp_runtime::{Perbill, SaturatedConversion, Saturating};

    /// Balance of the native currency (smallest ATC units).
    pub type Balance = u128;

    /// A provider record of this runtime.
    pub type RecordOf<T> = ProviderRecord<Balance, BlockNumberFor<T>>;

    /// A provider's model list.
    pub type ModelList = BoundedVec<ModelPrice, ConstU32<MAX_PROVIDER_MODELS>>;

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
            + MutateHold<Self::AccountId, Reason = Self::RuntimeHoldReason>
            + BalancedHold<Self::AccountId, Reason = Self::RuntimeHoldReason>;
        /// Registered models.
        type Models: ModelLookup;
        /// The reference rate.
        type Price: PriceSource;
        /// Where slashed stake goes; the runtime burns it through `Emission`.
        type Slash: OnUnbalanced<Credit<Self::AccountId, Self::Currency>>;
        /// Told when a provider is jailed (work settlement voids its unmatured earnings).
        type OnJail: OnJail<Self::AccountId>;
        /// Weights.
        type WeightInfo: WeightInfo;
        /// Registers models and sets a rate for the benchmarks.
        #[cfg(feature = "runtime-benchmarks")]
        type BenchmarkHelper: super::BenchmarkHelper;
    }

    /// Why this pallet holds funds.
    #[pallet::composite_enum]
    pub enum HoldReason {
        /// Bonded or unbonding provider stake.
        #[codec(index = 0)]
        Stake,
    }

    /// Registered providers.
    #[pallet::storage]
    pub type Providers<T: Config> = StorageMap<_, Identity, T::AccountId, RecordOf<T>, OptionQuery>;

    /// Providers of each model (active or jailed providers only; exiting ones are removed).
    /// `Identity` keys iterate in account order; account IDs are hashes, so keys cannot be
    /// chosen to unbalance the trie.
    #[pallet::storage]
    pub type ModelProviders<T: Config> =
        StorageDoubleMap<_, Identity, ModelId, Identity, T::AccountId, (), OptionQuery>;

    /// Parameters set at genesis.
    #[pallet::storage]
    pub type Params<T: Config> = StorageValue<_, ProviderParams, ValueQuery>;

    #[pallet::genesis_config]
    pub struct GenesisConfig<T: Config> {
        /// Parameters.
        pub params: ProviderParams,
        /// Unused type marker.
        #[serde(skip)]
        pub _marker: core::marker::PhantomData<T>,
    }

    impl<T: Config> Default for GenesisConfig<T> {
        fn default() -> Self {
            Self {
                params: ProviderParams::LIVE,
                _marker: core::marker::PhantomData,
            }
        }
    }

    #[pallet::genesis_build]
    impl<T: Config> BuildGenesisConfig for GenesisConfig<T> {
        fn build(&self) {
            let p = self.params;
            if p.heartbeat_interval < 2 || p.unbond_blocks == 0 || p.t2_min_usd.0 == 0 {
                // Genesis is built once, off chain: a bad parameter must stop the chain from
                // being created at all.
                #[allow(clippy::panic)]
                {
                    panic!("invalid provider genesis parameters: {p:?}");
                }
            }
            Params::<T>::put(p);
        }
    }

    #[pallet::event]
    #[pallet::generate_deposit(pub(super) fn deposit_event)]
    pub enum Event<T: Config> {
        /// A provider registered.
        Registered {
            /// The provider.
            who: T::AccountId,
            /// Its tier.
            tier: Tier,
            /// Initial stake.
            stake: Balance,
        },
        /// A provider changed its endpoint, key or models.
        Updated {
            /// The provider.
            who: T::AccountId,
        },
        /// A provider sent a heartbeat.
        Heartbeat {
            /// The provider.
            who: T::AccountId,
        },
        /// Stake was added.
        Bonded {
            /// The provider.
            who: T::AccountId,
            /// Amount added.
            amount: Balance,
        },
        /// Stake started unbonding.
        Unbonding {
            /// The provider.
            who: T::AccountId,
            /// Amount.
            amount: Balance,
            /// Block it unlocks at.
            unlock_at: BlockNumberFor<T>,
        },
        /// A provider is leaving.
        Exiting {
            /// The provider.
            who: T::AccountId,
        },
        /// Unlocked stake was released.
        Withdrawn {
            /// The provider.
            who: T::AccountId,
            /// Amount released.
            amount: Balance,
        },
        /// An exited provider's record was removed.
        Removed {
            /// The former provider.
            who: T::AccountId,
        },
        /// Stake was slashed and burned.
        Slashed {
            /// The provider.
            who: T::AccountId,
            /// Amount burned.
            amount: Balance,
        },
        /// A provider was jailed.
        Jailed {
            /// The provider.
            who: T::AccountId,
        },
    }

    #[pallet::error]
    pub enum Error<T> {
        /// The account is already a provider.
        AlreadyRegistered,
        /// The account is not a provider.
        NotProvider,
        /// T0 (TEE) is reserved for the full version.
        TierNotEnabled,
        /// TEE attestations are reserved for the full version.
        AttestationNotEnabled,
        /// The endpoint is empty or not UTF-8.
        InvalidEndpoint,
        /// Only X-Wing encryption keys are accepted.
        UnsupportedKem,
        /// A provider serves at least one model.
        NoModels,
        /// A listed model is not registered.
        UnknownModel,
        /// A model is listed twice.
        DuplicateModel,
        /// Prices must be positive.
        ZeroPrice,
        /// No reference rate: the dollar threshold cannot be converted.
        RateNotSet,
        /// The threshold overflows when converted.
        PriceOverflow,
        /// The stake would be below the tier's threshold.
        BelowThreshold,
        /// The account cannot cover the stake.
        InsufficientBalance,
        /// An exiting provider cannot change.
        Exiting,
        /// Only active providers send heartbeats.
        NotActive,
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
        /// Registers the caller as a provider and bonds `stake`.
        #[pallet::call_index(0)]
        #[pallet::weight(T::WeightInfo::register(u32::try_from(registration.models.len()).unwrap_or(u32::MAX)))]
        pub fn register(origin: OriginFor<T>, registration: Registration) -> DispatchResult {
            let who = frame_system::ensure_signed(origin)?;
            let Registration {
                tier,
                endpoint,
                kem_pk,
                models,
                stake,
                attestation,
            } = registration;
            ensure!(
                !Providers::<T>::contains_key(&who),
                Error::<T>::AlreadyRegistered
            );
            ensure!(tier != Tier::T0, Error::<T>::TierNotEnabled);
            ensure!(attestation.is_none(), Error::<T>::AttestationNotEnabled);
            Self::check_endpoint(&endpoint)?;
            Self::check_kem(&kem_pk)?;
            Self::check_models(&models)?;
            ensure!(
                stake >= Self::threshold(tier).map_err(Error::<T>::from)?,
                Error::<T>::BelowThreshold
            );
            T::Currency::hold(&HoldReason::Stake.into(), &who, stake)
                .map_err(|_| Error::<T>::InsufficientBalance)?;
            let now = frame_system::Pallet::<T>::block_number();
            for m in &models {
                ModelProviders::<T>::insert(m.model, &who, ());
            }
            Providers::<T>::insert(
                &who,
                RecordOf::<T> {
                    tier,
                    endpoint,
                    kem_pk,
                    models,
                    stake,
                    unlocking: UnlockingList::default(),
                    status: ProviderStatus::Active,
                    last_heartbeat: now,
                    metrics: SlaMetrics::default(),
                    attestation: None,
                    registered_at: now,
                },
            );
            Self::deposit_event(Event::Registered { who, tier, stake });
            Ok(())
        }

        /// Changes the endpoint, the encryption key and/or the model list.
        #[pallet::call_index(1)]
        #[pallet::weight(T::WeightInfo::update(MAX_PROVIDER_MODELS))]
        pub fn update(
            origin: OriginFor<T>,
            endpoint: Option<Endpoint>,
            kem_pk: Option<KemPublicKey>,
            models: Option<ModelList>,
        ) -> DispatchResult {
            let who = frame_system::ensure_signed(origin)?;
            let mut p = Providers::<T>::get(&who).ok_or(Error::<T>::NotProvider)?;
            ensure!(p.status != ProviderStatus::Exiting, Error::<T>::Exiting);
            if let Some(e) = endpoint {
                Self::check_endpoint(&e)?;
                p.endpoint = e;
            }
            if let Some(k) = kem_pk {
                Self::check_kem(&k)?;
                p.kem_pk = k;
            }
            if let Some(list) = models {
                Self::check_models(&list)?;
                for m in &p.models {
                    ModelProviders::<T>::remove(m.model, &who);
                }
                for m in &list {
                    ModelProviders::<T>::insert(m.model, &who, ());
                }
                p.models = list;
            }
            Providers::<T>::insert(&who, p);
            Self::deposit_event(Event::Updated { who });
            Ok(())
        }

        /// Records a heartbeat. Free when at least half an interval passed since the last one.
        #[pallet::call_index(2)]
        #[pallet::weight(T::WeightInfo::heartbeat())]
        pub fn heartbeat(origin: OriginFor<T>) -> DispatchResultWithPostInfo {
            let who = frame_system::ensure_signed(origin)?;
            let mut p = Providers::<T>::get(&who).ok_or(Error::<T>::NotProvider)?;
            ensure!(p.status == ProviderStatus::Active, Error::<T>::NotActive);
            let now = frame_system::Pallet::<T>::block_number();
            let elapsed = Self::blocks_between(p.last_heartbeat, now);
            let pays = if elapsed >= Params::<T>::get().heartbeat_interval / 2 {
                Pays::No
            } else {
                Pays::Yes
            };
            p.last_heartbeat = now;
            Providers::<T>::insert(&who, p);
            Self::deposit_event(Event::Heartbeat { who });
            Ok(pays.into())
        }

        /// Adds `amount` to the stake.
        #[pallet::call_index(3)]
        #[pallet::weight(T::WeightInfo::bond_extra())]
        pub fn bond_extra(origin: OriginFor<T>, amount: Balance) -> DispatchResult {
            let who = frame_system::ensure_signed(origin)?;
            ensure!(amount > 0, Error::<T>::ZeroAmount);
            let mut p = Providers::<T>::get(&who).ok_or(Error::<T>::NotProvider)?;
            ensure!(p.status != ProviderStatus::Exiting, Error::<T>::Exiting);
            T::Currency::hold(&HoldReason::Stake.into(), &who, amount)
                .map_err(|_| Error::<T>::InsufficientBalance)?;
            p.stake = p.stake.saturating_add(amount);
            Providers::<T>::insert(&who, p);
            Self::deposit_event(Event::Bonded { who, amount });
            Ok(())
        }

        /// Starts unbonding `amount`; the rest must stay at or above the threshold.
        #[pallet::call_index(4)]
        #[pallet::weight(T::WeightInfo::unbond())]
        pub fn unbond(origin: OriginFor<T>, amount: Balance) -> DispatchResult {
            let who = frame_system::ensure_signed(origin)?;
            ensure!(amount > 0, Error::<T>::ZeroAmount);
            let mut p = Providers::<T>::get(&who).ok_or(Error::<T>::NotProvider)?;
            ensure!(p.status != ProviderStatus::Exiting, Error::<T>::Exiting);
            ensure!(amount <= p.stake, Error::<T>::AmountTooLarge);
            let rest = p.stake.saturating_sub(amount);
            ensure!(
                rest >= Self::threshold(p.tier).map_err(Error::<T>::from)?,
                Error::<T>::BelowThreshold
            );
            p.stake = rest;
            let unlock_at = Self::unlock_block();
            schedule_unlock(&mut p.unlocking, amount, unlock_at);
            Providers::<T>::insert(&who, p);
            Self::deposit_event(Event::Unbonding {
                who,
                amount,
                unlock_at,
            });
            Ok(())
        }

        /// Leaves: the provider stops being serviceable and its whole stake starts unbonding.
        #[pallet::call_index(5)]
        #[pallet::weight(T::WeightInfo::exit(MAX_PROVIDER_MODELS))]
        pub fn exit(origin: OriginFor<T>) -> DispatchResult {
            let who = frame_system::ensure_signed(origin)?;
            let mut p = Providers::<T>::get(&who).ok_or(Error::<T>::NotProvider)?;
            ensure!(p.status != ProviderStatus::Exiting, Error::<T>::Exiting);
            for m in &p.models {
                ModelProviders::<T>::remove(m.model, &who);
            }
            let amount = core::mem::take(&mut p.stake);
            let unlock_at = Self::unlock_block();
            if amount > 0 {
                schedule_unlock(&mut p.unlocking, amount, unlock_at);
            }
            p.status = ProviderStatus::Exiting;
            Providers::<T>::insert(&who, p);
            Self::deposit_event(Event::Exiting { who: who.clone() });
            Self::deposit_event(Event::Unbonding {
                who,
                amount,
                unlock_at,
            });
            Ok(())
        }

        /// Releases unbonding stake that is due; removes an exited provider with nothing left.
        #[pallet::call_index(6)]
        #[pallet::weight(T::WeightInfo::withdraw_unbonded())]
        pub fn withdraw_unbonded(origin: OriginFor<T>) -> DispatchResult {
            let who = frame_system::ensure_signed(origin)?;
            let mut p = Providers::<T>::get(&who).ok_or(Error::<T>::NotProvider)?;
            let due = take_due(&mut p.unlocking, frame_system::Pallet::<T>::block_number());
            ensure!(due > 0, Error::<T>::NothingToWithdraw);
            T::Currency::release(&HoldReason::Stake.into(), &who, due, Precision::BestEffort)?;
            let gone =
                p.status == ProviderStatus::Exiting && p.stake == 0 && p.unlocking.is_empty();
            if gone {
                Providers::<T>::remove(&who);
            } else {
                Providers::<T>::insert(&who, p);
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

        fn check_kem(key: &KemPublicKey) -> DispatchResult {
            // Other KEM AlgIds do not even decode today; this keeps the rule explicit when
            // `ac-crypto` enables more.
            ensure!(key.alg() == KemAlg::XWing, Error::<T>::UnsupportedKem);
            Ok(())
        }

        fn check_models(models: &ModelList) -> DispatchResult {
            ensure!(!models.is_empty(), Error::<T>::NoModels);
            let mut seen = BTreeSet::new();
            for m in models {
                ensure!(seen.insert(m.model), Error::<T>::DuplicateModel);
                ensure!(T::Models::exists(&m.model), Error::<T>::UnknownModel);
                ensure!(m.price.is_positive(), Error::<T>::ZeroPrice);
            }
            Ok(())
        }

        fn blocks_between(from: BlockNumberFor<T>, to: BlockNumberFor<T>) -> u64 {
            to.saturated_into::<u64>()
                .saturating_sub(from.saturated_into::<u64>())
        }

        fn unlock_block() -> BlockNumberFor<T> {
            frame_system::Pallet::<T>::block_number()
                .saturating_add(Params::<T>::get().unbond_blocks.saturated_into())
        }

        /// Stake a provider of `tier` needs now, rounded up.
        ///
        /// # Errors
        ///
        /// [`PriceError::NotSet`] without a rate; [`PriceError::Overflow`].
        pub fn threshold(tier: Tier) -> Result<Balance, PriceError> {
            let params = Params::<T>::get();
            let usd = match tier {
                Tier::T1 => params.t1_min_usd,
                // T0 is refused at registration; it would share T1's threshold.
                Tier::T0 => params.t1_min_usd,
                Tier::T2 => params.t2_min_usd,
            };
            T::Price::to_atc(usd, Rounding::Threshold)
        }

        /// A registered provider.
        #[must_use]
        pub fn provider(who: &T::AccountId) -> Option<RecordOf<T>> {
            Providers::<T>::get(who)
        }

        /// Whether `record` is serviceable at the current block.
        #[must_use]
        pub fn record_serviceable(record: &RecordOf<T>) -> bool {
            let now = frame_system::Pallet::<T>::block_number();
            let interval = Params::<T>::get().heartbeat_interval;
            record.status == ProviderStatus::Active
                && Self::threshold(record.tier).is_ok_and(|t| record.stake >= t)
                && Self::blocks_between(record.last_heartbeat, now) <= interval.saturating_mul(2)
        }

        /// Whether `who` is serviceable now.
        #[must_use]
        pub fn is_serviceable(who: &T::AccountId) -> bool {
            Providers::<T>::get(who).is_some_and(|p| Self::record_serviceable(&p))
        }

        /// Serviceable providers of `model` in account order, after `start_after`, at most
        /// `limit` (capped at 256). For the runtime API only: it iterates storage.
        #[must_use]
        pub fn serviceable_providers(
            model: ModelId,
            start_after: Option<T::AccountId>,
            limit: u32,
        ) -> Vec<(T::AccountId, RecordOf<T>)> {
            let limit = usize::try_from(limit.min(256)).unwrap_or(256);
            let keys = match start_after {
                Some(after) => ModelProviders::<T>::iter_key_prefix_from(
                    model,
                    ModelProviders::<T>::hashed_key_for(model, after),
                ),
                None => ModelProviders::<T>::iter_key_prefix(model),
            };
            keys.filter_map(|who| {
                let p = Providers::<T>::get(&who)?;
                Self::record_serviceable(&p).then_some((who, p))
            })
            .take(limit)
            .collect()
        }

        /// Total stake of `who` (bonded plus unbonding), as held.
        #[must_use]
        pub fn total_stake(who: &T::AccountId) -> Balance {
            Providers::<T>::get(who)
                .map(|p| p.stake.saturating_add(unlocking_total(&p.unlocking)))
                .unwrap_or(0)
        }
    }

    impl<T: Config> ac_primitives::market::traits::ProviderPenalty<T::AccountId, Balance>
        for Pallet<T>
    {
        fn slash(who: &T::AccountId, ratio: Perbill) -> Balance {
            let Some(mut p) = Providers::<T>::get(who) else {
                return 0;
            };
            let total = p.stake.saturating_add(unlocking_total(&p.unlocking));
            let target = ratio.mul_floor(total);
            let taken = take_for_slash(&mut p.stake, &mut p.unlocking, target);
            let (credit, _) = T::Currency::slash(&HoldReason::Stake.into(), who, taken);
            let burned = credit.peek();
            T::Slash::on_unbalanced(credit);
            if p.status == ProviderStatus::Exiting && p.stake == 0 && p.unlocking.is_empty() {
                Providers::<T>::remove(who);
            } else {
                Providers::<T>::insert(who, p);
            }
            Self::deposit_event(Event::Slashed {
                who: who.clone(),
                amount: burned,
            });
            burned
        }

        fn jail(who: &T::AccountId) -> DispatchResult {
            let mut p = Providers::<T>::get(who).ok_or(Error::<T>::NotProvider)?;
            for m in &p.models {
                ModelProviders::<T>::remove(m.model, who);
            }
            if p.status == ProviderStatus::Active {
                p.status = ProviderStatus::Jailed;
            }
            Providers::<T>::insert(who, p);
            Self::deposit_event(Event::Jailed { who: who.clone() });
            T::OnJail::on_jail(who);
            Ok(())
        }
    }
}
