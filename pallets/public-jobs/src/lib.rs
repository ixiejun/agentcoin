//! # Public jobs
//!
//! The public job queue (plan §5.6; m6-public-jobs; spec `market/public-jobs`):
//!
//! - **Workers** register with no stake, declare up to 8 models they can run and declare
//!   themselves ready once per round. The first block of each round freezes the roster: workers
//!   that were ready in the previous round, not suspended, and with room for more pending work.
//! - **Jobs** are published by the publisher origin (the PoA administration until M8): an
//!   evaluation, data cleaning or embedding job of up to 100,000 units, priced in dollars per
//!   unit and passing worker, optionally with a canary Merkle root.
//! - **Units**: each round opens up to [`PublicParams::units_per_round`] units and draws three
//!   eligible workers for each from the roster with the round's seed. Workers commit to a summary
//!   of their result, reveal it after the commit deadline, and the unit settles when two
//!   summaries agree under the job's rules: the majority earns the unit's price as public work,
//!   everyone else misses. A unit without agreement is reopened to fresh workers, at most three
//!   times. Three consecutive misses suspend a worker for a while.
//! - **Canaries**: anyone can reveal a canary leaf of an accepted unit with its Merkle proof; a
//!   majority that disagrees with it loses its locked rewards (burned) and all its unclaimed
//!   work, and is suspended; a minority that agreed with the canary earns the unit.
//! - **Rewards**: public work counts after the challenge period; emission settles it into this
//!   pallet's payout account ([`PublicPayout`]) and workers claim their share, which stays locked
//!   on their account for [`PublicParams::lock_blocks`] (rounded up to a sixteenth of that) and
//!   slashable until then.
//!
//! The calls workers make for assigned work (ready, commit, reveal, close, claim, withdraw) are
//! refunded when they succeed: zero stake, but a worker needs a little balance for the fee that
//! is taken up front (design D3).
//!
//! [`PublicParams::units_per_round`]: ac_primitives::market::public::PublicParams
//! [`PublicParams::lock_blocks`]: ac_primitives::market::public::PublicParams
//! [`PublicPayout`]: ac_primitives::emission::PublicPayout

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

use sp_core::H256;

/// Most units waiting to be reopened.
pub const MAX_REOPEN: u32 = 1_024;
/// Most pending-work epochs a worker keeps; a worker with a full list must claim before it is
/// put on a roster again.
pub const MAX_PENDING_EPOCHS: u32 = 32;
/// Most epochs one claim covers.
pub const MAX_CLAIM_EPOCHS: u32 = 16;

/// Randomness the rounds are seeded from.
pub trait JobsRandomness {
    /// A value derived from the latest randomness for `subject`, `None` before the first.
    fn random(subject: &[u8]) -> Option<H256>;
}

/// What the benchmarks need from the runtime: a rate, registered models and randomness.
#[cfg(feature = "runtime-benchmarks")]
pub trait BenchmarkHelper {
    /// Sets the reference rate to `rate` smallest units per dollar.
    fn set_rate(rate: u128);
    /// Registers `model`.
    fn register_model(model: ac_primitives::market::ModelId);
    /// Makes randomness available.
    fn set_randomness(seed: H256);
    /// Makes `epoch` the current emission epoch.
    fn set_epoch(epoch: ac_primitives::emission::EpochIndex);
}

// FRAME's `pallet` macro expands to code (genesis defaults, storage metadata, call decoding)
// that uses `expect` / `unreachable!` on our spans; hand-written code here uses neither except
// the documented genesis check.
#[allow(clippy::expect_used, clippy::unreachable)]
#[frame_support::pallet]
pub mod pallet {
    use super::{JobsRandomness, MAX_CLAIM_EPOCHS, MAX_PENDING_EPOCHS, MAX_REOPEN, WeightInfo};
    use ac_primitives::emission::{EpochIndex, EpochIndexSource, PublicPayout, WorkSource};
    use ac_primitives::market::audit::{RoundIndex, round_of, round_start};
    use ac_primitives::market::public::{
        Assignment, CanaryReveal, EpochPublic, JobId, JobRecord, JobSpec, LockedList,
        MAX_ACTIVE_JOBS, MAX_ATTEMPTS, MAX_PRICE_CAP, MAX_WORKERS, MISSES_TO_SUSPEND, PublicParams,
        REDUNDANCY, ROUND_SEED_SUBJECT, Reveal, Summary, UnitIndex, UnitRecord, UnitState,
        WorkerModels, WorkerRecord, WorkerReveal, agree, assign_unit, canary_leaf, check_spec,
        check_summary, commitment, max_per_worker, reference, verify_canary,
    };
    use ac_primitives::market::traits::{ModelLookup, PriceSource};
    use ac_primitives::market::usd::Rounding;
    use ac_primitives::market::{MicroUsd, work::JobKind};
    use alloc::collections::BTreeMap;
    use alloc::vec::Vec;
    use frame_support::PalletId;
    use frame_support::dispatch::{DispatchResultWithPostInfo, Pays};
    use frame_support::pallet_prelude::{
        BuildGenesisConfig, ConstU32, DispatchResult, Hooks, IsType, OptionQuery, StorageDoubleMap,
        StorageMap, StorageValue, ValueQuery, Weight, ensure,
    };
    use frame_support::traits::fungible::{
        Balanced, BalancedHold, Credit, Inspect, InspectHold, Mutate, MutateHold,
    };
    use frame_support::traits::tokens::{Fortitude, Precision, Preservation};
    use frame_support::traits::{EnsureOrigin, Get, Imbalance, OnUnbalanced};
    use frame_support::{BoundedVec, Identity, Twox64Concat};
    use frame_system::pallet_prelude::{BlockNumberFor, OriginFor};
    use sp_core::H256;
    use sp_runtime::traits::AccountIdConversion;
    use sp_runtime::{AccountId32, SaturatedConversion, Saturating};

    /// Balance of the native currency (smallest ATC units).
    pub type Balance = u128;
    /// A worker record of this runtime.
    pub type WorkerOf<T> = WorkerRecord<BlockNumberFor<T>>;
    /// A job record of this runtime.
    pub type JobOf<T> = JobRecord<BlockNumberFor<T>>;
    /// A unit record of this runtime.
    pub type UnitOf<T> = UnitRecord<AccountId32, BlockNumberFor<T>>;
    /// The current round's roster: workers and the models they run.
    pub type Roster = BoundedVec<(AccountId32, WorkerModels), ConstU32<MAX_WORKERS>>;
    /// A worker's unclaimed public work by maturity epoch.
    pub type PendingList = BoundedVec<(EpochIndex, Balance), ConstU32<MAX_PENDING_EPOCHS>>;

    /// The payout account's identifier: its account is derived from it.
    pub const POT_ID: PalletId = PalletId(*b"ac/publc");

