//! # Aura-PQ
//!
//! Runtime side of Aura-PQ (plan §4.1): the ML-DSA-65 authority set and the current slot.
//! Block validity (slot order, author, seal) is enforced by the node's import verifier, as in
//! upstream Aura; the runtime only records the slot and never panics during block execution
//! (AGENT.md §8). Inconsistencies seen here are logged.
//!
//! The authority set comes from genesis (PoA) and changes only at epoch boundaries, through
//! the validator-set pallet ([`ac_primitives::validator_set::BlockAuthorities`]; the M1
//! [`AuthoritySetWriter`] remains as an alias for M3's PoA → PoS state machine). The author of
//! each block is recorded in [`CurrentAuthor`] for pallets that act on the author's behalf
//! (commit–reveal randomness).

#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

// Deliberate glob re-export (G.MOD.03 deviation): `construct_runtime` resolves hidden items that
// the `#[pallet]` macro generates inside `pallet`, so they cannot be listed explicitly.
pub use pallet::*;

/// Runs the README example as a doctest.
#[cfg(doctest)]
#[doc = include_str!("../README.md")]
pub struct ReadmeDoctests;

#[cfg(test)]
mod mock;
#[cfg(test)]
mod tests;

use alloc::vec::Vec;

use ac_crypto::PqPublicKey;
pub use ac_primitives::aura_pq::{AuthorityError, Slot};

const LOG_TARGET: &str = "runtime::aura-pq";

/// Replaces the authority set; implemented by this pallet and used by M3's validator set.
pub trait AuthoritySetWriter {
    /// Validates and installs `authorities` (in slot-assignment order).
    ///
    /// # Errors
    ///
    /// [`AuthorityError`] if the set is empty, too large, has duplicates or non-ML-DSA-65 keys.
    fn write_authorities(authorities: Vec<PqPublicKey>) -> Result<(), AuthorityError>;
}

// FRAME's `pallet` macro expands to code (genesis defaults, storage metadata) that uses
// `expect` / `unreachable!` on our spans; hand-written code here uses neither.
#[allow(clippy::expect_used, clippy::unreachable)]
#[frame_support::pallet]
pub mod pallet {
    use super::{AuthorityError, AuthoritySetWriter, LOG_TARGET, PqPublicKey, Slot, Vec};
    use ac_primitives::aura_pq::{find_slot, slot_author, validate_authorities};
    use ac_primitives::validator_set::{BlockAuthorities, CurrentAuthor as CurrentAuthorTrait};
    use frame_support::pallet_prelude::OptionQuery;
    use frame_support::pallet_prelude::{
        BoundedVec, BuildGenesisConfig, Get, Hooks, StorageValue, ValueQuery, Weight,
    };
    use frame_system::pallet_prelude::BlockNumberFor;

    #[pallet::pallet]
    pub struct Pallet<T>(_);

    #[pallet::config]
    pub trait Config: frame_system::Config {
        /// Slot duration in milliseconds.
        #[pallet::constant]
        type SlotDuration: Get<u64>;
        /// Maximum number of authorities.
        #[pallet::constant]
        type MaxAuthorities: Get<u32>;
    }

    /// Authorities in slot-assignment order: slot `s` belongs to index `s mod N`.
    #[pallet::storage]
    pub type Authorities<T: Config> =
        StorageValue<_, BoundedVec<PqPublicKey, T::MaxAuthorities>, ValueQuery>;

    /// Slot of the current block.
    #[pallet::storage]
    pub type CurrentSlot<T: Config> = StorageValue<_, Slot, ValueQuery>;

    /// Author of the current block: the authority of its slot under the list in force when the
    /// block started (before any change made while executing it).
    #[pallet::storage]
    pub type CurrentAuthor<T: Config> = StorageValue<_, PqPublicKey, OptionQuery>;

