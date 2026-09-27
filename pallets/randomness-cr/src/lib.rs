//! # Commit–reveal randomness
//!
//! Epoch randomness from validators (plan §4.2; design D8 of `m2-finality`). In every epoch `e`
//! each validator commits, in a block it authors, to a secret derived from its key; in epoch
//! `e + 1` it reveals the secret; at the boundary of epoch `e + 2` the chain publishes
//! `R(e)` computed by [`ac_primitives::randomness::epoch_randomness`] from all valid reveals.
//!
//! Commitments and reveals travel in the inherent `note_randomness`, which the author's node
//! fills from its local secrets. The author comes from Aura-PQ's record of the block's slot, so
//! nobody can commit or reveal for someone else. Invalid data (a second commitment in one epoch,
//! a reveal that does not match its commitment) is ignored and changes nothing; the call never
//! fails. Validators that committed but did not reveal are counted in [`MissedReveals`], and
//! each conclusion's misses are offered to staking through
//! [`RevealTracker`](ac_primitives::validator_set::RevealTracker): in PoS a miss costs the
//! epoch's work points and three in a row pause the validator.
//!
//! **Known bias**: the last validator to reveal may withhold its reveal to choose between two
//! outcomes, and colluding validators amplify this. The randomness suits low-value purposes
//! such as audit sampling only; full plan §2.4 removes the bias with a hash-based VDF.

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

use ac_primitives::epoch::EpochIndex;
pub use weights::WeightInfo;

/// A validator's 32-byte account ID, derived from its public key.
pub type AccountBytes = [u8; 32];

// FRAME's `pallet` macro expands to code (genesis defaults, storage metadata) that uses
// `expect` / `unreachable!` on our spans; hand-written code here uses neither.
#[allow(clippy::expect_used, clippy::unreachable)]
#[frame_support::pallet]
pub mod pallet {
    use super::{AccountBytes, EpochIndex, WeightInfo};
    use ac_primitives::epoch::{epoch_of, is_boundary};
    use ac_primitives::randomness::{
        INHERENT_IDENTIFIER, InherentSecrets, epoch_randomness, subject_value,
    };
    use ac_primitives::validator_set::{CurrentAuthor, ValidatorSetInterface};
    use alloc::vec::Vec;
    use frame_support::pallet_prelude::ProvideInherent;
    use frame_support::pallet_prelude::{
        BoundedVec, DispatchClass, DispatchResult, Get, Hooks, IsType, OptionQuery,
        StorageDoubleMap, StorageMap, StorageValue, ValueQuery, Weight,
    };
    use frame_support::{Blake2_128Concat, Twox64Concat};
    use frame_system::pallet_prelude::{BlockNumberFor, OriginFor};
    use sp_core::H256;
    use sp_inherents::{InherentData, InherentIdentifier, MakeFatalError};
    use sp_runtime::SaturatedConversion;

    #[pallet::pallet]
    pub struct Pallet<T>(_);

    #[pallet::config]
    pub trait Config: frame_system::Config {
        /// The overarching event type.
        #[allow(deprecated)] // Kept explicit for compatibility with the SDK's test tooling.
        type RuntimeEvent: From<Event<Self>> + IsType<<Self as frame_system::Config>::RuntimeEvent>;
        /// Author of the block being executed (Aura-PQ).
        type Author: CurrentAuthor;
        /// Epochs (validator set).
        type Epochs: ValidatorSetInterface;
        /// Maximum number of authorities (bounds per-epoch work).
        #[pallet::constant]
        type MaxAuthorities: Get<u32>;
        /// Number of past epochs whose randomness stays queryable.
        #[pallet::constant]
        type KeepEpochs: Get<u32>;
        /// Weights.
        type WeightInfo: WeightInfo;
    }

    /// Commitments per epoch and validator.
    #[pallet::storage]
    pub type Commits<T: Config> = StorageDoubleMap<
        _,
        Twox64Concat,
        EpochIndex,
        Blake2_128Concat,
        AccountBytes,
        [u8; 32],
        OptionQuery,
    >;

    /// Reveals per epoch and validator.
    #[pallet::storage]
    pub type Reveals<T: Config> = StorageDoubleMap<
        _,
        Twox64Concat,
        EpochIndex,
        Blake2_128Concat,
        AccountBytes,
        [u8; 32],
        OptionQuery,
    >;

