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
//! be checked against the set it was signed in.
//!
//! **PoA → PoS** (design D6, D7 of `m3-pos`; decisions D19, D24): during PoA the set is the
//! [`PoaAuthorities`] roster, which the administration grows or shrinks (effective at the next
//! boundary). Every PoA boundary is a checkpoint of the switch conditions
//! ([`ac_primitives::staking::transition_step`]); once they have held for the sustain period
//! the boundary switches to PoS for good: the roster is emptied and the set becomes the elected
//! validators, weighted by backing. From then on each boundary installs the election run in the
//! previous block. [`Phase`], [`QualifiedSince`], [`PoaAuthorities`] and [`TransitionParams`]
//! are **published well-known storage keys** read by the node, which recomputes every
//! checkpoint: never rename or re-encode them.

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
pub use ac_primitives::staking::TransitionParams as SwitchParams;
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
        Authority, EpochIndex, HistoricalSet, LOG_TARGET, PqPublicKey, SetId, SwitchParams, Vec,
        WeightInfo,
    };
    use ac_crypto::SigAlg;
    use ac_primitives::ac_bft::{ConsensusLog, ScheduledChange, keys};
    use ac_primitives::epoch::{check_epoch_length, epoch_of, is_boundary};
    use ac_primitives::staking::{
        ChainPhase, TransitionInputs, TransitionProgress, backing_to_weight, transition_step,
    };
    use ac_primitives::validator_set::{BlockAuthorities, StakingInterface, ValidatorSetInterface};
    use frame_support::Twox64Concat;
    use frame_support::pallet_prelude::{
        BoundedVec, BuildGenesisConfig, DispatchResult, EnsureOrigin, Get, Hooks, IsType,
        OptionQuery, StorageMap, StorageValue, ValueQuery, Weight, ensure,
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
        /// Block-authoring authority list (Aura-PQ).
        type BlockAuthorities: BlockAuthorities;
        /// The staking ledger: switch inputs and elections.
        type Staking: StakingInterface;
        /// The PoA administration (adds and removes PoA authorities, sets `K`).
        type AdminOrigin: EnsureOrigin<Self::RuntimeOrigin>;
        /// Maximum number of authorities (also the largest allowed `K`).
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

    /// Validator phase. **Well-known key** (`twox128("ValidatorSet") ‖ twox128("Phase")`, SCALE
    /// [`ChainPhase`]), written at genesis; only ever changes from PoA to PoS.
    #[pallet::storage]
    pub type Phase<T: Config> = StorageValue<_, ChainPhase, ValueQuery>;

    /// Start of the current run of passing checkpoints. **Well-known key** (SCALE
    /// `Option<u64>`, always present from genesis on).
    #[pallet::storage]
    pub type QualifiedSince<T: Config> = StorageValue<_, Option<u64>, ValueQuery>;

    /// The PoA roster: the set of the next PoA epoch. **Well-known key** (SCALE vector of
    /// keys); empty for good once in PoS.
    #[pallet::storage]
    pub type PoaAuthorities<T: Config> =
        StorageValue<_, BoundedVec<PqPublicKey, T::MaxAuthorities>, ValueQuery>;

    /// Switch parameters, set at genesis. **Well-known key** (SCALE [`SwitchParams`]); the node
    /// checks live chains against the constitution values.
    #[pallet::storage]
    pub type TransitionParams<T: Config> = StorageValue<_, SwitchParams, ValueQuery>;

    /// Active-set size `K` of PoS elections.
    #[pallet::storage]
    pub type ValidatorCount<T: Config> = StorageValue<_, u32, ValueQuery>;

    /// Block of the switch to PoS.
    #[pallet::storage]
    pub type SwitchedAt<T: Config> = StorageValue<_, u64, OptionQuery>;

    /// PoS: the set elected in the last block of the previous epoch, for the boundary opening
    /// the given epoch.
    #[pallet::storage]
    pub type NextElected<T: Config> =
        StorageValue<_, (EpochIndex, BoundedVec<Authority, T::MaxAuthorities>), OptionQuery>;

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
        /// The administration added a PoA authority (effective at the next boundary).
        PoaAuthorityAdded {
            /// Its key.
            key: PqPublicKey,
        },
        /// The administration removed a PoA authority (effective at the next boundary).
        PoaAuthorityRemoved {
            /// Its key.
            key: PqPublicKey,
        },
        /// A checkpoint of the switch conditions.
        TransitionCheckpoint {
            /// Whether all conditions held.
            qualified: bool,
            /// Start of the current qualified run.
            since: Option<u64>,
        },
        /// The chain switched to PoS at this block; there is no way back.
        SwitchedToPos {
            /// Block of the switch.
            block: u64,
        },
        /// The active-set size changed.
        ValidatorCountSet {
            /// New `K`.
            count: u32,
        },
    }

    #[pallet::error]
    pub enum Error<T> {
        /// The chain is in PoS: the PoA roster can no longer change.
        NotPoa,
        /// PoA authorities must have ML-DSA-65 keys.
        NotMlDsa65,
        /// The key is already in the roster.
        AlreadyAuthority,
        /// The key is not in the roster.
        NotAuthority,
        /// The roster must never become empty.
        WouldEmpty,
        /// The set would exceed the maximum or half the epoch length.
        TooMany,
        /// `K` must be at least the switch's candidate count, at most the maximum number of
        /// authorities, and at most half the epoch length.
        BadValidatorCount,
    }

    #[pallet::genesis_config]
    pub struct GenesisConfig<T: Config> {
        /// Epoch length in blocks; at least twice the larger of `K` and the genesis authorities.
        pub epoch_length: u64,
        /// Switch parameters (live chains: the constitution values).
        pub transition: SwitchParams,
        /// Active-set size `K` of PoS elections.
        pub validator_count: u32,
        #[serde(skip)]
        /// Marker.
        pub _config: core::marker::PhantomData<T>,
    }

    impl<T: Config> Default for GenesisConfig<T> {
        fn default() -> Self {
            Self {
                epoch_length: 0,
                transition: SwitchParams::CONSTITUTION,
                validator_count: 100,
                _config: core::marker::PhantomData,
            }
        }
    }

    impl<T: Config> GenesisConfig<T> {
        /// Genesis of a chain that stays in PoA unless someone stakes: `K` = 1 and the
        /// constitution's switch values except one qualified candidate (tests of other pallets).
        pub fn poa(epoch_length: u64) -> Self {
            Self {
                epoch_length,
                transition: SwitchParams {
                    min_candidates: 1,
                    ..SwitchParams::CONSTITUTION
                },
                validator_count: 1,
                _config: core::marker::PhantomData,
            }
        }
    }

    #[pallet::genesis_build]
    impl<T: Config> BuildGenesisConfig for GenesisConfig<T> {
        // Genesis building has no error channel: aborting is how a chain-spec build fails, and
        // it never runs during block execution.
        #[allow(clippy::panic)]
        fn build(&self) {
            // Built after the Aura-PQ pallet, whose genesis list is the initial set.
            let keys = T::BlockAuthorities::authorities();
            // The well-known keys exist on every chain, so the node can always read them.
            Phase::<T>::put(ChainPhase::Poa);
            QualifiedSince::<T>::put(None::<u64>);
            TransitionParams::<T>::put(self.transition);
            EpochLength::<T>::put(self.epoch_length);
            if keys.is_empty() {
                // SDK tooling builds a default genesis without authorities; the node refuses
                // to run such a chain.
                return;
            }
            let count = u64::try_from(keys.len()).unwrap_or(u64::MAX);
            let k = self.validator_count;
            if let Err(e) = check_epoch_length(self.epoch_length, count.max(u64::from(k))) {
                panic!("invalid validator-set genesis: {e}");
            }
            if self.transition.min_candidates == 0
                || k < self.transition.min_candidates
                || k > T::MaxAuthorities::get()
            {
                panic!("invalid validator-set genesis: validator count {k} or switch parameters");
            }
            let Ok(roster) = BoundedVec::<PqPublicKey, T::MaxAuthorities>::try_from(keys.clone())
            else {
                panic!("invalid validator-set genesis: too many authorities");
            };
            let authorities: Vec<Authority> = keys.into_iter().map(Authority::poa).collect();
            let Ok(bounded) = BoundedVec::try_from(authorities) else {
                panic!("invalid validator-set genesis: too many authorities");
            };
            EpochLength::<T>::put(self.epoch_length);
            ValidatorCount::<T>::put(k);
            PoaAuthorities::<T>::put(roster);
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
            let mut weight = T::DbWeight::get().reads(2);
            // Register the weight of the election that `on_finalize` runs in the last block
            // of an epoch.
            if Self::elects_after(number) {
                weight =
                    weight.saturating_add(T::Staking::election_weight(ValidatorCount::<T>::get()));
            }
            if !is_boundary(number, length) {
                return weight;
            }
            let epoch = epoch_of(number, length).unwrap_or(0);
            let size = u32::try_from(Authorities::<T>::get().len()).unwrap_or(u32::MAX);
            let (changed, extra) = Self::enact(epoch, number);
            let boundary = if changed {
                T::WeightInfo::epoch_boundary_with_change(size)
            } else {
                T::WeightInfo::epoch_boundary_without_change(size)
            };
            weight.saturating_add(boundary).saturating_add(extra)
        }

        fn on_finalize(n: BlockNumberFor<T>) {
            let number: u64 = n.saturated_into();
            if !Self::elects_after(number) {
                return;
            }
            let next_epoch =
                epoch_of(number.saturating_add(1), EpochLength::<T>::get()).unwrap_or(0);
            let seats = ValidatorCount::<T>::get();
            match Phase::<T>::get() {
                ChainPhase::Pos => {
                    let set = Self::weighted(T::Staking::elect(seats, false));
                    NextElected::<T>::put((next_epoch, set));
                }
                ChainPhase::Poa => {
                    // Buffer period: publish a preview; the PoA set stays.
                    let _ = T::Staking::elect(seats, true);
                }
            }
        }
    }

    #[pallet::call]
    impl<T: Config> Pallet<T> {
        /// Adds `key` to the PoA roster; it joins the set at the next epoch boundary. Only in
        /// the PoA phase.
        #[pallet::call_index(0)]
        #[pallet::weight(T::WeightInfo::add_poa_authority())]
        pub fn add_poa_authority(origin: OriginFor<T>, key: PqPublicKey) -> DispatchResult {
            T::AdminOrigin::ensure_origin(origin)?;
            ensure!(Phase::<T>::get() == ChainPhase::Poa, Error::<T>::NotPoa);
            ensure!(key.alg() == SigAlg::MlDsa65, Error::<T>::NotMlDsa65);
            let mut roster = PoaAuthorities::<T>::get();
            ensure!(!roster.contains(&key), Error::<T>::AlreadyAuthority);
            let size = u64::try_from(roster.len())
                .unwrap_or(u64::MAX)
                .saturating_add(1);
            ensure!(
                check_epoch_length(EpochLength::<T>::get(), size).is_ok(),
                Error::<T>::TooMany
            );
            roster
                .try_push(key.clone())
                .map_err(|_| Error::<T>::TooMany)?;
            PoaAuthorities::<T>::put(roster);
            Self::deposit_event(Event::PoaAuthorityAdded { key });
            Ok(())
        }

        /// Removes `key` from the PoA roster; it leaves the set at the next epoch boundary.
        /// Only in the PoA phase; the roster never becomes empty.
        #[pallet::call_index(1)]
        #[pallet::weight(T::WeightInfo::remove_poa_authority())]
        pub fn remove_poa_authority(origin: OriginFor<T>, key: PqPublicKey) -> DispatchResult {
            T::AdminOrigin::ensure_origin(origin)?;
            ensure!(Phase::<T>::get() == ChainPhase::Poa, Error::<T>::NotPoa);
            let mut roster = PoaAuthorities::<T>::get();
            ensure!(roster.contains(&key), Error::<T>::NotAuthority);
            ensure!(roster.len() > 1, Error::<T>::WouldEmpty);
            roster.retain(|k| k != &key);
            PoaAuthorities::<T>::put(roster);
            Self::deposit_event(Event::PoaAuthorityRemoved { key });
            Ok(())
        }

        /// Sets the active-set size `K` of PoS elections (effective from the next election).
        #[pallet::call_index(2)]
        #[pallet::weight(T::WeightInfo::set_validator_count())]
        pub fn set_validator_count(origin: OriginFor<T>, count: u32) -> DispatchResult {
            T::AdminOrigin::ensure_origin(origin)?;
            ensure!(
                count >= TransitionParams::<T>::get().min_candidates
                    && count <= T::MaxAuthorities::get()
                    && check_epoch_length(EpochLength::<T>::get(), u64::from(count)).is_ok(),
                Error::<T>::BadValidatorCount
            );
            ValidatorCount::<T>::put(count);
            Self::deposit_event(Event::ValidatorCountSet { count });
            Ok(())
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

        /// Phase, switch conditions and their current values.
        pub fn transition_progress() -> TransitionProgress {
            let height: u64 = frame_system::Pallet::<T>::block_number().saturated_into();
            let inputs = TransitionInputs::new(
                T::Staking::total_active(),
                T::Staking::total_issuance(),
                T::Staking::qualified_candidates(),
                height,
            );
            TransitionProgress::new(
                Phase::<T>::get(),
                (SwitchedAt::<T>::get(), QualifiedSince::<T>::get()),
                &inputs,
                TransitionParams::<T>::get(),
            )
        }

        /// Whether `on_finalize` of block `number` runs an election: in the last block of an
        /// epoch, always in PoS, and in PoA during the switch buffer (a preview).
        fn elects_after(number: u64) -> bool {
            let length = EpochLength::<T>::get();
            if length == 0 || !is_boundary(number.saturating_add(1), length) {
                return false;
            }
            match Phase::<T>::get() {
                ChainPhase::Pos => true,
                ChainPhase::Poa => QualifiedSince::<T>::get().is_some(),
            }
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

        /// Elected keys with their backings as weighted authorities (bounded by the maximum).
        fn weighted(elected: Vec<(PqPublicKey, u128)>) -> BoundedVec<Authority, T::MaxAuthorities> {
            BoundedVec::truncate_from(
                elected
                    .into_iter()
                    .map(|(key, backing)| Authority {
                        key,
                        weight: backing_to_weight(backing),
                    })
                    .collect(),
            )
        }

        /// The set for the boundary opening `epoch` at block `number`: runs the PoA checkpoint
        /// (possibly switching to PoS) or takes the PoS election. Returns the new members and
        /// the extra weight spent on staking reads and elections.
        fn next_members(
            epoch: EpochIndex,
            number: u64,
            removals: &[PqPublicKey],
        ) -> (Vec<Authority>, Weight) {
            let seats = ValidatorCount::<T>::get();
            match Phase::<T>::get() {
                ChainPhase::Poa => {
                    let inputs = TransitionInputs::new(
                        T::Staking::total_active(),
                        T::Staking::total_issuance(),
                        T::Staking::qualified_candidates(),
                        number,
                    );
                    let params = TransitionParams::<T>::get();
                    // Test-only fault (m3-pos 9.1), compiled only with `--cfg
                    // ac_test_early_switch` by `tests/early-switch-runtime`: ignore the minimum
                    // height and the sustain period.
                    #[cfg(ac_test_early_switch)]
                    let params = SwitchParams {
                        min_height: 0,
                        sustain_blocks: 0,
                        ..params
                    };
                    let step = transition_step(QualifiedSince::<T>::get(), &inputs, &params);
                    QualifiedSince::<T>::put(step.qualified_since);
                    Self::deposit_event(Event::TransitionCheckpoint {
                        qualified: step.ok,
                        since: step.qualified_since,
                    });
                    let mut weight = T::Staking::inputs_weight();
                    if step.switch {
                        weight = weight.saturating_add(T::Staking::election_weight(seats));
                        let elected = Self::weighted(T::Staking::elect(seats, false));
                        Phase::<T>::put(ChainPhase::Pos);
                        SwitchedAt::<T>::put(number);
                        PoaAuthorities::<T>::put(BoundedVec::new());
                        Self::deposit_event(Event::SwitchedToPos { block: number });
                        return (elected.into_inner(), weight);
                    }
                    // Offenders leave the roster for good (as in M2), never emptying it.
                    let mut roster = PoaAuthorities::<T>::get();
                    let first = roster.first().cloned();
                    roster.retain(|k| !removals.contains(k));
                    if roster.is_empty()
                        && let Some(first) = first
                    {
                        roster = BoundedVec::truncate_from(Vec::from([first]));
                    }
                    PoaAuthorities::<T>::put(roster.clone());
                    (roster.into_iter().map(Authority::poa).collect(), weight)
                }
                ChainPhase::Pos => match NextElected::<T>::take() {
                    Some((for_epoch, set)) if for_epoch == epoch => {
                        (set.into_inner(), Weight::zero())
                    }
                    _ => (
                        Self::weighted(T::Staking::elect(seats, false)).into_inner(),
                        T::Staking::election_weight(seats),
                    ),
                },
            }
        }

        /// Processes the boundary opening `epoch` at block `number`: removes offenders, runs
        /// the PoA checkpoint or takes the PoS election, and installs the new set if it
        /// differs. Returns whether the set changed and the extra weight used. Never panics;
        /// an unexpected failure leaves the set as it was.
        pub(crate) fn enact(epoch: EpochIndex, number: u64) -> (bool, Weight) {
            let removals = PendingRemovals::<T>::take();
            Self::prune_history(epoch);
            let current = Authorities::<T>::get();
            let (members, weight) = Self::next_members(epoch, number, &removals);
            let mut next: Vec<Authority> = members
                .iter()
                .filter(|a| !removals.contains(&a.key))
                .cloned()
                .collect();
            if next.is_empty() {
                // Never empty the set: keep the first candidate in order, or the current set.
                match members.first().or(current.first()) {
                    Some(first) => next.push(first.clone()),
                    None => return (false, weight),
                }
            }
            if next == current.to_vec() {
                return (false, weight);
            }
            let Ok(bounded) = BoundedVec::<Authority, T::MaxAuthorities>::try_from(next) else {
                return (false, weight);
            };
            if let Err(e) = T::BlockAuthorities::set_authorities(keys(&bounded)) {
                log::error!(target: LOG_TARGET, "cannot install the new authority set: {e}");
                return (false, weight);
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
            for removed in current
                .iter()
                .filter(|a| removals.contains(&a.key) && !bounded.iter().any(|b| b.key == a.key))
            {
                Self::deposit_event(Event::AuthorityDisabled {
                    key: removed.key.clone(),
                });
            }
            Self::deposit_event(Event::NewSet {
                set_id,
                size: u32::try_from(bounded.len()).unwrap_or(u32::MAX),
            });
            (true, weight)
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

        fn epoch_length() -> u64 {
            EpochLength::<T>::get()
        }

        fn phase() -> ChainPhase {
            Phase::<T>::get()
        }
    }
}
