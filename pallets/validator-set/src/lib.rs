//! # Validator set
//!
//! Epochs and the authority set shared by Aura-PQ block authoring and AC-BFT finality (plan
//! §4.1, §4.3; design D6 of `m2-finality`). In M2 the set comes from genesis and only shrinks:
//! validators recorded for double signing are removed at the next epoch boundary. The boundary
//! block then
//! - installs the new list for block authoring (effective from the next block),
//! - increments the set id,
//! - announces the new set in an `acbf` header digest so AC-BFT switches voters after
//!   finalizing this block.
//!
//! Recent sets are kept (with the epochs they were active in) so double-signing evidence can
//! be checked against the set it was signed in. M3 extends this pallet with the PoA → PoS
//! state machine, stake weights and session-key rotation (full plan §11).

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

use alloc::vec::Vec;

use ac_crypto::PqPublicKey;
use ac_primitives::ac_bft::{Authority, SetId};
use ac_primitives::epoch::EpochIndex;
pub use weights::WeightInfo;

const LOG_TARGET: &str = "runtime::validator-set";

/// A historical set: its members and the first epoch it was active in.
#[derive(
    Clone,
    Debug,
    PartialEq,
    Eq,
    parity_scale_codec::Encode,
    parity_scale_codec::Decode,
    parity_scale_codec::DecodeWithMemTracking,
    parity_scale_codec::MaxEncodedLen,
    scale_info::TypeInfo,
)]
#[scale_info(skip_type_params(S))]
pub struct HistoricalSet<S: frame_support::traits::Get<u32>> {
    /// Members in order.
    pub authorities: frame_support::BoundedVec<Authority, S>,
    /// First epoch the set was active in.
    pub since: EpochIndex,
}

// FRAME's `pallet` macro expands to code (genesis defaults, storage metadata) that uses
// `expect` / `unreachable!` on our spans; hand-written code here uses neither.
#[allow(clippy::expect_used, clippy::unreachable)]
#[frame_support::pallet]
pub mod pallet {
    use super::{
        Authority, EpochIndex, HistoricalSet, LOG_TARGET, PqPublicKey, SetId, Vec, WeightInfo,
    };
    use ac_primitives::ac_bft::{ConsensusLog, ScheduledChange, keys};
    use ac_primitives::epoch::{check_epoch_length, epoch_of, is_boundary};
    use ac_primitives::validator_set::{BlockAuthorities, ValidatorSetInterface};
    use frame_support::Twox64Concat;
    use frame_support::pallet_prelude::{
        BoundedVec, BuildGenesisConfig, Get, Hooks, IsType, OptionQuery, StorageMap, StorageValue,
        ValueQuery, Weight,
    };
    use frame_system::pallet_prelude::BlockNumberFor;
    use sp_runtime::SaturatedConversion;

    #[pallet::pallet]
    pub struct Pallet<T>(_);

    #[pallet::config]
    pub trait Config: frame_system::Config {
        /// The overarching event type.
        #[allow(deprecated)] // Kept explicit for compatibility with the SDK's test tooling.
        type RuntimeEvent: From<Event<Self>> + IsType<<Self as frame_system::Config>::RuntimeEvent>;
        /// Block-authoring authority list (Aura-PQ).
        type BlockAuthorities: BlockAuthorities;
        /// Maximum number of authorities.
        #[pallet::constant]
        type MaxAuthorities: Get<u32>;
        /// Number of past epochs whose sets are kept for evidence verification.
        #[pallet::constant]
        type HistoryEpochs: Get<u32>;
        /// Weights.
        type WeightInfo: WeightInfo;
    }

    /// Epoch length in blocks; fixed at genesis.
    #[pallet::storage]
    pub type EpochLength<T: Config> = StorageValue<_, u64, ValueQuery>;

    /// Id of the current authority set (0 at genesis).
    #[pallet::storage]
    pub type CurrentSetId<T: Config> = StorageValue<_, SetId, ValueQuery>;

    /// Current authority set, in order.
    #[pallet::storage]
    pub type Authorities<T: Config> =
        StorageValue<_, BoundedVec<Authority, T::MaxAuthorities>, ValueQuery>;

