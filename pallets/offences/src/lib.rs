//! # Offences
//!
//! Double-signing evidence on chain (plan §4.1; design D7 of `m2-finality`, decision D39
//! pending). Anyone may submit evidence without a signature or fee: the call authorizes itself
//! when the evidence is valid and new (checked before it enters the transaction pool, through
//! the `AuthorizeCall` transaction extension). A recorded offender is
//! - removed from block authoring and AC-BFT voting at the next epoch boundary (through the
//!   validator set), and
//! - passed to the [`SlashHandler`](ac_primitives::validator_set::SlashHandler). The runtime's
//!   handler (`pallet-staking-pos`, `m3-pos`) slashes and burns the offender's self-stake; a PoA
//!   authority without stake loses nothing.
//!
//! Each offence is recorded once, and each offender at most once per offence kind (block seal,
//! AC-BFT vote) and authority set, so the records of a set never exceed twice its size. Only an
//! offender's first record in a set disables it; a record of the other kind is kept and reported
//! as an event but punishes nothing more, and the slash handler is told the kinds already
//! recorded so that slashing follows the most severe one instead of adding up. Records are
//! dropped once their set leaves the validator set's history window.

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
use ac_primitives::offences::{AuthoritySets, OffenceKind};
use ac_primitives::validator_set::ValidatorSetInterface;
pub use weights::WeightInfo;

/// Authority sets as seen through the validator set, for evidence verification.
struct RecentSets<V>(core::marker::PhantomData<V>);

impl<V: ValidatorSetInterface> AuthoritySets for RecentSets<V> {
    fn set(&self, set_id: SetId) -> Option<Vec<Authority>> {
        V::historical(set_id)
    }

    fn contains(&self, key: &PqPublicKey) -> bool {
        V::is_recent_member(key)
    }
}

/// Twice `N`: an offender may hold one record per offence kind in a set.
pub struct TwicePerSet<N>(core::marker::PhantomData<N>);

impl<N: frame_support::traits::Get<u32>> frame_support::traits::Get<u32> for TwicePerSet<N> {
    fn get() -> u32 {
        N::get().saturating_mul(2)
    }
}

/// Number of offence kinds; one record per kind and offender is kept per set.
const OFFENCE_KINDS: usize = 2;

// FRAME's `pallet` macro expands to code (genesis defaults, storage metadata) that uses
// `expect` / `unreachable!` on our spans; hand-written code here uses neither.
#[allow(clippy::expect_used, clippy::unreachable)]
#[frame_support::pallet]
pub mod pallet {
    use super::{
        OFFENCE_KINDS, OffenceKind, PqPublicKey, RecentSets, SetId, TwicePerSet, Vec, WeightInfo,
    };
    use ac_primitives::offences::{Evidence, Offence, OffenceKey, verify_evidence};
    use ac_primitives::validator_set::{CurrentAuthor, SlashHandler, ValidatorSetInterface};
    use frame_support::pallet_prelude::{
        BoundedVec, DispatchResult, Get, Hooks, InvalidTransaction, IsType, OptionQuery,
        StorageMap, StorageValue, TransactionSource, TransactionValidityError, ValidTransaction,
        ValueQuery, Weight,
    };
    use frame_support::{Blake2_128Concat, Twox64Concat};
    use frame_system::pallet_prelude::{BlockNumberFor, OriginFor};
    use sp_core::H256;
    use sp_runtime::traits::Zero;
    use sp_runtime::transaction_validity::TransactionValidityWithRefund;

    #[pallet::pallet]
    pub struct Pallet<T>(_);