    #[pallet::pallet]
    pub struct Pallet<T>(_);

    #[pallet::config]
    pub trait Config: frame_system::Config<AccountId = AccountId32, Hash = H256> {
        /// The overarching event type.
        #[allow(deprecated)] // Kept explicit for compatibility with the SDK's test tooling.
        type RuntimeEvent: From<Event<Self>> + IsType<<Self as frame_system::Config>::RuntimeEvent>;
        /// The overarching hold reason.
        type RuntimeHoldReason: From<HoldReason>;
        /// The native currency; claimed rewards are held with [`HoldReason::Locked`].
        type Currency: Inspect<AccountId32, Balance = Balance>
            + Mutate<AccountId32>
            + Balanced<AccountId32>
            + InspectHold<AccountId32, Reason = Self::RuntimeHoldReason>
            + MutateHold<AccountId32, Reason = Self::RuntimeHoldReason>
            + BalancedHold<AccountId32, Reason = Self::RuntimeHoldReason>;
        /// Registered models.
        type Models: ModelLookup;
        /// The reference rate.
        type Price: PriceSource;
        /// The chain's randomness.
        type Randomness: JobsRandomness;
        /// The current emission epoch.
        type Epochs: EpochIndexSource;
        /// Where slashed rewards go; the runtime burns them through `Emission`.
        type Burn: OnUnbalanced<Credit<AccountId32, Self::Currency>>;
        /// Who publishes and cancels jobs (the PoA administration until M8).
        type PublisherOrigin: EnsureOrigin<Self::RuntimeOrigin>;
        /// Who adjusts the price cap.
        type AdminOrigin: EnsureOrigin<Self::RuntimeOrigin>;
        /// Most settled unit records removed per block.
        #[pallet::constant]
        type PruneLimit: Get<u32>;
        /// Weights.
        type WeightInfo: WeightInfo;
        /// Sets up rates, models and randomness for the benchmarks.
        #[cfg(feature = "runtime-benchmarks")]
        type BenchmarkHelper: super::BenchmarkHelper;
    }

    /// Why this pallet holds funds.
    #[pallet::composite_enum]
    pub enum HoldReason {
        /// Claimed rewards that are still locked (slashable by a canary).
        #[codec(index = 0)]
        Locked,
    }

    /// Parameters.
    #[pallet::storage]
    pub type Params<T: Config> = StorageValue<_, PublicParams, OptionQuery>;

    /// Registered workers.
    #[pallet::storage]
    pub type Workers<T: Config> = StorageMap<_, Identity, AccountId32, WorkerOf<T>, OptionQuery>;

    /// Number of registered workers.
    #[pallet::storage]
    pub type WorkerCount<T: Config> = StorageValue<_, u32, ValueQuery>;

    /// The last round whose first block ran.
    #[pallet::storage]
    pub type LastRound<T: Config> = StorageValue<_, RoundIndex, OptionQuery>;

    /// The current round's roster.
    #[pallet::storage]
    pub type CurrentRoster<T: Config> = StorageValue<_, Roster, ValueQuery>;

    /// The current round's seed (absent without randomness).
    #[pallet::storage]
    pub type Seed<T: Config> = StorageValue<_, H256, OptionQuery>;

    /// Identifier of the next job.
    #[pallet::storage]
    pub type NextJob<T: Config> = StorageValue<_, JobId, ValueQuery>;

    /// Jobs.
    #[pallet::storage]
    pub type Jobs<T: Config> = StorageMap<_, Twox64Concat, JobId, JobOf<T>, OptionQuery>;

    /// Jobs with units still to open for the first time, in publication order.
    #[pallet::storage]
    pub type ActiveJobs<T: Config> =
        StorageValue<_, BoundedVec<JobId, ConstU32<MAX_ACTIVE_JOBS>>, ValueQuery>;

    /// Units by job and index.
    #[pallet::storage]
    pub type Units<T: Config> =
        StorageDoubleMap<_, Twox64Concat, JobId, Twox64Concat, UnitIndex, UnitOf<T>, OptionQuery>;

    /// Units waiting to be reopened, oldest first.
    #[pallet::storage]
    pub type Reopen<T: Config> =
        StorageValue<_, BoundedVec<(JobId, UnitIndex), ConstU32<MAX_REOPEN>>, ValueQuery>;

    /// Each worker's unsettled units.
    #[pallet::storage]
    pub type Assigned<T: Config> = StorageDoubleMap<
        _,
        Identity,
        AccountId32,
        Twox64Concat,
        (JobId, UnitIndex),
        (),
        OptionQuery,
    >;

    /// Each worker's unclaimed public work by maturity epoch.
    #[pallet::storage]
    pub type Pending<T: Config> = StorageMap<_, Identity, AccountId32, PendingList, ValueQuery>;

    /// Public work and emission by epoch.
    #[pallet::storage]
    pub type Epochs<T: Config> =
        StorageMap<_, Twox64Concat, EpochIndex, EpochPublic<Balance>, ValueQuery>;

    /// Emission in the payout account that workers have not claimed yet.
    #[pallet::storage]
    pub type Unclaimed<T: Config> = StorageValue<_, Balance, ValueQuery>;

    /// Each worker's locked rewards.
    #[pallet::storage]
    pub type Locked<T: Config> =
        StorageMap<_, Identity, AccountId32, LockedList<BlockNumberFor<T>>, ValueQuery>;

    /// Settled units in settlement order, to prune their records.
    #[pallet::storage]
    pub type SettledQueue<T: Config> =
        StorageMap<_, Twox64Concat, u64, (JobId, UnitIndex, BlockNumberFor<T>), OptionQuery>;

    /// Next free position and oldest unpruned position of [`SettledQueue`].
    #[pallet::storage]
    pub type QueueBounds<T: Config> = StorageValue<_, (u64, u64), ValueQuery>;

    #[pallet::genesis_config]
    pub struct GenesisConfig<T: Config> {
        /// Parameters.
        pub params: PublicParams,
        /// Unused type marker.
        #[serde(skip)]
        pub _marker: core::marker::PhantomData<T>,
    }

    impl<T: Config> Default for GenesisConfig<T> {
        fn default() -> Self {
            Self {
                params: PublicParams::LIVE,
                _marker: core::marker::PhantomData,
            }
        }
    }

    #[pallet::genesis_build]
    impl<T: Config> BuildGenesisConfig for GenesisConfig<T> {
        fn build(&self) {
            if self.params.check().is_err() {
                // Genesis is built once, off chain: a bad parameter must stop the chain from
                // being created at all (spec "参数与护栏", "创世参数越界").
                #[allow(clippy::panic)]
                {
                    panic!("invalid public jobs genesis parameters: {:?}", self.params);
                }
            }
            Params::<T>::put(self.params);
        }
    }