    /// Keys to remove at the next epoch boundary.
    #[pallet::storage]
    pub type PendingRemovals<T: Config> =
        StorageValue<_, BoundedVec<PqPublicKey, T::MaxAuthorities>, ValueQuery>;

    /// Recent sets by id, with the first epoch each was active in.
    #[pallet::storage]
    pub type SetHistory<T: Config> =
        StorageMap<_, Twox64Concat, SetId, HistoricalSet<T::MaxAuthorities>, OptionQuery>;

    /// Oldest set id still in [`SetHistory`].
    #[pallet::storage]
    pub type OldestSet<T: Config> = StorageValue<_, SetId, ValueQuery>;

    #[pallet::event]
    #[pallet::generate_deposit(pub(super) fn deposit_event)]
    pub enum Event<T: Config> {
        /// A validator was removed from the set at an epoch boundary.
        AuthorityDisabled {
            /// Its key.
            key: PqPublicKey,
        },
        /// A new authority set is in force from the next block.
        NewSet {
            /// Id of the new set.
            set_id: SetId,
            /// Number of members.
            size: u32,
        },
    }

    #[pallet::genesis_config]
    #[derive(frame_support::DefaultNoBound)]
    pub struct GenesisConfig<T: Config> {
        /// Epoch length in blocks; at least twice the number of genesis authorities.
        pub epoch_length: u64,
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
            // Built after the Aura-PQ pallet, whose genesis list is the initial set.
            let keys = T::BlockAuthorities::authorities();
            if keys.is_empty() {
                // SDK tooling builds a default genesis without authorities; the node refuses
                // to run such a chain.
                return;
            }
            let count = u64::try_from(keys.len()).unwrap_or(u64::MAX);
            if let Err(e) = check_epoch_length(self.epoch_length, count) {
                panic!("invalid validator-set genesis: {e}");
            }
            let authorities: Vec<Authority> = keys.into_iter().map(Authority::poa).collect();
            let Ok(bounded) = BoundedVec::try_from(authorities) else {
                panic!("invalid validator-set genesis: too many authorities");
            };
            EpochLength::<T>::put(self.epoch_length);
            Authorities::<T>::put(bounded.clone());
            SetHistory::<T>::insert(
                0,
                HistoricalSet {
                    authorities: bounded,
                    since: 0,
                },
            );
        }
    }

    #[pallet::hooks]
    impl<T: Config> Hooks<BlockNumberFor<T>> for Pallet<T> {
        fn on_initialize(n: BlockNumberFor<T>) -> Weight {
            let number: u64 = n.saturated_into();
            let length = EpochLength::<T>::get();
            // Block 1 opens epoch 0: nothing can be pending yet.
            if number <= 1 || !is_boundary(number, length) {
                return T::DbWeight::get().reads(1);
            }
            let epoch = epoch_of(number, length).unwrap_or(0);
            let size = u32::try_from(Authorities::<T>::get().len()).unwrap_or(u32::MAX);
            if Self::enact(epoch) {
                T::WeightInfo::epoch_boundary_with_change(size)
            } else {
                T::WeightInfo::epoch_boundary_without_change(size)
            }
        }
    }

    impl<T: Config> Pallet<T> {
        /// Current set id and members.
        pub fn authority_set() -> (SetId, Vec<Authority>) {
            (
                CurrentSetId::<T>::get(),
                Authorities::<T>::get().into_inner(),
            )
        }

        /// Epoch length in blocks.
        pub fn epoch_length() -> u64 {
            EpochLength::<T>::get()
        }

        /// Epoch of the block being executed.
        pub fn current_epoch() -> EpochIndex {
            let number: u64 = frame_system::Pallet::<T>::block_number().saturated_into();
            epoch_of(number, EpochLength::<T>::get()).unwrap_or(0)
        }

        /// Members of `set_id` if it was active within the last `HistoryEpochs` epochs.
        pub fn historical_set(set_id: SetId) -> Option<Vec<Authority>> {
            let set = SetHistory::<T>::get(set_id)?;
            Self::within_window(set_id).then(|| set.authorities.into_inner())
        }

        /// Whether set `set_id` was still active at the start of the retention window.
        fn within_window(set_id: SetId) -> bool {
            if set_id == CurrentSetId::<T>::get() {
                return true;
            }
            // A past set was active until its successor's first epoch.
            let Some(next) = set_id.checked_add(1).and_then(SetHistory::<T>::get) else {
                return false;
            };
            let window_start =
                Self::current_epoch().saturating_sub(u64::from(T::HistoryEpochs::get()));
            next.since > window_start
        }

        /// Applies pending removals at the boundary opening `epoch`; returns whether the set
        /// changed. Never panics; an unexpected failure leaves the set as it was.
        pub(crate) fn enact(epoch: EpochIndex) -> bool {
            let removals = PendingRemovals::<T>::take();
            Self::prune_history(epoch);
            if removals.is_empty() {
                return false;
            }
            let current = Authorities::<T>::get();
            let mut next: Vec<Authority> = current
                .iter()
                .filter(|a| !removals.contains(&a.key))
                .cloned()
                .collect();
            if next.is_empty() {
                // Never empty the set: keep the first offender in set order.
                if let Some(first) = current.first() {
                    next.push(first.clone());
                }
            }
            if next.len() == current.len() {
                return false;
            }
            let Ok(bounded) = BoundedVec::<Authority, T::MaxAuthorities>::try_from(next) else {
                return false;
            };
            if let Err(e) = T::BlockAuthorities::set_authorities(keys(&bounded)) {
                log::error!(target: LOG_TARGET, "cannot install the new authority set: {e}");
                return false;
            }
            let set_id = CurrentSetId::<T>::get().saturating_add(1);
            CurrentSetId::<T>::put(set_id);
            Authorities::<T>::put(bounded.clone());
            SetHistory::<T>::insert(
                set_id,
                HistoricalSet {
                    authorities: bounded.clone(),
                    since: epoch,
                },
            );
            let change = ConsensusLog::ScheduledChange(ScheduledChange {
                set_id,
                authorities: BoundedVec::truncate_from(bounded.to_vec()),
            });
            frame_system::Pallet::<T>::deposit_log(change.to_digest());
            for removed in current.iter().filter(|a| !bounded.contains(a)) {
                Self::deposit_event(Event::AuthorityDisabled {
                    key: removed.key.clone(),
                });
            }
            Self::deposit_event(Event::NewSet {
                set_id,
                size: u32::try_from(bounded.len()).unwrap_or(u32::MAX),
            });
            true
        }

        /// Drops sets that ended before the retention window. At most one set changes per
        /// epoch, so this removes a bounded number of entries.
        fn prune_history(epoch: EpochIndex) {
            let window_start = epoch.saturating_sub(u64::from(T::HistoryEpochs::get()));
            let current = CurrentSetId::<T>::get();
            let mut oldest = OldestSet::<T>::get();
            while oldest < current {
                let successor_since = oldest
                    .checked_add(1)
                    .and_then(SetHistory::<T>::get)
                    .map_or(u64::MAX, |s| s.since);
                if successor_since > window_start {
                    break;
                }
                SetHistory::<T>::remove(oldest);
                oldest = oldest.saturating_add(1);
            }
            OldestSet::<T>::put(oldest);
        }
    }

    impl<T: Config> ValidatorSetInterface for Pallet<T> {
        fn current() -> (SetId, Vec<Authority>) {
            Self::authority_set()
        }

        fn historical(set_id: SetId) -> Option<Vec<Authority>> {
            Self::historical_set(set_id)
        }

        fn is_recent_member(key: &PqPublicKey) -> bool {
            let current = CurrentSetId::<T>::get();
            let mut id = OldestSet::<T>::get();
            loop {
                if Self::historical_set(id).is_some_and(|set| set.iter().any(|a| &a.key == key)) {
                    return true;
                }
                if id >= current {
                    return false;
                }
                id = id.saturating_add(1);
            }
        }

        fn disable(key: &PqPublicKey) -> bool {
            if !Authorities::<T>::get().iter().any(|a| &a.key == key) {
                return false;
            }
            PendingRemovals::<T>::mutate(|pending| {
                if !pending.contains(key) {
                    // Bounded by the set size: only current members are added.
                    let _ = pending.try_push(key.clone());
                }
            });
            true
        }

        fn current_epoch() -> EpochIndex {
            Self::current_epoch()
        }
    }
}