    #[pallet::config]
    pub trait Config: frame_system::Config<Hash = H256> {
        /// The overarching event type.
        #[allow(deprecated)] // Kept explicit for compatibility with the SDK's test tooling.
        type RuntimeEvent: From<Event<Self>> + IsType<<Self as frame_system::Config>::RuntimeEvent>;
        /// Validator set: membership, history and disabling.
        type ValidatorSet: ValidatorSetInterface;
        /// Current slot, to reject seal evidence older than [`Config::MaxEvidenceAge`].
        type Slots: CurrentAuthor;
        /// Slashing of offenders (`()` in M2: nothing to slash).
        type SlashHandler: SlashHandler;
        /// Maximum number of authorities (bounds records per set).
        #[pallet::constant]
        type MaxAuthorities: Get<u32>;
        /// Oldest acceptable seal-evidence slot, counted back from the current slot.
        #[pallet::constant]
        type MaxEvidenceAge: Get<u64>;
        /// Weights.
        type WeightInfo: WeightInfo;
    }

    /// Recorded offences by key, with the set they were recorded under.
    #[pallet::storage]
    pub type Reports<T: Config> = StorageMap<_, Blake2_128Concat, OffenceKey, SetId, OptionQuery>;

    /// Offenders recorded per set (at most one record per offender and offence kind), with the
    /// offence. The encoding is that of the former one-per-offender bound, so no migration is
    /// needed.
    #[pallet::storage]
    pub type Offenders<T: Config> = StorageMap<
        _,
        Twox64Concat,
        SetId,
        BoundedVec<(PqPublicKey, OffenceKey), TwicePerSet<T::MaxAuthorities>>,
        ValueQuery,
    >;

    /// Oldest set that may still have records.
    #[pallet::storage]
    pub type OldestRecordedSet<T: Config> = StorageValue<_, SetId, ValueQuery>;

    #[pallet::event]
    #[pallet::generate_deposit(pub(super) fn deposit_event)]
    pub enum Event<T: Config> {
        /// Double signing was proven. On the offender's first record in the set it leaves the
        /// set at the next epoch boundary; a record of the other kind changes nothing more.
        OffenceReported {
            /// Offender's key.
            offender: PqPublicKey,
            /// The offence.
            key: OffenceKey,
            /// Set the offence is recorded under.
            set_id: SetId,
            /// Amount slashed (always 0 in M2).
            slashed: u128,
        },
    }

    #[pallet::hooks]
    impl<T: Config> Hooks<BlockNumberFor<T>> for Pallet<T> {
        fn on_initialize(_n: BlockNumberFor<T>) -> Weight {
            // Drop the records of at most one expired set per block: bounded work.
            let oldest = OldestRecordedSet::<T>::get();
            let (current, _) = T::ValidatorSet::current();
            if oldest >= current || T::ValidatorSet::historical(oldest).is_some() {
                return T::DbWeight::get().reads(3);
            }
            let records = Offenders::<T>::take(oldest);
            for (_, key) in &records {
                Reports::<T>::remove(key);
            }
            OldestRecordedSet::<T>::put(oldest.saturating_add(1));
            let n = u64::try_from(records.len()).unwrap_or(u64::MAX);
            T::DbWeight::get().reads_writes(3, n.saturating_add(2))
        }
    }

    #[pallet::call]
    impl<T: Config> Pallet<T> {
        /// Reports double signing. Needs no signature and pays no fee: the call is authorized
        /// when `evidence` proves an offence that is not recorded yet.
        #[pallet::call_index(0)]
        #[pallet::weight(T::WeightInfo::report_equivocation())]
        #[pallet::authorize(Pallet::<T>::authorize_report)]
        #[pallet::weight_of_authorize(T::WeightInfo::authorize_report_equivocation())]
        pub fn report_equivocation(origin: OriginFor<T>, evidence: Evidence) -> DispatchResult {
            frame_system::ensure_authorized(origin)?;
            // Verified again against this block's state before recording (the benchmarked
            // weight includes this second verification); invalid or stale evidence changes
            // nothing.
            if let Ok(offence) = Self::check(&evidence) {
                Self::record(offence);
            }
            Ok(())
        }
    }