    #[pallet::event]
    #[pallet::generate_deposit(pub(super) fn deposit_event)]
    pub enum Event<T: Config> {
        /// A worker registered.
        WorkerRegistered {
            /// The worker.
            who: AccountId32,
        },
        /// A worker changed its models.
        ModelsSet {
            /// The worker.
            who: AccountId32,
        },
        /// A worker deregistered.
        WorkerDeregistered {
            /// The former worker.
            who: AccountId32,
        },
        /// A round started.
        RoundStarted {
            /// The round.
            round: RoundIndex,
            /// Workers on its roster.
            workers: u32,
            /// Units opened.
            opened: u32,
        },
        /// A job was published.
        JobPublished {
            /// Its identifier.
            job: JobId,
            /// Its kind.
            kind: JobKind,
            /// Its unit count.
            units: u32,
        },
        /// A job was cancelled.
        JobCancelled {
            /// The job.
            job: JobId,
        },
        /// A unit was opened.
        UnitOpened {
            /// The job.
            job: JobId,
            /// The unit.
            unit: UnitIndex,
            /// Its attempt.
            attempt: u8,
            /// The workers drawn.
            workers: [AccountId32; REDUNDANCY],
        },
        /// A worker committed.
        Committed {
            /// The job.
            job: JobId,
            /// The unit.
            unit: UnitIndex,
            /// The worker.
            who: AccountId32,
        },
        /// A worker revealed.
        Revealed {
            /// The job.
            job: JobId,
            /// The unit.
            unit: UnitIndex,
            /// The worker.
            who: AccountId32,
        },
        /// A unit passed.
        UnitAccepted {
            /// The job.
            job: JobId,
            /// The unit.
            unit: UnitIndex,
            /// Majority slots.
            majority: [bool; REDUNDANCY],
        },
        /// A unit did not pass and waits to be reopened.
        UnitRetry {
            /// The job.
            job: JobId,
            /// The unit.
            unit: UnitIndex,
        },
        /// A unit failed for good.
        UnitFailed {
            /// The job.
            job: JobId,
            /// The unit.
            unit: UnitIndex,
        },
        /// Public work was credited.
        WorkCredited {
            /// The worker.
            who: AccountId32,
            /// Epoch it matures in.
            epoch: EpochIndex,
            /// Amount.
            work: Balance,
        },
        /// Work could not be credited: the worker's pending list is full.
        WorkDropped {
            /// The worker.
            who: AccountId32,
            /// Amount.
            work: Balance,
        },
        /// A worker missed a unit.
        Missed {
            /// The worker.
            who: AccountId32,
        },
        /// A worker was suspended.
        Suspended {
            /// The worker.
            who: AccountId32,
            /// Block the suspension ends at.
            until: BlockNumberFor<T>,
        },
        /// A canary agreed with the unit's reference.
        CanaryPassed {
            /// The job.
            job: JobId,
            /// The unit.
            unit: UnitIndex,
        },
        /// A canary disagreed: the majority was punished.
        CanaryFailed {
            /// The job.
            job: JobId,
            /// The unit.
            unit: UnitIndex,
        },
        /// A worker's locked rewards were slashed and burned and its unclaimed work voided.
        Punished {
            /// The worker.
            who: AccountId32,
            /// Locked rewards burned.
            burned: Balance,
            /// Unclaimed work voided.
            voided: Balance,
        },
        /// Public emission of an epoch was settled into the payout account.
        PublicSettled {
            /// The epoch.
            epoch: EpochIndex,
            /// Emission minted for it.
            public: Balance,
            /// Its verified public work.
            work: Balance,
        },
        /// A worker claimed rewards (now locked).
        Claimed {
            /// The worker.
            who: AccountId32,
            /// Amount.
            amount: Balance,
            /// Block they unlock at.
            unlock_at: BlockNumberFor<T>,
        },
        /// A worker withdrew unlocked rewards.
        Withdrawn {
            /// The worker.
            who: AccountId32,
            /// Amount.
            amount: Balance,
        },
        /// The administration set the unit price cap.
        PriceCapSet {
            /// The new cap.
            cap: MicroUsd,
        },
    }

    #[pallet::error]
    pub enum Error<T> {
        /// The account is already a worker.
        AlreadyRegistered,
        /// The account is not a worker.
        NotWorker,
        /// The worker limit is reached.
        TooManyWorkers,
        /// A declared model is not registered.
        UnknownModel,
        /// The worker still has unsettled units.
        HasUnits,
        /// The worker already declared itself ready this round.
        AlreadyReady,
        /// Genesis parameters are missing.
        NotConfigured,
        /// The job specification breaks a rule.
        InvalidSpec,
        /// Too many jobs in progress.
        TooManyJobs,
        /// No such job.
        NoJob,
        /// The job was already cancelled.
        Cancelled,
        /// No such open unit.
        NoUnit,
        /// The caller is not assigned to the unit.
        NotAssigned,
        /// The caller already committed or revealed.
        AlreadySubmitted,
        /// The commit window is over.
        CommitClosed,
        /// Reveals open after the commit deadline and close at the reveal deadline.
        RevealWindow,
        /// The caller did not commit.
        NotCommitted,
        /// The reveal does not match the commitment.
        CommitmentMismatch,
        /// The summary does not have the job kind's format.
        BadSummary,
        /// The reveal deadline has not passed.
        RevealNotOver,
        /// The job has no canaries, the unit is not accepted, or its canary was revealed.
        NoCanary,
        /// The canary proof does not verify.
        BadProof,
        /// Nothing to claim.
        NothingToClaim,
        /// The locked list is full: withdraw first.
        LockedFull,
        /// Nothing is unlocked.
        NothingToWithdraw,
        /// The price cap is above $10.
        OutOfBounds,
    }

    #[pallet::hooks]
    impl<T: Config> Hooks<BlockNumberFor<T>> for Pallet<T> {
        fn on_initialize(now: BlockNumberFor<T>) -> Weight {
            let Some(params) = Params::<T>::get() else {
                return T::DbWeight::get().reads(1);
            };
            let round = round_of(now.saturated_into(), params.round_blocks);
            let mut weight = T::DbWeight::get().reads(2);
            if LastRound::<T>::get() != Some(round) {
                let (workers, opened) = Self::start_round(round, &params);
                weight = weight.saturating_add(T::WeightInfo::start_round(workers, opened));
            }
            let pruned = Self::prune_step(now, &params);
            weight.saturating_add(T::WeightInfo::prune(pruned))
        }
    }