    #[pallet::genesis_config]
    #[derive(frame_support::DefaultNoBound)]
    pub struct GenesisConfig<T: Config> {
        /// Initial authorities; ML-DSA-65 only, no duplicates.
        pub authorities: Vec<PqPublicKey>,
        #[serde(skip)]
        /// Marker.
        pub _config: core::marker::PhantomData<T>,
    }

    #[pallet::genesis_build]
    impl<T: Config> BuildGenesisConfig for GenesisConfig<T> {
        // Genesis building has no error channel: aborting is how a chain-spec build fails, and
        // it never runs during block execution.
        #[allow(clippy::panic)]
        fn build(&self) {
            // The empty default exists for SDK tooling (default genesis); the node refuses to
            // start a chain without authorities. Any configured list is validated strictly.
            if self.authorities.is_empty() {
                return;
            }
            if let Err(e) = Pallet::<T>::install(self.authorities.clone()) {
                panic!("invalid Aura-PQ genesis authorities: {e}");
            }
        }
    }

    #[pallet::hooks]
    impl<T: Config> Hooks<BlockNumberFor<T>> for Pallet<T> {
        fn on_initialize(_n: BlockNumberFor<T>) -> Weight {
            let Ok(slot) = find_slot(&frame_system::Pallet::<T>::digest()) else {
                CurrentAuthor::<T>::kill();
                return T::DbWeight::get().reads_writes(1, 1);
            };
            if slot <= CurrentSlot::<T>::get() {
                log::error!(target: LOG_TARGET, "slot {slot:?} does not increase; the import verifier should have rejected this block");
            }
            CurrentSlot::<T>::put(slot);
            // This pallet initializes before the validator set, so an authority change enacted
            // in this block does not affect who authored it.
            match slot_author(slot, &Authorities::<T>::get()) {
                Some(author) => CurrentAuthor::<T>::put(author.clone()),
                None => CurrentAuthor::<T>::kill(),
            }
            T::DbWeight::get().reads_writes(3, 2)
        }
    }

    impl<T: Config> Pallet<T> {
        /// Current authorities in slot-assignment order.
        pub fn authorities() -> Vec<PqPublicKey> {
            Authorities::<T>::get().into_inner()
        }

        /// Slot duration in milliseconds.
        pub fn slot_duration() -> u64 {
            T::SlotDuration::get()
        }

        fn install(authorities: Vec<PqPublicKey>) -> Result<(), AuthorityError> {
            validate_authorities(&authorities, T::MaxAuthorities::get())?;
            let bounded = BoundedVec::try_from(authorities).map_err(|_| AuthorityError::TooMany)?;
            Authorities::<T>::put(bounded);
            Ok(())
        }
    }

    impl<T: Config> AuthoritySetWriter for Pallet<T> {
        fn write_authorities(authorities: Vec<PqPublicKey>) -> Result<(), AuthorityError> {
            Self::install(authorities)
        }
    }

    impl<T: Config> BlockAuthorities for Pallet<T> {
        fn authorities() -> Vec<PqPublicKey> {
            Authorities::<T>::get().into_inner()
        }

        fn set_authorities(authorities: Vec<PqPublicKey>) -> Result<(), AuthorityError> {
            Self::install(authorities)
        }
    }

    impl<T: Config> CurrentAuthorTrait for Pallet<T> {
        fn current_author() -> Option<PqPublicKey> {
            CurrentAuthor::<T>::get()
        }

        fn current_slot() -> u64 {
            CurrentSlot::<T>::get().into()
        }
    }

    impl<T: Config> frame_support::traits::OnTimestampSet<u64> for Pallet<T> {
        fn on_timestamp_set(moment: u64) {
            let Some(expected) = moment.checked_div(T::SlotDuration::get()) else {
                return;
            };
            let current = u64::from(CurrentSlot::<T>::get());
            // Blocks without an Aura-PQ digest (slot 0) carry no claim to check.
            if current != 0 && current != expected {
                log::error!(target: LOG_TARGET, "timestamp slot {expected} differs from block slot {current}");
            }
        }
    }
}