    impl<T: Config> Pallet<T> {
        /// Authorization of [`Pallet::report_equivocation`]: the evidence must be valid and new.
        ///
        /// # Errors
        ///
        /// `InvalidTransaction::BadProof` for invalid evidence, `InvalidTransaction::Stale` for
        /// an offence already recorded or an offender already recorded for this kind in the
        /// set.
        pub fn authorize_report(
            _source: TransactionSource,
            evidence: &Evidence,
        ) -> TransactionValidityWithRefund {
            let offence = Self::check(evidence)?;
            let set_id = Self::record_set(&offence);
            let validity = ValidTransaction::with_tag_prefix("AcOffences")
                .priority(u64::MAX >> 1)
                // Reports of one kind against one offender in one set exclude each other in the
                // pool; the other kind is independent.
                .and_provides((set_id, offence.offender, offence.key.kind()))
                .longevity(64)
                .propagate(true)
                .build()?;
            Ok((validity, Weight::zero()))
        }

        /// Verifies evidence against the recent sets and checks it is new.
        fn check(evidence: &Evidence) -> Result<Offence, TransactionValidityError> {
            let genesis = frame_system::Pallet::<T>::block_hash(BlockNumberFor::<T>::zero());
            let offence = verify_evidence(
                &genesis,
                evidence,
                &RecentSets::<T::ValidatorSet>(Default::default()),
            )
            .map_err(|_| TransactionValidityError::Invalid(InvalidTransaction::BadProof))?;
            if let OffenceKey::Aura { slot, .. } = offence.key
                && slot.saturating_add(T::MaxEvidenceAge::get()) < T::Slots::current_slot()
            {
                return Err(TransactionValidityError::Invalid(InvalidTransaction::Stale));
            }
            let set_id = Self::record_set(&offence);
            let kind = offence.key.kind();
            if Reports::<T>::contains_key(&offence.key)
                || Self::prior_kinds(set_id, &offence.offender).contains(&kind)
            {
                return Err(TransactionValidityError::Invalid(InvalidTransaction::Stale));
            }
            Ok(offence)
        }

        /// The set an offence is recorded under: its own for AC-BFT, the current one for seals.
        fn record_set(offence: &Offence) -> SetId {
            offence
                .set_id
                .unwrap_or_else(|| T::ValidatorSet::current().0)
        }

        /// Kinds of offence already recorded for `offender` in `set_id`.
        fn prior_kinds(set_id: SetId, offender: &PqPublicKey) -> Vec<OffenceKind> {
            let mut kinds = Vec::with_capacity(OFFENCE_KINDS);
            for (who, key) in Offenders::<T>::get(set_id).iter() {
                if who == offender && !kinds.contains(&key.kind()) {
                    kinds.push(key.kind());
                }
            }
            kinds
        }

        /// Records a checked offence. The offender's first record in the set disables it; every
        /// record is passed to the slash handler with the kinds recorded before it.
        pub(crate) fn record(offence: Offence) {
            let set_id = Self::record_set(&offence);
            let prior = Self::prior_kinds(set_id, &offence.offender);
            let pushed = Offenders::<T>::mutate(set_id, |records| {
                records
                    .try_push((offence.offender.clone(), offence.key.clone()))
                    .is_ok()
            });
            if !pushed {
                return;
            }
            Reports::<T>::insert(&offence.key, set_id);
            if prior.is_empty() {
                T::ValidatorSet::disable(&offence.offender);
            }
            let slashed =
                T::SlashHandler::on_offence(&offence.offender, offence.key.kind(), &prior);
            Self::deposit_event(Event::OffenceReported {
                offender: offence.offender,
                key: offence.key,
                set_id,
                slashed,
            });
        }

        /// Offences recorded under `set_id`.
        pub fn offences(set_id: SetId) -> Vec<(PqPublicKey, OffenceKey)> {
            Offenders::<T>::get(set_id).into_inner()
        }
    }
}