    #[pallet::call]
    impl<T: Config> Pallet<T> {
        /// Registers the caller as a worker that runs `models` (spec "工作者登记与就绪").
        #[pallet::call_index(0)]
        #[pallet::weight(T::WeightInfo::register())]
        pub fn register(origin: OriginFor<T>, models: WorkerModels) -> DispatchResult {
            let who = frame_system::ensure_signed(origin)?;
            ensure!(
                !Workers::<T>::contains_key(&who),
                Error::<T>::AlreadyRegistered
            );
            let count = WorkerCount::<T>::get();
            ensure!(count < MAX_WORKERS, Error::<T>::TooManyWorkers);
            Self::check_models(&models)?;
            Workers::<T>::insert(
                &who,
                WorkerOf::<T> {
                    models,
                    last_ready: None,
                    misses: 0,
                    suspended_until: None,
                    accepted: 0,
                    missed: 0,
                },
            );
            WorkerCount::<T>::put(count.saturating_add(1));
            Self::deposit_event(Event::WorkerRegistered { who });
            Ok(())
        }

        /// Replaces the caller's models.
        #[pallet::call_index(1)]
        #[pallet::weight(T::WeightInfo::set_models())]
        pub fn set_models(origin: OriginFor<T>, models: WorkerModels) -> DispatchResult {
            let who = frame_system::ensure_signed(origin)?;
            let mut w = Workers::<T>::get(&who).ok_or(Error::<T>::NotWorker)?;
            Self::check_models(&models)?;
            w.models = models;
            Workers::<T>::insert(&who, w);
            Self::deposit_event(Event::ModelsSet { who });
            Ok(())
        }

        /// Deregisters the caller once it has no unsettled units. Its pending work and locked
        /// rewards stay claimable and withdrawable.
        #[pallet::call_index(2)]
        #[pallet::weight(T::WeightInfo::deregister())]
        pub fn deregister(origin: OriginFor<T>) -> DispatchResult {
            let who = frame_system::ensure_signed(origin)?;
            ensure!(Workers::<T>::contains_key(&who), Error::<T>::NotWorker);
            ensure!(
                Assigned::<T>::iter_prefix(&who).next().is_none(),
                Error::<T>::HasUnits
            );
            Workers::<T>::remove(&who);
            WorkerCount::<T>::mutate(|c| *c = c.saturating_sub(1));
            Self::deposit_event(Event::WorkerDeregistered { who });
            Ok(())
        }

        /// Declares the caller ready for the next round; free once per round.
        #[pallet::call_index(3)]
        #[pallet::weight(T::WeightInfo::ready())]
        pub fn ready(origin: OriginFor<T>) -> DispatchResultWithPostInfo {
            let who = frame_system::ensure_signed(origin)?;
            let params = Params::<T>::get().ok_or(Error::<T>::NotConfigured)?;
            let mut w = Workers::<T>::get(&who).ok_or(Error::<T>::NotWorker)?;
            let round = Self::current_round(&params);
            ensure!(w.last_ready != Some(round), Error::<T>::AlreadyReady);
            w.last_ready = Some(round);
            Workers::<T>::insert(&who, w);
            Ok(Pays::No.into())
        }

        /// Publishes a job (spec "任务发布与取消").
        #[pallet::call_index(4)]
        #[pallet::weight(T::WeightInfo::publish())]
        pub fn publish(origin: OriginFor<T>, spec: alloc::boxed::Box<JobSpec>) -> DispatchResult {
            T::PublisherOrigin::ensure_origin(origin)?;
            let params = Params::<T>::get().ok_or(Error::<T>::NotConfigured)?;
            check_spec(&spec, params.price_cap).map_err(|_| Error::<T>::InvalidSpec)?;
            if let Some(model) = &spec.model {
                ensure!(T::Models::exists(model), Error::<T>::UnknownModel);
            }
            let mut active = ActiveJobs::<T>::get();
            let job = NextJob::<T>::get();
            active.try_push(job).map_err(|_| Error::<T>::TooManyJobs)?;
            ActiveJobs::<T>::put(active);
            NextJob::<T>::put(job.saturating_add(1));
            let (kind, units) = (spec.kind, spec.units);
            Jobs::<T>::insert(
                job,
                JobOf::<T> {
                    spec: *spec,
                    published_at: frame_system::Pallet::<T>::block_number(),
                    opened: 0,
                    accepted: 0,
                    failed: 0,
                    cancelled: false,
                },
            );
            Self::deposit_event(Event::JobPublished { job, kind, units });
            Ok(())
        }

        /// Cancels a job: no further units open; opened units settle as usual.
        #[pallet::call_index(5)]
        #[pallet::weight(T::WeightInfo::cancel())]
        pub fn cancel(origin: OriginFor<T>, job: JobId) -> DispatchResult {
            T::PublisherOrigin::ensure_origin(origin)?;
            let mut j = Jobs::<T>::get(job).ok_or(Error::<T>::NoJob)?;
            ensure!(!j.cancelled, Error::<T>::Cancelled);
            j.cancelled = true;
            Jobs::<T>::insert(job, j);
            ActiveJobs::<T>::mutate(|a| a.retain(|x| *x != job));
            Self::deposit_event(Event::JobCancelled { job });
            Ok(())
        }

        /// Commits to a summary of the caller's result for an assigned unit.
        #[pallet::call_index(6)]
        #[pallet::weight(T::WeightInfo::commit())]
        pub fn commit(
            origin: OriginFor<T>,
            job: JobId,
            unit: UnitIndex,
            hash: H256,
        ) -> DispatchResultWithPostInfo {
            let who = frame_system::ensure_signed(origin)?;
            let mut u = Self::open_unit(job, unit)?;
            let slot = Self::slot(&u, &who)?;
            ensure!(
                frame_system::Pallet::<T>::block_number() <= u.commit_by,
                Error::<T>::CommitClosed
            );
            let c = u.commits.get_mut(slot).ok_or(Error::<T>::NotAssigned)?;
            ensure!(c.is_none(), Error::<T>::AlreadySubmitted);
            *c = Some(hash);
            Units::<T>::insert(job, unit, u);
            Self::deposit_event(Event::Committed { job, unit, who });
            Ok(Pays::No.into())
        }

        /// Reveals the committed summary, result hash and salt; settles the unit once all three
        /// workers have revealed.
        #[pallet::call_index(7)]
        #[pallet::weight(T::WeightInfo::reveal())]
        pub fn reveal(
            origin: OriginFor<T>,
            job: JobId,
            unit: UnitIndex,
            revealed: WorkerReveal,
        ) -> DispatchResultWithPostInfo {
            let who = frame_system::ensure_signed(origin)?;
            let WorkerReveal {
                summary,
                result_hash,
                salt,
            } = revealed;
            let mut u = Self::open_unit(job, unit)?;
            let spec = Jobs::<T>::get(job).ok_or(Error::<T>::NoJob)?.spec;
            let slot = Self::slot(&u, &who)?;
            let now = frame_system::Pallet::<T>::block_number();
            ensure!(
                now > u.commit_by && now <= u.reveal_by,
                Error::<T>::RevealWindow
            );
            let committed = u
                .commits
                .get(slot)
                .copied()
                .flatten()
                .ok_or(Error::<T>::NotCommitted)?;
            ensure!(
                u.reveals.get(slot).is_some_and(Option::is_none),
                Error::<T>::AlreadySubmitted
            );
            check_summary(spec.kind, spec.rules, &summary).map_err(|_| Error::<T>::BadSummary)?;
            let expected = commitment(&Reveal {
                job,
                unit,
                attempt: u.attempt,
                worker: &who,
                summary: &summary,
                result_hash: &result_hash,
                salt: &salt,
            });
            ensure!(expected == committed, Error::<T>::CommitmentMismatch);
            if let Some(r) = u.reveals.get_mut(slot) {
                *r = Some((summary, result_hash));
            }
            Self::deposit_event(Event::Revealed { job, unit, who });
            if u.reveals.iter().all(Option::is_some) {
                Self::settle(job, unit, u, &spec);
            } else {
                Units::<T>::insert(job, unit, u);
            }
            Ok(Pays::No.into())
        }