    /// Published randomness per epoch, with the block that published it.
    #[pallet::storage]
    pub type Randomness<T: Config> =
        StorageMap<_, Twox64Concat, EpochIndex, (H256, BlockNumberFor<T>), OptionQuery>;

    /// Latest published randomness.
    #[pallet::storage]
    pub type Latest<T: Config> = StorageValue<_, (EpochIndex, H256), OptionQuery>;

    /// Commitments left unrevealed, per validator.
    #[pallet::storage]
    pub type MissedReveals<T: Config> =
        StorageMap<_, Blake2_128Concat, AccountBytes, u32, ValueQuery>;

    /// Whether this block already carried `note_randomness`.
    #[pallet::storage]
    pub type Noted<T: Config> = StorageValue<_, bool, ValueQuery>;

    /// Validators that missed their reveal in the latest conclusion, with the block that
    /// concluded (read by staking to zero their work points, design D8 of `m3-pos`).
    #[pallet::storage]
    pub type LastMissed<T: Config> =
        StorageValue<_, (u64, BoundedVec<AccountBytes, T::MaxAuthorities>), OptionQuery>;

    #[pallet::event]
    #[pallet::generate_deposit(pub(super) fn deposit_event)]
    pub enum Event<T: Config> {
        /// Randomness of `epoch` was published.
        RandomnessPublished {
            /// Epoch the reveals belong to.
            epoch: EpochIndex,
            /// The value.
            value: H256,
        },
    }

    #[pallet::hooks]
    impl<T: Config> Hooks<BlockNumberFor<T>> for Pallet<T> {
        fn on_initialize(n: BlockNumberFor<T>) -> Weight {
            Noted::<T>::kill();
            let number: u64 = n.saturated_into();
            let length = T::Epochs::epoch_length();
            let base = T::DbWeight::get().reads_writes(1, 1);
            if !is_boundary(number, length) {
                return base;
            }
            let Some(epoch) = epoch_of(number, length).and_then(|e| e.checked_sub(2)) else {
                return base;
            };
            let reveals = Self::conclude(epoch, n);
            base.saturating_add(T::WeightInfo::conclude_epoch(reveals))
        }
    }

    #[pallet::call]
    impl<T: Config> Pallet<T> {
        /// The block author's commitment for this epoch and reveal for the previous one.
        /// Inherent: included by the author, never signed. Invalid parts are ignored.
        #[pallet::call_index(0)]
        #[pallet::weight((T::WeightInfo::note_randomness(), DispatchClass::Mandatory))]
        pub fn note_randomness(
            origin: OriginFor<T>,
            commit: Option<[u8; 32]>,
            reveal: Option<[u8; 32]>,
        ) -> DispatchResult {
            frame_system::ensure_none(origin)?;
            if Noted::<T>::get() {
                return Ok(());
            }
            Noted::<T>::put(true);
            let Some(author) = T::Author::current_author() else {
                return Ok(());
            };
            let who: AccountBytes = *ac_crypto::account_id(&author).as_bytes();
            Self::note(who, T::Epochs::current_epoch(), commit, reveal);
            Ok(())
        }
    }

    #[pallet::inherent]
    impl<T: Config> ProvideInherent for Pallet<T> {
        type Call = Call<T>;
        type Error = MakeFatalError<()>;
        const INHERENT_IDENTIFIER: InherentIdentifier = INHERENT_IDENTIFIER;

        fn create_inherent(data: &InherentData) -> Option<Self::Call> {
            let secrets: InherentSecrets = data.get_data(&INHERENT_IDENTIFIER).ok()??;
            let epoch = T::Epochs::current_epoch();
            if secrets.epoch != epoch {
                return None;
            }
            let author = T::Author::current_author()?;
            let who: AccountBytes = *ac_crypto::account_id(&author).as_bytes();
            let commit = (!Commits::<T>::contains_key(epoch, who))
                .then(|| ac_crypto::randomness_commit(&secrets.current).ok())
                .flatten();
            let reveal = epoch.checked_sub(1).and_then(|previous| {
                let due = Commits::<T>::contains_key(previous, who)
                    && !Reveals::<T>::contains_key(previous, who);
                due.then_some(secrets.previous).flatten()
            });
            (commit.is_some() || reveal.is_some())
                .then_some(Call::note_randomness { commit, reveal })
        }

        fn is_inherent(call: &Self::Call) -> bool {
            matches!(call, Call::note_randomness { .. })
        }
    }

