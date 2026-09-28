//! # Model registry
//!
//! Permissionless registration of inference models (plan §5.3; m5-market-registry design D4):
//!
//! - A model ID is the domain-separated BLAKE3 hash of the model's weight manifest (name,
//!   architecture, quantization, ordered shard hashes). The chain computes it; the registrant
//!   cannot choose it, and anyone can recompute it off chain.
//! - Each manifest registers once. A registration may declare a parent model and how the model
//!   derives from it (D28); the parent must be registered. The licence tag is informational
//!   (D6). Royalties are reserved for the full version (D22) and refused.
//! - A registration holds a storage deposit on the registrant (0.01 ATC per item plus 0.0001 ATC
//!   per byte on live chains, the same formula as contract storage). Records are immutable and,
//!   in the MVP, never removed.

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

use ac_primitives::market::ModelId;
use ac_primitives::market::traits::ModelLookup;

// FRAME's `pallet` macro expands to code (storage metadata, call decoding) that uses `expect` /
// `unreachable!` on our spans; hand-written code here uses neither.
#[allow(clippy::expect_used, clippy::unreachable)]
#[frame_support::pallet]
pub mod pallet {
    use super::WeightInfo;
    use ac_primitives::market::ModelId;
    use ac_primitives::market::model::{
        Lineage, MAX_LICENSE_TAG_LEN, ModelManifest, ModelRecord, RoyaltySpec,
    };
    use frame_support::pallet_prelude::{
        ConstU32, DispatchResult, Get, IsType, OptionQuery, StorageMap, ensure,
    };
    use frame_support::traits::fungible::{Inspect, InspectHold, Mutate, MutateHold};
    use frame_support::{BoundedVec, Identity};
    use frame_system::pallet_prelude::{BlockNumberFor, OriginFor};
    use parity_scale_codec::Encode;

    /// Balance of the native currency (smallest ATC units).
    pub type Balance = u128;

    /// A model record of this runtime.
    pub type RecordOf<T> =
        ModelRecord<<T as frame_system::Config>::AccountId, Balance, BlockNumberFor<T>>;

    #[pallet::pallet]
    pub struct Pallet<T>(_);

    #[pallet::config]
    pub trait Config: frame_system::Config {
        /// The overarching event type.
        #[allow(deprecated)] // Kept explicit for compatibility with the SDK's test tooling.
        type RuntimeEvent: From<Event<Self>> + IsType<<Self as frame_system::Config>::RuntimeEvent>;
        /// The overarching hold reason.
        type RuntimeHoldReason: From<HoldReason>;
        /// The native currency; deposits are held with [`HoldReason::Deposit`].
        type Currency: Inspect<Self::AccountId, Balance = Balance>
            + Mutate<Self::AccountId>
            + InspectHold<Self::AccountId, Reason = Self::RuntimeHoldReason>
            + MutateHold<Self::AccountId, Reason = Self::RuntimeHoldReason>;
        /// Deposit per storage item (0.01 ATC on live chains).
        #[pallet::constant]
        type DepositPerItem: Get<Balance>;
        /// Deposit per stored byte (0.0001 ATC on live chains).
        #[pallet::constant]
        type DepositPerByte: Get<Balance>;
        /// Weights.
        type WeightInfo: WeightInfo;
    }

    /// Why this pallet holds funds.
    #[pallet::composite_enum]
    pub enum HoldReason {
        /// Storage deposit of a registered model.
        #[codec(index = 0)]
        Deposit,
    }

    /// Registered models by ID.
    #[pallet::storage]
    pub type Models<T: Config> = StorageMap<_, Identity, ModelId, RecordOf<T>, OptionQuery>;

    #[pallet::event]
    #[pallet::generate_deposit(pub(super) fn deposit_event)]
    pub enum Event<T: Config> {
        /// A model was registered.
        ModelRegistered {
            /// Its ID.
            id: ModelId,
            /// The registrant.
            owner: T::AccountId,
            /// Deposit held.
            deposit: Balance,
        },
    }

    #[pallet::error]
    pub enum Error<T> {
        /// The name or architecture is empty or not UTF-8, or no shard is listed.
        InvalidManifest,
        /// The licence tag is not UTF-8.
        InvalidLicenseTag,
        /// A model with this manifest is already registered.
        AlreadyRegistered,
        /// The declared parent model is not registered.
        UnknownParent,
        /// A model cannot be its own parent.
        SelfParent,
        /// Royalties are reserved for the full version.
        RoyaltyNotEnabled,
        /// The registrant cannot cover the deposit.
        InsufficientBalance,
    }

    #[pallet::call]
    impl<T: Config> Pallet<T> {
        /// Registers a model; its ID is computed from `manifest`.
        #[pallet::call_index(0)]
        #[pallet::weight(T::WeightInfo::register(u32::try_from(manifest.shards.len()).unwrap_or(u32::MAX)))]
        pub fn register(
            origin: OriginFor<T>,
            manifest: ModelManifest,
            lineage: Option<Lineage>,
            license_tag: BoundedVec<u8, ConstU32<MAX_LICENSE_TAG_LEN>>,
            royalty: Option<RoyaltySpec<T::AccountId>>,
        ) -> DispatchResult {
            let owner = frame_system::ensure_signed(origin)?;
            ensure!(royalty.is_none(), Error::<T>::RoyaltyNotEnabled);
            let id = manifest.id().map_err(|_| Error::<T>::InvalidManifest)?;
            ensure!(
                core::str::from_utf8(&license_tag).is_ok(),
                Error::<T>::InvalidLicenseTag
            );
            ensure!(
                !Models::<T>::contains_key(id),
                Error::<T>::AlreadyRegistered
            );
            if let Some(l) = &lineage {
                ensure!(l.parent != id, Error::<T>::SelfParent);
                ensure!(
                    Models::<T>::contains_key(l.parent),
                    Error::<T>::UnknownParent
                );
            }
            let mut record = RecordOf::<T> {
                owner: owner.clone(),
                manifest,
                lineage,
                license_tag,
                royalty: None,
                deposit: 0,
                registered_at: frame_system::Pallet::<T>::block_number(),
            };
            // The deposit is a fixed-width u128, so its value does not change the length.
            let deposit = Self::deposit_for(record.encoded_size());
            record.deposit = deposit;
            T::Currency::hold(&HoldReason::Deposit.into(), &owner, deposit)
                .map_err(|_| Error::<T>::InsufficientBalance)?;
            Models::<T>::insert(id, record);
            Self::deposit_event(Event::ModelRegistered { id, owner, deposit });
            Ok(())
        }
    }

    impl<T: Config> Pallet<T> {
        /// Deposit of one record of `bytes` bytes (saturating: a huge record costs at most
        /// `u128::MAX`).
        #[must_use]
        pub fn deposit_for(bytes: usize) -> Balance {
            let bytes = Balance::try_from(bytes).unwrap_or(Balance::MAX);
            T::DepositPerItem::get().saturating_add(T::DepositPerByte::get().saturating_mul(bytes))
        }

        /// A registered model.
        #[must_use]
        pub fn model(id: &ModelId) -> Option<RecordOf<T>> {
            Models::<T>::get(id)
        }
    }
}

impl<T: Config> ModelLookup for Pallet<T> {
    fn exists(id: &ModelId) -> bool {
        Models::<T>::contains_key(id)
    }
}