        /// Settles a unit whose reveal deadline has passed; free for its workers.
        #[pallet::call_index(8)]
        #[pallet::weight(T::WeightInfo::close())]
        pub fn close(
            origin: OriginFor<T>,
            job: JobId,
            unit: UnitIndex,
        ) -> DispatchResultWithPostInfo {
            let who = frame_system::ensure_signed(origin)?;
            let u = Self::open_unit(job, unit)?;
            let spec = Jobs::<T>::get(job).ok_or(Error::<T>::NoJob)?.spec;
            ensure!(
                frame_system::Pallet::<T>::block_number() > u.reveal_by,
                Error::<T>::RevealNotOver
            );
            let pays = if u.assigned.contains(&who) {
                Pays::No
            } else {
                Pays::Yes
            };
            Self::settle(job, unit, u, &spec);
            Ok(pays.into())
        }

        /// Reveals the canary of an accepted unit (spec "金丝雀单元").
        #[pallet::call_index(9)]
        #[pallet::weight(T::WeightInfo::reveal_canary())]
        pub fn reveal_canary(
            origin: OriginFor<T>,
            job: JobId,
            unit: UnitIndex,
            canary: alloc::boxed::Box<CanaryReveal>,
        ) -> DispatchResult {
            frame_system::ensure_signed(origin)?;
            let CanaryReveal {
                summary: expected,
                salt,
                proof,
            } = *canary;
            let mut j = Jobs::<T>::get(job).ok_or(Error::<T>::NoJob)?;
            let root = j.spec.canary_root.ok_or(Error::<T>::NoCanary)?;
            let mut u = Units::<T>::get(job, unit).ok_or(Error::<T>::NoUnit)?;
            let UnitState::Accepted {
                reference: r,
                majority,
            } = u.state
            else {
                return Err(Error::<T>::NoCanary.into());
            };
            ensure!(!u.canary_revealed, Error::<T>::NoCanary);
            check_summary(j.spec.kind, j.spec.rules, &expected)
                .map_err(|_| Error::<T>::BadSummary)?;
            let leaf = canary_leaf(job, unit, &expected, &salt);
            ensure!(verify_canary(&root, &leaf, &proof), Error::<T>::BadProof);
            u.canary_revealed = true;
            let reveal_of = |slot: usize| -> Option<Summary> {
                u.reveals
                    .get(slot)
                    .cloned()
                    .flatten()
                    .map(|(summary, _)| summary)
            };
            let base = reveal_of(usize::from(r)).ok_or(Error::<T>::NoCanary)?;
            if agree(j.spec.kind, j.spec.rules, &base, &expected) {
                Units::<T>::insert(job, unit, u);
                Self::deposit_event(Event::CanaryPassed { job, unit });
                return Ok(());
            }
            let params = Params::<T>::get().ok_or(Error::<T>::NotConfigured)?;
            let price = j.spec.price;
            for (slot, worker) in u.assigned.iter().enumerate() {
                let in_majority = majority.get(slot).copied().unwrap_or(false);
                if in_majority {
                    Self::punish(worker, &params);
                } else if reveal_of(slot)
                    .is_some_and(|s| agree(j.spec.kind, j.spec.rules, &s, &expected))
                {
                    Self::credit(worker, price, &params);
                    Workers::<T>::mutate(worker, |w| {
                        if let Some(w) = w {
                            w.misses = 0;
                        }
                    });
                }
            }
            u.state = UnitState::Failed;
            Units::<T>::insert(job, unit, u);
            j.accepted = j.accepted.saturating_sub(1);
            j.failed = j.failed.saturating_add(1);
            Jobs::<T>::insert(job, j);
            Self::deposit_event(Event::CanaryFailed { job, unit });
            Ok(())
        }

        /// Claims the caller's share of settled epochs; the rewards are locked.
        #[pallet::call_index(10)]
        #[pallet::weight(T::WeightInfo::claim(epochs.len().saturated_into()))]
        pub fn claim(
            origin: OriginFor<T>,
            epochs: BoundedVec<EpochIndex, ConstU32<MAX_CLAIM_EPOCHS>>,
        ) -> DispatchResultWithPostInfo {
            let who = frame_system::ensure_signed(origin)?;
            let params = Params::<T>::get().ok_or(Error::<T>::NotConfigured)?;
            let now = frame_system::Pallet::<T>::block_number();
            let unlock_at = Self::unlock_block(now, &params);
            let mut locked = Locked::<T>::get(&who);
            ensure!(
                locked.iter().any(|(at, _)| *at == unlock_at) || !locked.is_full(),
                Error::<T>::LockedFull
            );
            let mut pending = Pending::<T>::get(&who);
            let mut total = 0u128;
            for epoch in epochs {
                let Some(emission) = Epochs::<T>::get(epoch).emission else {
                    continue;
                };
                let Some(pos) = pending.iter().position(|(e, _)| *e == epoch) else {
                    continue;
                };
                let (_, work) = pending.remove(pos);
                let verified = Epochs::<T>::get(epoch).verified;
                total = total.saturating_add(part(emission, work, verified));
            }
            Pending::<T>::insert(&who, pending);
            ensure!(total > 0, Error::<T>::NothingToClaim);
            T::Currency::transfer(&Self::pot(), &who, total, Preservation::Preserve)?;
            T::Currency::hold(&HoldReason::Locked.into(), &who, total)?;
            Unclaimed::<T>::mutate(|u| *u = u.saturating_sub(total));
            match locked.iter_mut().find(|(at, _)| *at == unlock_at) {
                Some((_, amount)) => *amount = amount.saturating_add(total),
                None => locked
                    .try_push((unlock_at, total))
                    .map_err(|_| Error::<T>::LockedFull)?,
            }
            Locked::<T>::insert(&who, locked);
            Self::deposit_event(Event::Claimed {
                who,
                amount: total,
                unlock_at,
            });
            Ok(Pays::No.into())
        }