    impl<T: Config> Pallet<T> {
        /// Records `who`'s commitment for `epoch` and reveal for `epoch − 1`, ignoring anything
        /// invalid.
        pub(crate) fn note(
            who: AccountBytes,
            epoch: EpochIndex,
            commit: Option<[u8; 32]>,
            reveal: Option<[u8; 32]>,
        ) {
            if let Some(commit) = commit
                && !Commits::<T>::contains_key(epoch, who)
            {
                Commits::<T>::insert(epoch, who, commit);
            }
            if let (Some(secret), Some(previous)) = (reveal, epoch.checked_sub(1))
                && !Reveals::<T>::contains_key(previous, who)
                && Commits::<T>::get(previous, who)
                    .is_some_and(|c| ac_crypto::randomness_commit(&secret).is_ok_and(|h| h == c))
            {
                Reveals::<T>::insert(previous, who, secret);
            }
        }

        /// Publishes `R(epoch)`, counts missed reveals and clears the epoch. Returns the number of
        /// reveals.
        pub(crate) fn conclude(epoch: EpochIndex, now: BlockNumberFor<T>) -> u32 {
            let max = T::MaxAuthorities::get();
            let reveals: Vec<(AccountBytes, [u8; 32])> = Reveals::<T>::iter_prefix(epoch).collect();
            let mut missed = Vec::new();
            for (who, _) in Commits::<T>::iter_prefix(epoch) {
                if !reveals.iter().any(|(r, _)| *r == who) {
                    MissedReveals::<T>::mutate(who, |n| *n = n.saturating_add(1));
                    missed.push(who);
                }
            }
            // At most one commitment per authority, so the list fits the bound.
            LastMissed::<T>::put((
                now.saturated_into::<u64>(),
                BoundedVec::truncate_from(missed),
            ));
            if let Ok(Some(value)) = epoch_randomness(epoch, &reveals) {
                Randomness::<T>::insert(epoch, (value, now));
                Latest::<T>::put((epoch, value));
                Self::deposit_event(Event::RandomnessPublished { epoch, value });
            }
            // Bounded: at most one commitment and one reveal per authority.
            let _ = Commits::<T>::clear_prefix(epoch, max, None);
            let _ = Reveals::<T>::clear_prefix(epoch, max, None);
            if let Some(old) = epoch.checked_sub(u64::from(T::KeepEpochs::get())) {
                Randomness::<T>::remove(old);
            }
            u32::try_from(reveals.len()).unwrap_or(u32::MAX)
        }

        /// Latest randomness and its epoch.
        pub fn latest() -> Option<(EpochIndex, H256)> {
            Latest::<T>::get()
        }

        /// Value for `subject` from the latest randomness, with its epoch.
        pub fn random(subject: &[u8]) -> Option<(EpochIndex, H256)> {
            let (epoch, r) = Latest::<T>::get()?;
            subject_value(&r, subject).ok().map(|v| (epoch, v))
        }

        /// Randomness of `epoch`, if kept.
        pub fn epoch_randomness(epoch: EpochIndex) -> Option<H256> {
            Randomness::<T>::get(epoch).map(|(v, _)| v)
        }

        /// Reveals of `epoch` while it is open.
        pub fn reveals(epoch: EpochIndex) -> Vec<(AccountBytes, [u8; 32])> {
            Reveals::<T>::iter_prefix(epoch).collect()
        }
    }

    /// FRAME randomness for other pallets (audit sampling in M6). Before the first published
    /// value it returns the zero hash and block 0; callers that need a real value use
    /// [`Pallet::random`], which returns `None` then.
    impl<T: Config> ac_primitives::validator_set::RevealTracker for Pallet<T> {
        fn missed_now() -> Vec<[u8; 32]> {
            let now: u64 = frame_system::Pallet::<T>::block_number().saturated_into();
            match LastMissed::<T>::get() {
                Some((at, missed)) if at == now => missed.into_inner(),
                _ => Vec::new(),
            }
        }
    }

    impl<T: Config> frame_support::traits::Randomness<H256, BlockNumberFor<T>> for Pallet<T> {
        fn random(subject: &[u8]) -> (H256, BlockNumberFor<T>) {
            let Some((epoch, r)) = Latest::<T>::get() else {
                return (H256::zero(), BlockNumberFor::<T>::default());
            };
            let known_at = Randomness::<T>::get(epoch)
                .map(|(_, b)| b)
                .unwrap_or_default();
            (subject_value(&r, subject).unwrap_or_default(), known_at)
        }
    }
}