        /// Releases the caller's rewards whose lock has ended.
        #[pallet::call_index(11)]
        #[pallet::weight(T::WeightInfo::withdraw())]
        pub fn withdraw(origin: OriginFor<T>) -> DispatchResultWithPostInfo {
            let who = frame_system::ensure_signed(origin)?;
            let now = frame_system::Pallet::<T>::block_number();
            let mut locked = Locked::<T>::get(&who);
            let due: Balance = locked
                .iter()
                .filter(|(at, _)| *at <= now)
                .fold(0, |a, (_, x)| a.saturating_add(*x));
            ensure!(due > 0, Error::<T>::NothingToWithdraw);
            locked.retain(|(at, _)| *at > now);
            T::Currency::release(&HoldReason::Locked.into(), &who, due, Precision::BestEffort)?;
            if locked.is_empty() {
                Locked::<T>::remove(&who);
            } else {
                Locked::<T>::insert(&who, locked);
            }
            Self::deposit_event(Event::Withdrawn { who, amount: due });
            Ok(Pays::No.into())
        }

        /// Sets the unit price cap, at most $10.
        #[pallet::call_index(12)]
        #[pallet::weight(T::WeightInfo::set_price_cap())]
        pub fn set_price_cap(origin: OriginFor<T>, cap: MicroUsd) -> DispatchResult {
            T::AdminOrigin::ensure_origin(origin)?;
            ensure!(cap <= MAX_PRICE_CAP, Error::<T>::OutOfBounds);
            let mut params = Params::<T>::get().ok_or(Error::<T>::NotConfigured)?;
            params.price_cap = cap;
            Params::<T>::put(params);
            Self::deposit_event(Event::PriceCapSet { cap });
            Ok(())
        }
    }

    /// `value × num / den`, rounded down; 0 when `den` is 0.
    fn part(value: u128, num: u128, den: u128) -> u128 {
        sp_runtime::helpers_128bit::multiply_by_rational_with_rounding(
            value,
            num,
            den,
            sp_runtime::Rounding::Down,
        )
        .unwrap_or(0)
    }

    impl<T: Config> Pallet<T> {
        /// The payout account.
        #[must_use]
        pub fn pot() -> AccountId32 {
            POT_ID.into_account_truncating()
        }

        /// The round of the current block.
        #[must_use]
        pub fn current_round(params: &PublicParams) -> RoundIndex {
            round_of(
                frame_system::Pallet::<T>::block_number().saturated_into(),
                params.round_blocks,
            )
        }

        fn check_models(models: &WorkerModels) -> DispatchResult {
            ensure!(
                models.iter().all(T::Models::exists),
                Error::<T>::UnknownModel
            );
            Ok(())
        }

        fn open_unit(job: JobId, unit: UnitIndex) -> Result<UnitOf<T>, Error<T>> {
            let u = Units::<T>::get(job, unit).ok_or(Error::<T>::NoUnit)?;
            ensure!(u.state == UnitState::Open, Error::<T>::NoUnit);
            Ok(u)
        }

        fn slot(u: &UnitOf<T>, who: &AccountId32) -> Result<usize, Error<T>> {
            u.assigned
                .iter()
                .position(|a| a == who)
                .ok_or(Error::<T>::NotAssigned)
        }

        /// Block rewards claimed at `now` unlock at: `now + lock_blocks` rounded up to a
        /// sixteenth of `lock_blocks`, so claims of one stretch share a locked segment.
        pub(crate) fn unlock_block(
            now: BlockNumberFor<T>,
            params: &PublicParams,
        ) -> BlockNumberFor<T> {
            let bucket = (params.lock_blocks / 16).max(1);
            let at: u32 = now
                .saturated_into::<u32>()
                .saturating_add(params.lock_blocks);
            at.div_ceil(bucket).saturating_mul(bucket).saturated_into()
        }

        /// Freezes `round`'s roster and seed and opens its units. Returns the worker records read
        /// and the units opened.
        pub(crate) fn start_round(round: RoundIndex, params: &PublicParams) -> (u32, u32) {
            let now = frame_system::Pallet::<T>::block_number();
            let previous = round.checked_sub(1);
            let mut roster: Vec<(AccountId32, WorkerModels)> = Vec::new();
            let mut read = 0u32;
            for (who, w) in Workers::<T>::iter() {
                read = read.saturating_add(1);
                let suspended = w.suspended_until.is_some_and(|until| until > now);
                if previous.is_some() && w.last_ready == previous && !suspended {
                    roster.push((who, w.models));
                }
            }
            roster.sort_by(|a, b| a.0.cmp(&b.0));
            roster.retain(|(who, _)| !Pending::<T>::get(who).is_full());
            let mut subject = ROUND_SEED_SUBJECT.to_vec();
            subject.extend_from_slice(&round.to_le_bytes());
            let seed = T::Randomness::random(&subject);
            let workers = u32::try_from(roster.len()).unwrap_or(u32::MAX);
            let opened = match seed {
                Some(s) => Self::open_units(&roster, &s, params),
                None => 0,
            };
            // At most MAX_WORKERS records exist, so the roster always fits.
            CurrentRoster::<T>::put(Roster::truncate_from(roster));
            match seed {
                Some(s) => Seed::<T>::put(s),
                None => Seed::<T>::kill(),
            }
            LastRound::<T>::put(round);
            Self::deposit_event(Event::RoundStarted {
                round,
                workers,
                opened,
            });
            (read, opened)
        }

        /// Opens up to `units_per_round` units: reopened ones first, then new ones in
        /// publication order. A unit that cannot get three eligible workers waits for the next
        /// round, and so do its job's later units; other jobs still open theirs.
        fn open_units(
            roster: &[(AccountId32, WorkerModels)],
            seed: &H256,
            params: &PublicParams,
        ) -> u32 {
            let cap = max_per_worker(params.units_per_round, roster.len());
            let mut load: BTreeMap<AccountId32, u32> = BTreeMap::new();
            let mut opened = 0u32;
            let mut kept: Vec<(JobId, UnitIndex)> = Vec::new();
            for (job, unit) in Reopen::<T>::get() {
                if opened < params.units_per_round
                    && Self::open_unit_now((job, unit), roster, seed, (&mut load, cap), params)
                {
                    opened = opened.saturating_add(1);
                } else {
                    kept.push((job, unit));
                }
            }
            // `kept` is a subset of the stored list, so it fits.
            Reopen::<T>::put(BoundedVec::truncate_from(kept));
            let mut active = ActiveJobs::<T>::get();
            let mut done: Vec<JobId> = Vec::new();
            for job in active.iter().copied() {
                let Some(mut j) = Jobs::<T>::get(job) else {
                    done.push(job);
                    continue;
                };
                while j.opened < j.spec.units && opened < params.units_per_round {
                    if !Self::open_unit_now((job, j.opened), roster, seed, (&mut load, cap), params)
                    {
                        break;
                    }
                    j.opened = j.opened.saturating_add(1);
                    opened = opened.saturating_add(1);
                }
                if j.opened >= j.spec.units {
                    done.push(job);
                }
                Jobs::<T>::insert(job, j);
            }
            active.retain(|j| !done.contains(j));
            ActiveJobs::<T>::put(active);
            opened
        }

        /// Draws the workers of `(job, unit)`'s next attempt and opens it; `false` when fewer
        /// than three are eligible.
        fn open_unit_now(
            (job, unit): (JobId, UnitIndex),
            roster: &[(AccountId32, WorkerModels)],
            seed: &H256,
            (load, cap): (&mut BTreeMap<AccountId32, u32>, u32),
            params: &PublicParams,
        ) -> bool {
            let Some(j) = Jobs::<T>::get(job) else {
                return true;
            };
            let previous = Units::<T>::get(job, unit);
            let attempt = previous.as_ref().map_or(1, |u| u.attempt.saturating_add(1));
            let mut tried: Vec<AccountId32> = previous
                .as_ref()
                .map(|u| {
                    let mut t = u.tried.to_vec();
                    t.extend(u.assigned.iter().cloned());
                    t
                })
                .unwrap_or_default();
            tried.sort();
            tried.dedup();
            let eligible = |entry: &(AccountId32, WorkerModels)| {
                let (who, models) = entry;
                j.spec.model.as_ref().is_none_or(|m| models.contains(m))
                    && tried.binary_search(who).is_err()
                    && load.get(who).copied().unwrap_or(0) < cap
            };
            let drawn = assign_unit(roster, seed, (job, unit, attempt), eligible);
            let Ok(assigned) = <[(AccountId32, WorkerModels); REDUNDANCY]>::try_from(drawn) else {
                return false;
            };
            let assigned = assigned.map(|(who, _)| who);
            for who in &assigned {
                load.entry(who.clone())
                    .and_modify(|n| *n = n.saturating_add(1))
                    .or_insert(1);
                Assigned::<T>::insert(who, (job, unit), ());
            }
            let now = frame_system::Pallet::<T>::block_number();
            let commit_by = now.saturating_add(params.commit_blocks.saturated_into());
            let reveal_by = commit_by.saturating_add(params.reveal_blocks.saturated_into());
            Units::<T>::insert(
                job,
                unit,
                UnitOf::<T> {
                    attempt,
                    opened_at: now,
                    commit_by,
                    reveal_by,
                    assigned: assigned.clone(),
                    tried: BoundedVec::truncate_from(tried),
                    commits: [None, None, None],
                    reveals: [None, None, None],
                    state: UnitState::Open,
                    settled_at: None,
                    canary_revealed: false,
                },
            );
            Self::deposit_event(Event::UnitOpened {
                job,
                unit,
                attempt,
                workers: assigned,
            });
            true
        }

        /// Settles an open unit (spec "单元结算").
        fn settle(job: JobId, unit: UnitIndex, mut u: UnitOf<T>, spec: &JobSpec) {
            let Some(params) = Params::<T>::get() else {
                return;
            };
            let now = frame_system::Pallet::<T>::block_number();
            let refs: [Option<&[u8]>; REDUNDANCY] = [
                Self::summary_of(&u, 0),
                Self::summary_of(&u, 1),
                Self::summary_of(&u, 2),
            ];
            let outcome = reference(spec.kind, spec.rules, &refs);
            let mut job_record = Jobs::<T>::get(job);
            match outcome {
                Some(m) => {
                    for (slot, who) in u.assigned.iter().enumerate() {
                        if m.members.get(slot).copied().unwrap_or(false) {
                            Self::credit(who, spec.price, &params);
                            Workers::<T>::mutate(who, |w| {
                                if let Some(w) = w {
                                    w.misses = 0;
                                    w.accepted = w.accepted.saturating_add(1);
                                }
                            });
                        } else {
                            Self::miss(who, &params);
                        }
                    }
                    u.state = UnitState::Accepted {
                        reference: m.reference,
                        majority: m.members,
                    };
                    if let Some(j) = job_record.as_mut() {
                        j.accepted = j.accepted.saturating_add(1);
                    }
                    Self::queue_for_pruning(job, unit, now);
                    Self::deposit_event(Event::UnitAccepted {
                        job,
                        unit,
                        majority: m.members,
                    });
                }
                None => {
                    for who in &u.assigned {
                        Self::miss(who, &params);
                    }
                    let mut reopen = Reopen::<T>::get();
                    if u.attempt < MAX_ATTEMPTS && reopen.try_push((job, unit)).is_ok() {
                        Reopen::<T>::put(reopen);
                        u.state = UnitState::Retry;
                        Self::deposit_event(Event::UnitRetry { job, unit });
                    } else {
                        u.state = UnitState::Failed;
                        if let Some(j) = job_record.as_mut() {
                            j.failed = j.failed.saturating_add(1);
                        }
                        Self::queue_for_pruning(job, unit, now);
                        Self::deposit_event(Event::UnitFailed { job, unit });
                    }
                }
            }
            if let Some(j) = job_record {
                Jobs::<T>::insert(job, j);
            }
            for who in &u.assigned {
                Assigned::<T>::remove(who, (job, unit));
            }
            u.settled_at = Some(now);
            Units::<T>::insert(job, unit, u);
        }

        fn summary_of(u: &UnitOf<T>, slot: usize) -> Option<&[u8]> {
            u.reveals
                .get(slot)
                .and_then(Option::as_ref)
                .map(|(s, _)| s.as_slice())
        }

        /// Credits `price` (in dollars, at the current rate rounded down) as public work maturing
        /// after the challenge period. Nothing without a rate.
        fn credit(who: &AccountId32, price: MicroUsd, params: &PublicParams) {
            if let Ok(work) = T::Price::to_atc(price, Rounding::Payment) {
                Self::add_work(who, work, params);
            }
        }

        /// Test access to [`Self::add_work`] with the stored parameters.
        #[cfg(test)]
        pub(crate) fn credit_for_test(who: &AccountId32, work: Balance) {
            if let Some(params) = Params::<T>::get() {
                Self::add_work(who, work, &params);
            }
        }

        /// Adds `work` to `who`'s pending work of the epoch after the challenge period.
        pub(crate) fn add_work(who: &AccountId32, work: Balance, params: &PublicParams) {
            if work == 0 {
                return;
            }
            let epoch =
                T::Epochs::current_epoch().saturating_add(u64::from(params.challenge_epochs));
            let mut pending = Pending::<T>::get(who);
            let added = match pending.iter_mut().find(|(e, _)| *e == epoch) {
                Some((_, w)) => {
                    *w = w.saturating_add(work);
                    true
                }
                None => pending.try_push((epoch, work)).is_ok(),
            };
            if !added {
                Self::deposit_event(Event::WorkDropped {
                    who: who.clone(),
                    work,
                });
                return;
            }
            Pending::<T>::insert(who, pending);
            Epochs::<T>::mutate(epoch, |e| e.verified = e.verified.saturating_add(work));
            Self::deposit_event(Event::WorkCredited {
                who: who.clone(),
                epoch,
                work,
            });
        }

        /// Records a miss; the third consecutive one suspends the worker.
        fn miss(who: &AccountId32, params: &PublicParams) {
            let Some(mut w) = Workers::<T>::get(who) else {
                return;
            };
            w.misses = w.misses.saturating_add(1);
            w.missed = w.missed.saturating_add(1);
            Self::deposit_event(Event::Missed { who: who.clone() });
            if w.misses >= MISSES_TO_SUSPEND {
                w.misses = 0;
                Self::suspend(who, &mut w, params);
            }
            Workers::<T>::insert(who, w);
        }

        fn suspend(who: &AccountId32, w: &mut WorkerOf<T>, params: &PublicParams) {
            let until = frame_system::Pallet::<T>::block_number()
                .saturating_add(params.suspend_blocks.saturated_into());
            w.suspended_until = Some(until);
            Self::deposit_event(Event::Suspended {
                who: who.clone(),
                until,
            });
        }

        /// Burns `who`'s locked rewards, voids all its unclaimed work and suspends it.
        fn punish(who: &AccountId32, params: &PublicParams) {
            let locked = Locked::<T>::take(who);
            let amount = locked.iter().fold(0u128, |a, (_, x)| a.saturating_add(*x));
            let mut burned = 0u128;
            if amount > 0 {
                let (credit, _) = T::Currency::slash(&HoldReason::Locked.into(), who, amount);
                burned = burned.saturating_add(credit.peek());
                T::Burn::on_unbalanced(credit);
            }
            let mut voided = 0u128;
            for (epoch, work) in Pending::<T>::take(who) {
                voided = voided.saturating_add(work);
                let e = Epochs::<T>::get(epoch);
                match e.emission {
                    None => Epochs::<T>::mutate(epoch, |e| {
                        e.verified = e.verified.saturating_sub(work);
                    }),
                    Some(emission) => {
                        // The share was minted into the payout account: burn it there.
                        let share = part(emission, work, e.verified);
                        if share > 0
                            && let Ok(credit) = T::Currency::withdraw(
                                &Self::pot(),
                                share,
                                Precision::BestEffort,
                                Preservation::Preserve,
                                Fortitude::Polite,
                            )
                        {
                            Unclaimed::<T>::mutate(|u| *u = u.saturating_sub(credit.peek()));
                            burned = burned.saturating_add(credit.peek());
                            T::Burn::on_unbalanced(credit);
                        }
                    }
                }
            }
            if let Some(mut w) = Workers::<T>::get(who) {
                Self::suspend(who, &mut w, params);
                Workers::<T>::insert(who, w);
            }
            Self::deposit_event(Event::Punished {
                who: who.clone(),
                burned,
                voided,
            });
        }

        fn queue_for_pruning(job: JobId, unit: UnitIndex, now: BlockNumberFor<T>) {
            let (next, oldest) = QueueBounds::<T>::get();
            SettledQueue::<T>::insert(next, (job, unit, now));
            QueueBounds::<T>::put((next.saturating_add(1), oldest));
        }

        /// Removes at most [`Config::PruneLimit`] unit records settled more than
        /// `retention_blocks` ago. Returns the number removed.
        pub(crate) fn prune_step(now: BlockNumberFor<T>, params: &PublicParams) -> u32 {
            let (next, mut oldest) = QueueBounds::<T>::get();
            let mut removed = 0u32;
            while oldest < next && removed < T::PruneLimit::get() {
                let Some((job, unit, at)) = SettledQueue::<T>::get(oldest) else {
                    oldest = oldest.saturating_add(1);
                    continue;
                };
                if at.saturating_add(params.retention_blocks.saturated_into()) > now {
                    break;
                }
                SettledQueue::<T>::remove(oldest);
                // A unit reopened after it was queued is not removed while it is in use.
                if Units::<T>::get(job, unit).is_some_and(|u| u.settled_at == Some(at)) {
                    Units::<T>::remove(job, unit);
                }
                oldest = oldest.saturating_add(1);
                removed = removed.saturating_add(1);
            }
            QueueBounds::<T>::put((next, oldest));
            removed
        }

        /// The current round and its first and last block.
        #[must_use]
        pub fn round_bounds() -> Option<(RoundIndex, u32, u32)> {
            let params = Params::<T>::get()?;
            let r = Self::current_round(&params);
            Some((
                r,
                round_start(r, params.round_blocks),
                round_start(r.saturating_add(1), params.round_blocks).saturating_sub(1),
            ))
        }

        /// A worker's unsettled units (runtime API only).
        #[must_use]
        pub fn assigned(who: &AccountId32) -> Vec<Assignment<BlockNumberFor<T>>> {
            Assigned::<T>::iter_key_prefix(who)
                .filter_map(|(job, unit)| {
                    let u = Units::<T>::get(job, unit)?;
                    let slot = u.assigned.iter().position(|a| a == who)?;
                    Some(Assignment {
                        job,
                        unit,
                        attempt: u.attempt,
                        commit_by: u.commit_by,
                        reveal_by: u.reveal_by,
                        committed: u.commits.get(slot).is_some_and(Option::is_some),
                        revealed: u.reveals.get(slot).is_some_and(Option::is_some),
                    })
                })
                .collect()
        }

        /// Balance of the payout account.
        #[must_use]
        pub fn pot_balance() -> Balance {
            T::Currency::balance(&Self::pot())
        }
    }

    impl<T: Config> WorkSource for Pallet<T> {
        fn verified_work(epoch: EpochIndex) -> (u128, u128) {
            (0, Epochs::<T>::get(epoch).verified)
        }
    }

    impl<T: Config> PublicPayout<AccountId32> for Pallet<T> {
        fn account() -> Option<AccountId32> {
            Some(Self::pot())
        }

        fn settled(epoch: EpochIndex, minted: u128, work: u128) {
            // The payout account keeps one existential deposit forever, so claims never reap it
            // (as the market's, m5-work-settlement design D6).
            let pot = T::Currency::balance(&Self::pot());
            let floor = Unclaimed::<T>::get().saturating_add(T::Currency::minimum_balance());
            let public = minted.min(pot.saturating_sub(floor));
            Unclaimed::<T>::mutate(|u| *u = u.saturating_add(public));
            Epochs::<T>::mutate(epoch, |e| {
                e.verified = work;
                e.emission = Some(public);
            });
            Self::deposit_event(Event::PublicSettled {
                epoch,
                public,
                work,
            });
        }
    }
}
