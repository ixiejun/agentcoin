//! # Nominated proof of stake
//!
//! The staking ledger behind the PoA → PoS switch, the validator election and slashing (plan
//! §4.3; design D1–D4 of `m3-pos`; decisions D19, D24):
//!
//! - **Candidates** register an ML-DSA-65 validator key (with a proof of possession), bond a
//!   self-stake of at least 0.1% of the issuance and set a commission of 5%–100%.
//! - **Nominators** bond at least 0.001% of the issuance and nominate 1 to 16 candidates. An
//!   account is either a candidate or a nominator, never both.
//! - Both lists are capped. When a list is full a newcomer must bond more than the smallest
//!   entry, which is evicted and starts unbonding.
//! - **Unbonding**: stake stops counting immediately but stays held until it unlocks. A
//!   candidate's self-stake waits a fixed period (28 days on live chains); nominations leave
//!   through a network-wide queue that waits 2–28 days depending on how much stake is leaving
//!   ([`ac_primitives::staking::unbonding_unlock`]).
//! - **Commission** increases take effect after a delay (7 days on live chains), decreases at
//!   the next validator epoch.
//!
//! Stake is held with the `StakingPos` hold reason of the balances pallet; bonding, unbonding
//! and withdrawing never change the issuance.
//!
//! [`Ledger`] and [`Candidates`] are **published well-known storage keys** read by the node's
//! PoA → PoS check (`ac-invariants`): never rename or re-encode them, and the pallet must stay
//! named `StakingPos` in the runtime.

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

use frame_support::pallet_prelude::{BoundedVec, Get};
use parity_scale_codec::{Decode, DecodeWithMemTracking, Encode, MaxEncodedLen};
use scale_info::TypeInfo;

/// Balance type of the pallet: ATC in smallest units.
pub type Balance = u128;

/// Part of a ledger that is unbonding.
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
)]
pub struct UnlockChunk {
    /// Amount.
    pub value: Balance,
    /// Block from which it can be withdrawn.
    pub unlock_at: u64,
}

/// The stake of one account, stored under the well-known key `StakingPos::Ledger`. The first
/// field is what the node sums; the field order is part of the published format.
#[derive(
    Clone, Debug, PartialEq, Eq, Encode, Decode, DecodeWithMemTracking, MaxEncodedLen, TypeInfo,
)]
#[scale_info(skip_type_params(S))]
pub struct StakingLedger<S: Get<u32>> {
    /// Bonded and counting: self-stake of a candidate or the nominated amount.
    pub active: Balance,
    /// Unbonding parts, still held.
    pub unlocking: BoundedVec<UnlockChunk, S>,
}

impl<S: Get<u32>> Default for StakingLedger<S> {
    fn default() -> Self {
        Self {
            active: 0,
            unlocking: BoundedVec::new(),
        }
    }
}

impl<S: Get<u32>> StakingLedger<S> {
    /// Total held: active plus unbonding.
    pub fn total(&self) -> Balance {
        self.unlocking
            .iter()
            .fold(self.active, |acc, c| acc.saturating_add(c.value))
    }

    /// Adds an unbonding part. Never fails: when all chunk slots are taken, the part is
    /// merged into the latest chunk, which then unlocks at the later of the two heights.
    fn push_chunk(&mut self, value: Balance, unlock_at: u64) {
        if value == 0 {
            return;
        }
        if let Some(chunk) = self.unlocking.iter_mut().find(|c| c.unlock_at == unlock_at) {
            chunk.value = chunk.value.saturating_add(value);
            return;
        }
        if self
            .unlocking
            .try_push(UnlockChunk { value, unlock_at })
            .is_ok()
        {
            return;
        }
        if let Some(latest) = self.unlocking.iter_mut().max_by_key(|c| c.unlock_at) {
            latest.value = latest.value.saturating_add(value);
            latest.unlock_at = latest.unlock_at.max(unlock_at);
        }
    }

    /// Removes and returns the parts unlocked at `now`.
    fn take_unlocked(&mut self, now: u64) -> Balance {
        let mut released = 0u128;
        self.unlocking.retain(|c| {
            if c.unlock_at <= now {
                released = released.saturating_add(c.value);
                false
            } else {
                true
            }
        });
        released
    }

    /// Whether nothing is held any more.
    pub fn is_empty(&self) -> bool {
        self.active == 0 && self.unlocking.is_empty()
    }
}

/// Staking parameters set at genesis (design D10 of `m3-pos`).
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
pub struct StakingParams {
    /// Most candidates at once.
    pub max_candidates: u32,
    /// Most nominators at once.
    pub max_nominators: u32,
    /// Blocks a candidate's self-stake takes to unbond.
    pub self_unbond_blocks: u64,
    /// Shortest wait of a nomination leaving through the unbonding queue.
    pub nomination_unbond_min: u64,
    /// Longest wait of a nomination; the queue drains the whole active stake in this time.
    pub nomination_unbond_max: u64,
    /// Blocks before a commission increase takes effect.
    pub commission_delay: u64,
}

impl StakingParams {
    /// Live-chain values: 500 candidates, 2,000 nominators, 28-day self-unbonding, 2–28-day
    /// nomination unbonding, 7-day commission delay (one-second blocks).
    pub const LIVE: Self = Self {
        max_candidates: 500,
        max_nominators: 2_000,
        self_unbond_blocks: 2_419_200,
        nomination_unbond_min: 172_800,
        nomination_unbond_max: 2_419_200,
        commission_delay: 604_800,
    };
}

impl Default for StakingParams {
    fn default() -> Self {
        Self::LIVE
    }
}

// FRAME's `pallet` macro expands to code (genesis defaults, storage metadata) that uses
// `expect` / `unreachable!` on our spans; hand-written code here uses neither except the
// documented genesis check.
#[allow(clippy::expect_used, clippy::unreachable)]
#[frame_support::pallet]
pub mod pallet {
    use super::{Balance, StakingLedger, StakingParams, WeightInfo};
    use ac_crypto::{PqPublicKey, PqSignature, SigAlg};
    use ac_primitives::epoch::epoch_start;
    use ac_primitives::staking::{
        AccountStake, CandidateInfo, CandidateRecord, MAX_COMMISSION_BPS, MAX_NOMINATIONS,
        MIN_COMMISSION_BPS, UnbondingParams, VALIDATOR_POP_CONTEXT, min_nomination, min_self_bond,
        pop_statement, unbonding_unlock, validator_key_id,
    };
    use ac_primitives::validator_set::ValidatorSetInterface;
    use alloc::vec::Vec;
    use frame_support::Identity;
    use frame_support::pallet_prelude::{
        BoundedVec, BuildGenesisConfig, ConstU32, CountedStorageMap, DispatchResult,
        DispatchResultWithPostInfo, Get, IsType, OptionQuery, StorageMap, StorageValue, ValueQuery,
        ensure,
    };
    use frame_support::traits::fungible::{Inspect, InspectHold, Mutate, MutateHold};
    use frame_support::traits::tokens::Precision;
    use frame_system::pallet_prelude::{BlockNumberFor, OriginFor, ensure_signed};
    use sp_runtime::SaturatedConversion;
    use sp_runtime::traits::Zero;

    /// Nominated candidates of one nominator.
    pub type Targets<T> =
        BoundedVec<<T as frame_system::Config>::AccountId, ConstU32<MAX_NOMINATIONS>>;

    /// Ledger type of the runtime.
    pub type LedgerOf<T> = StakingLedger<<T as Config>::MaxUnlocking>;

    #[pallet::pallet]
    pub struct Pallet<T>(_);

    #[pallet::config]
    pub trait Config: frame_system::Config {
        /// The overarching event type.
        #[allow(deprecated)] // Kept explicit for compatibility with the SDK's test tooling.
        type RuntimeEvent: From<Event<Self>> + IsType<<Self as frame_system::Config>::RuntimeEvent>;
        /// The overarching hold reason.
        type RuntimeHoldReason: From<HoldReason>;
        /// The native currency; stake is held with [`HoldReason::Staking`].
        type Currency: Inspect<Self::AccountId, Balance = Balance>
            + Mutate<Self::AccountId>
            + InspectHold<Self::AccountId, Reason = Self::RuntimeHoldReason>
            + MutateHold<Self::AccountId, Reason = Self::RuntimeHoldReason>;
        /// Validator epochs (commission decreases take effect at the next boundary).
        type Epochs: ValidatorSetInterface;
        /// Upper bound of the genesis parameter `max_candidates` (weights assume it).
        #[pallet::constant]
        type MaxCandidates: Get<u32>;
        /// Upper bound of the genesis parameter `max_nominators` (weights assume it).
        #[pallet::constant]
        type MaxNominators: Get<u32>;
        /// Unbonding chunks per ledger; further parts merge into the latest chunk.
        #[pallet::constant]
        type MaxUnlocking: Get<u32>;
        /// Weights.
        type WeightInfo: WeightInfo;
    }

    /// Why this pallet holds funds.
    #[pallet::composite_enum]
    pub enum HoldReason {
        /// Bonded or unbonding stake.
        #[codec(index = 0)]
        Staking,
    }

    /// Genesis parameters.
    #[pallet::storage]
    pub type Params<T: Config> = StorageValue<_, StakingParams, ValueQuery>;

    /// Stake of every candidate and nominator. **Well-known key**: `Identity`-hashed account,
    /// SCALE [`StakingLedger`]; the node sums `active` over all entries.
    #[pallet::storage]
    pub type Ledger<T: Config> = StorageMap<_, Identity, T::AccountId, LedgerOf<T>, OptionQuery>;

    /// Candidates. **Well-known key**: `Identity`-hashed account, SCALE [`CandidateRecord`];
    /// the node counts qualified, unchilled candidates.
    #[pallet::storage]
    pub type Candidates<T: Config> =
        CountedStorageMap<_, Identity, T::AccountId, CandidateRecord, OptionQuery>;

    /// Nominators and their targets; the nominated amount is their [`Ledger`] entry.
    #[pallet::storage]
    pub type Nominators<T: Config> =
        CountedStorageMap<_, Identity, T::AccountId, Targets<T>, OptionQuery>;

    /// Owner of every validator key ever registered, by key identifier
    /// (`ac_primitives::staking::validator_key_id`). Entries are never removed: a key is never
    /// reused by another account, and offences committed with an old key can still be slashed.
    #[pallet::storage]
    pub type KeyOwner<T: Config> = StorageMap<_, Identity, [u8; 32], T::AccountId, OptionQuery>;

    /// Sum of all ledgers' `active` amounts.
    #[pallet::storage]
    pub type TotalActive<T: Config> = StorageValue<_, Balance, ValueQuery>;

    /// End of the network-wide nomination unbonding queue (a block height).
    #[pallet::storage]
    pub type UnbondQueueEnd<T: Config> = StorageValue<_, u64, ValueQuery>;

    #[pallet::genesis_config]
    #[derive(frame_support::DefaultNoBound)]
    pub struct GenesisConfig<T: Config> {
        /// Staking parameters; the list caps must not exceed the runtime's bounds.
        pub params: StakingParams,
        /// Unused type marker.
        #[serde(skip)]
        pub _marker: core::marker::PhantomData<T>,
    }

    #[pallet::genesis_build]
    impl<T: Config> BuildGenesisConfig for GenesisConfig<T> {
        fn build(&self) {
            let p = self.params;
            let valid = p.max_candidates <= T::MaxCandidates::get()
                && p.max_nominators <= T::MaxNominators::get()
                && p.nomination_unbond_min <= p.nomination_unbond_max
                && p.nomination_unbond_max > 0;
            if !valid {
                // Genesis is built once, off chain, from a chain spec: a bad parameter must stop
                // the chain from being created at all.
                #[allow(clippy::panic)]
                {
                    panic!("invalid staking genesis parameters: {p:?}");
                }
            }
            Params::<T>::put(p);
        }
    }

    #[pallet::event]
    #[pallet::generate_deposit(pub(super) fn deposit_event)]
    pub enum Event<T: Config> {
        /// A candidate registered.
        CandidateRegistered {
            /// Candidate account.
            who: T::AccountId,
            /// Self-stake bonded.
            value: Balance,
        },
        /// A candidate left, by itself or evicted from a full list; its self-stake unbonds.
        CandidateRemoved {
            /// Candidate account.
            who: T::AccountId,
            /// Whether a larger newcomer evicted it.
            evicted: bool,
        },
        /// A candidate changed its validator key.
        ValidatorKeySet {
            /// Candidate account.
            who: T::AccountId,
        },
        /// A nominator started nominating.
        Nominated {
            /// Nominator.
            who: T::AccountId,
            /// Amount bonded.
            value: Balance,
            /// Number of nominated candidates.
            targets: u32,
        },
        /// A nominator changed its targets.
        NominationsChanged {
            /// Nominator.
            who: T::AccountId,
        },
        /// A nominator stopped nominating, by itself or evicted from a full list; its stake
        /// unbonds.
        NominatorRemoved {
            /// Nominator.
            who: T::AccountId,
            /// Whether a larger newcomer evicted it.
            evicted: bool,
        },
        /// Stake was added.
        Bonded {
            /// Account.
            who: T::AccountId,
            /// Amount.
            value: Balance,
        },
        /// Stake started unbonding.
        Unbonded {
            /// Account.
            who: T::AccountId,
            /// Amount.
            value: Balance,
            /// Block from which it can be withdrawn.
            unlock_at: u64,
        },
        /// Unlocked stake was released.
        Withdrawn {
            /// Account.
            who: T::AccountId,
            /// Amount.
            value: Balance,
        },
        /// A commission change was scheduled.
        CommissionScheduled {
            /// Candidate.
            who: T::AccountId,
            /// New commission in basis points.
            commission_bps: u32,
            /// Block it takes effect at.
            effective_at: u64,
        },
        /// A candidate was paused from elections.
        Chilled {
            /// Candidate.
            who: T::AccountId,
        },
        /// A paused candidate asked to be elected again.
        Validating {
            /// Candidate.
            who: T::AccountId,
        },
    }

    #[pallet::error]
    pub enum Error<T> {
        /// Validator keys must be ML-DSA-65.
        NotMlDsa65,
        /// The proof of possession does not verify under the validator key.
        BadProof,
        /// The validator key is or was used by a candidate.
        KeyInUse,
        /// The account is already a candidate or a nominator, or still has stake unbonding
        /// from its previous role.
        AlreadyStaking,
        /// Below the minimum self-stake or nomination for the current issuance.
        BelowMinimum,
        /// The account is not a candidate.
        NotCandidate,
        /// The account is not a nominator.
        NotNominator,
        /// Nominations must name 1 to 16 distinct registered candidates.
        BadTargets,
        /// The list is full and the amount does not exceed its smallest entry.
        ListFull,
        /// Commission must be between 5% and 100%.
        CommissionOutOfRange,
        /// Not enough active stake.
        NotEnoughActive,
        /// The amount must be positive.
        ZeroAmount,
        /// The free balance cannot cover the amount.
        InsufficientBalance,
        /// The candidate is not paused.
        NotChilled,
        /// The account is neither a candidate nor a nominator.
        NotStaking,
    }

    #[pallet::call]
    impl<T: Config> Pallet<T> {
        /// Registers the caller as a candidate with validator `key`, bonding `value` as
        /// self-stake. `proof` is `key`'s signature over
        /// [`ac_primitives::staking::pop_statement`] with context `agentcoin/validator-pop/v1`.
        #[pallet::call_index(0)]
        #[pallet::weight(T::WeightInfo::register_candidate(T::MaxCandidates::get()))]
        pub fn register_candidate(
            origin: OriginFor<T>,
            key: PqPublicKey,
            proof: PqSignature,
            value: Balance,
            commission_bps: u32,
        ) -> DispatchResultWithPostInfo {
            let who = ensure_signed(origin)?;
            Self::ensure_idle(&who)?;
            ensure!(
                (MIN_COMMISSION_BPS..=MAX_COMMISSION_BPS).contains(&commission_bps),
                Error::<T>::CommissionOutOfRange
            );
            ensure!(
                value >= min_self_bond(T::Currency::total_issuance()),
                Error::<T>::BelowMinimum
            );
            Self::check_key(&who, &key, &proof)?;
            let params = Params::<T>::get();
            let scanned = if Candidates::<T>::count() >= params.max_candidates {
                let (smallest, scanned) =
                    Self::smallest(Candidates::<T>::iter_keys()).ok_or(Error::<T>::ListFull)?;
                ensure!(value > smallest.1, Error::<T>::ListFull);
                Self::bond(&who, value)?;
                Self::remove_candidate(&smallest.0, true);
                scanned
            } else {
                Self::bond(&who, value)?;
                0
            };
            KeyOwner::<T>::insert(validator_key_id(&key), &who);
            Candidates::<T>::insert(
                &who,
                CandidateRecord {
                    key,
                    commission_bps,
                    pending_commission: None,
                    chilled: false,
                },
            );
            Self::deposit_event(Event::CandidateRegistered { who, value });
            Ok(Some(T::WeightInfo::register_candidate(scanned)).into())
        }

        /// Adds `value` to the caller's active stake (candidate or nominator).
        #[pallet::call_index(1)]
        #[pallet::weight(T::WeightInfo::bond_extra())]
        pub fn bond_extra(origin: OriginFor<T>, value: Balance) -> DispatchResult {
            let who = ensure_signed(origin)?;
            ensure!(
                Candidates::<T>::contains_key(&who) || Nominators::<T>::contains_key(&who),
                Error::<T>::NotStaking
            );
            Self::bond(&who, value)?;
            Self::deposit_event(Event::Bonded { who, value });
            Ok(())
        }

        /// Replaces the caller's validator key; `proof` as in `register_candidate`. Takes
        /// effect from the next election.
        #[pallet::call_index(2)]
        #[pallet::weight(T::WeightInfo::set_validator_key())]
        pub fn set_validator_key(
            origin: OriginFor<T>,
            key: PqPublicKey,
            proof: PqSignature,
        ) -> DispatchResult {
            let who = ensure_signed(origin)?;
            let mut record = Candidates::<T>::get(&who).ok_or(Error::<T>::NotCandidate)?;
            Self::check_key(&who, &key, &proof)?;
            KeyOwner::<T>::insert(validator_key_id(&key), &who);
            record.key = key;
            Candidates::<T>::insert(&who, record);
            Self::deposit_event(Event::ValidatorKeySet { who });
            Ok(())
        }

        /// Withdraws the caller's candidacy; all its self-stake starts unbonding.
        #[pallet::call_index(3)]
        #[pallet::weight(T::WeightInfo::retire())]
        pub fn retire(origin: OriginFor<T>) -> DispatchResult {
            let who = ensure_signed(origin)?;
            ensure!(
                Candidates::<T>::contains_key(&who),
                Error::<T>::NotCandidate
            );
            Self::remove_candidate(&who, false);
            Ok(())
        }

        /// Bonds `value` and nominates `targets` (1 to 16 distinct candidates).
        #[pallet::call_index(4)]
        #[pallet::weight(T::WeightInfo::nominate(T::MaxNominators::get()))]
        pub fn nominate(
            origin: OriginFor<T>,
            value: Balance,
            targets: Vec<T::AccountId>,
        ) -> DispatchResultWithPostInfo {
            let who = ensure_signed(origin)?;
            Self::ensure_idle(&who)?;
            let targets = Self::check_targets(targets)?;
            ensure!(
                value >= min_nomination(T::Currency::total_issuance()),
                Error::<T>::BelowMinimum
            );
            let params = Params::<T>::get();
            let scanned = if Nominators::<T>::count() >= params.max_nominators {
                let (smallest, scanned) =
                    Self::smallest(Nominators::<T>::iter_keys()).ok_or(Error::<T>::ListFull)?;
                ensure!(value > smallest.1, Error::<T>::ListFull);
                Self::bond(&who, value)?;
                Self::remove_nominator(&smallest.0, true);
                scanned
            } else {
                Self::bond(&who, value)?;
                0
            };
            let count = u32::try_from(targets.len()).unwrap_or(u32::MAX);
            Nominators::<T>::insert(&who, targets);
            Self::deposit_event(Event::Nominated {
                who,
                value,
                targets: count,
            });
            Ok(Some(T::WeightInfo::nominate(scanned)).into())
        }

        /// Replaces the caller's targets without unbonding; effective from the next election.
        #[pallet::call_index(5)]
        #[pallet::weight(T::WeightInfo::set_nominations())]
        pub fn set_nominations(origin: OriginFor<T>, targets: Vec<T::AccountId>) -> DispatchResult {
            let who = ensure_signed(origin)?;
            ensure!(
                Nominators::<T>::contains_key(&who),
                Error::<T>::NotNominator
            );
            let targets = Self::check_targets(targets)?;
            Nominators::<T>::insert(&who, targets);
            Self::deposit_event(Event::NominationsChanged { who });
            Ok(())
        }

        /// Stops nominating; the whole nominated amount starts unbonding.
        #[pallet::call_index(6)]
        #[pallet::weight(T::WeightInfo::unnominate())]
        pub fn unnominate(origin: OriginFor<T>) -> DispatchResult {
            let who = ensure_signed(origin)?;
            ensure!(
                Nominators::<T>::contains_key(&who),
                Error::<T>::NotNominator
            );
            Self::remove_nominator(&who, false);
            Ok(())
        }

        /// Starts unbonding `value` of the caller's active stake. What stays active must be 0
        /// (nominators, which then stop nominating) or at least the current minimum; candidates
        /// leave entirely with `retire`.
        #[pallet::call_index(7)]
        #[pallet::weight(T::WeightInfo::unbond())]
        pub fn unbond(origin: OriginFor<T>, value: Balance) -> DispatchResult {
            let who = ensure_signed(origin)?;
            ensure!(!value.is_zero(), Error::<T>::ZeroAmount);
            let ledger = Ledger::<T>::get(&who).ok_or(Error::<T>::NotEnoughActive)?;
            ensure!(ledger.active >= value, Error::<T>::NotEnoughActive);
            let remaining = ledger.active.saturating_sub(value);
            let issuance = T::Currency::total_issuance();
            let self_stake = Candidates::<T>::contains_key(&who);
            if self_stake {
                ensure!(
                    remaining >= min_self_bond(issuance),
                    Error::<T>::BelowMinimum
                );
            } else if Nominators::<T>::contains_key(&who) {
                if remaining.is_zero() {
                    Self::remove_nominator(&who, false);
                    return Ok(());
                }
                ensure!(
                    remaining >= min_nomination(issuance),
                    Error::<T>::BelowMinimum
                );
            } else {
                return Err(Error::<T>::NotStaking.into());
            }
            Self::start_unbonding(&who, value, self_stake);
            Ok(())
        }

        /// Releases the caller's unlocked stake.
        #[pallet::call_index(8)]
        #[pallet::weight(T::WeightInfo::withdraw_unbonded())]
        pub fn withdraw_unbonded(origin: OriginFor<T>) -> DispatchResult {
            let who = ensure_signed(origin)?;
            let mut ledger = Ledger::<T>::get(&who).ok_or(Error::<T>::NotEnoughActive)?;
            let released = ledger.take_unlocked(Self::now());
            if released > 0 {
                T::Currency::release(
                    &HoldReason::Staking.into(),
                    &who,
                    released,
                    Precision::BestEffort,
                )?;
            }
            if ledger.is_empty() {
                Ledger::<T>::remove(&who);
            } else {
                Ledger::<T>::insert(&who, ledger);
            }
            Self::deposit_event(Event::Withdrawn {
                who,
                value: released,
            });
            Ok(())
        }

        /// Sets the caller's commission (5%–100%): an increase takes effect after the
        /// commission delay, a decrease at the next validator epoch.
        #[pallet::call_index(9)]
        #[pallet::weight(T::WeightInfo::set_commission())]
        pub fn set_commission(origin: OriginFor<T>, commission_bps: u32) -> DispatchResult {
            let who = ensure_signed(origin)?;
            ensure!(
                (MIN_COMMISSION_BPS..=MAX_COMMISSION_BPS).contains(&commission_bps),
                Error::<T>::CommissionOutOfRange
            );
            let mut record = Candidates::<T>::get(&who).ok_or(Error::<T>::NotCandidate)?;
            let now = Self::now();
            record.commission_bps = record.commission_at(now);
            let effective_at = if commission_bps > record.commission_bps {
                now.saturating_add(Params::<T>::get().commission_delay)
            } else {
                Self::next_epoch_start()
            };
            record.pending_commission =
                (commission_bps != record.commission_bps).then_some((commission_bps, effective_at));
            Candidates::<T>::insert(&who, record);
            Self::deposit_event(Event::CommissionScheduled {
                who,
                commission_bps,
                effective_at,
            });
            Ok(())
        }

        /// Pauses the caller from elections; its stake stays bonded.
        #[pallet::call_index(10)]
        #[pallet::weight(T::WeightInfo::chill())]
        pub fn chill(origin: OriginFor<T>) -> DispatchResult {
            let who = ensure_signed(origin)?;
            ensure!(
                Candidates::<T>::contains_key(&who),
                Error::<T>::NotCandidate
            );
            Self::set_chilled(&who);
            Ok(())
        }

        /// Ends the caller's pause; its self-stake must reach the current minimum.
        #[pallet::call_index(11)]
        #[pallet::weight(T::WeightInfo::validate())]
        pub fn validate(origin: OriginFor<T>) -> DispatchResult {
            let who = ensure_signed(origin)?;
            let mut record = Candidates::<T>::get(&who).ok_or(Error::<T>::NotCandidate)?;
            ensure!(record.chilled, Error::<T>::NotChilled);
            ensure!(
                Self::active(&who) >= min_self_bond(T::Currency::total_issuance()),
                Error::<T>::BelowMinimum
            );
            record.chilled = false;
            Candidates::<T>::insert(&who, record);
            Self::deposit_event(Event::Validating { who });
            Ok(())
        }
    }

    impl<T: Config> Pallet<T> {
        /// Current block height.
        pub(crate) fn now() -> u64 {
            frame_system::Pallet::<T>::block_number().saturated_into()
        }

        /// First block of the next validator epoch.
        fn next_epoch_start() -> u64 {
            let length = T::Epochs::epoch_length();
            T::Epochs::current_epoch()
                .checked_add(1)
                .and_then(|e| epoch_start(e, length))
                .unwrap_or(u64::MAX)
        }

        /// Active stake of `who`.
        pub fn active(who: &T::AccountId) -> Balance {
            Ledger::<T>::get(who).map_or(0, |l| l.active)
        }

        /// The account neither stakes nor unbonds (role changes need an empty ledger).
        fn ensure_idle(who: &T::AccountId) -> DispatchResult {
            ensure!(
                !Candidates::<T>::contains_key(who)
                    && !Nominators::<T>::contains_key(who)
                    && !Ledger::<T>::contains_key(who),
                Error::<T>::AlreadyStaking
            );
            Ok(())
        }

        /// Checks a validator key: ML-DSA-65, never used, and signed the statement.
        fn check_key(who: &T::AccountId, key: &PqPublicKey, proof: &PqSignature) -> DispatchResult {
            ensure!(key.alg() == SigAlg::MlDsa65, Error::<T>::NotMlDsa65);
            ensure!(
                !KeyOwner::<T>::contains_key(validator_key_id(key)),
                Error::<T>::KeyInUse
            );
            let genesis = frame_system::Pallet::<T>::block_hash(BlockNumberFor::<T>::zero());
            let statement = pop_statement(&genesis, who, key);
            ac_crypto::sig::verify(key, &statement, VALIDATOR_POP_CONTEXT, proof)
                .map_err(|_| Error::<T>::BadProof)?;
            Ok(())
        }

        /// Checks nomination targets: 1 to 16 distinct registered candidates.
        fn check_targets(targets: Vec<T::AccountId>) -> Result<Targets<T>, Error<T>> {
            ensure!(!targets.is_empty(), Error::<T>::BadTargets);
            let mut sorted = targets.clone();
            sorted.sort();
            sorted.dedup();
            ensure!(sorted.len() == targets.len(), Error::<T>::BadTargets);
            ensure!(
                targets.iter().all(Candidates::<T>::contains_key),
                Error::<T>::BadTargets
            );
            Targets::<T>::try_from(targets).map_err(|_| Error::<T>::BadTargets)
        }

        /// The entry with the smallest active stake among `accounts` (the first such in
        /// iteration order, which is deterministic), and how many entries were scanned.
        fn smallest(
            accounts: impl Iterator<Item = T::AccountId>,
        ) -> Option<((T::AccountId, Balance), u32)> {
            let mut scanned = 0u32;
            let mut smallest: Option<(T::AccountId, Balance)> = None;
            for who in accounts {
                scanned = scanned.saturating_add(1);
                let active = Self::active(&who);
                if smallest.as_ref().is_none_or(|(_, s)| active < *s) {
                    smallest = Some((who, active));
                }
            }
            smallest.map(|s| (s, scanned))
        }

        /// Holds `value` more for `who` and counts it as active.
        fn bond(who: &T::AccountId, value: Balance) -> DispatchResult {
            ensure!(!value.is_zero(), Error::<T>::ZeroAmount);
            T::Currency::hold(&HoldReason::Staking.into(), who, value)
                .map_err(|_| Error::<T>::InsufficientBalance)?;
            let mut ledger = Ledger::<T>::get(who).unwrap_or_default();
            ledger.active = ledger.active.saturating_add(value);
            Ledger::<T>::insert(who, ledger);
            TotalActive::<T>::mutate(|t| *t = t.saturating_add(value));
            Ok(())
        }

        /// Moves `value` (at most the active stake) of `who` to unbonding: self-stake waits
        /// the fixed period, a nomination joins the queue.
        fn start_unbonding(who: &T::AccountId, value: Balance, self_stake: bool) {
            let Some(mut ledger) = Ledger::<T>::get(who) else {
                return;
            };
            let value = value.min(ledger.active);
            if value.is_zero() {
                return;
            }
            let now = Self::now();
            let params = Params::<T>::get();
            let unlock_at = if self_stake {
                now.saturating_add(params.self_unbond_blocks)
            } else {
                let (unlock_at, end) = unbonding_unlock(
                    now,
                    UnbondQueueEnd::<T>::get(),
                    value,
                    TotalActive::<T>::get(),
                    &UnbondingParams::new(
                        params.nomination_unbond_min,
                        params.nomination_unbond_max,
                    ),
                );
                UnbondQueueEnd::<T>::put(end);
                unlock_at
            };
            ledger.active = ledger.active.saturating_sub(value);
            ledger.push_chunk(value, unlock_at);
            Ledger::<T>::insert(who, ledger);
            TotalActive::<T>::mutate(|t| *t = t.saturating_sub(value));
            Self::deposit_event(Event::Unbonded {
                who: who.clone(),
                value,
                unlock_at,
            });
        }

        /// Removes a candidate; its whole self-stake unbonds with the fixed period.
        fn remove_candidate(who: &T::AccountId, evicted: bool) {
            if Candidates::<T>::take(who).is_none() {
                return;
            }
            Self::start_unbonding(who, Self::active(who), true);
            Self::deposit_event(Event::CandidateRemoved {
                who: who.clone(),
                evicted,
            });
        }

        /// Removes a nominator; its whole nominated amount unbonds through the queue.
        fn remove_nominator(who: &T::AccountId, evicted: bool) {
            if Nominators::<T>::take(who).is_none() {
                return;
            }
            Self::start_unbonding(who, Self::active(who), false);
            Self::deposit_event(Event::NominatorRemoved {
                who: who.clone(),
                evicted,
            });
        }

        /// Active, unbonding and withdrawable stake of `who`.
        pub fn stake_of(who: &T::AccountId) -> AccountStake {
            let Some(ledger) = Ledger::<T>::get(who) else {
                return AccountStake::default();
            };
            let now = Self::now();
            let (ready, waiting) = ledger.unlocking.iter().fold((0u128, 0u128), |(r, w), c| {
                if c.unlock_at <= now {
                    (r.saturating_add(c.value), w)
                } else {
                    (r, w.saturating_add(c.value))
                }
            });
            AccountStake::new(ledger.active, waiting, ready)
        }

        /// Candidate record of `who` with the nominations it receives (scans all nominators;
        /// for queries only).
        pub fn candidate_info(who: &T::AccountId) -> Option<CandidateInfo<T::AccountId>> {
            let record = Candidates::<T>::get(who)?;
            let nominators = Nominators::<T>::iter()
                .filter(|(_, targets)| targets.contains(who))
                .map(|(n, _)| {
                    let active = Self::active(&n);
                    (n, active)
                })
                .collect();
            Some(CandidateInfo::new(
                record,
                Self::now(),
                Self::active(who),
                nominators,
            ))
        }

        /// Whether `who` is a qualified candidate: registered, not chilled, and with an active
        /// self-stake of at least the current minimum. Only qualified candidates count for the
        /// PoA → PoS switch and take part in elections.
        pub fn is_qualified(who: &T::AccountId) -> bool {
            Candidates::<T>::get(who).is_some_and(|c| {
                !c.chilled && Self::active(who) >= min_self_bond(T::Currency::total_issuance())
            })
        }

        /// Commission of candidate `who` in force now.
        pub fn commission(who: &T::AccountId) -> Option<u32> {
            Candidates::<T>::get(who).map(|c| c.commission_at(Self::now()))
        }

        /// Current minimum self-stake and minimum nomination.
        pub fn minimums() -> (Balance, Balance) {
            let issuance = T::Currency::total_issuance();
            (min_self_bond(issuance), min_nomination(issuance))
        }

        /// Pauses `who` from elections if it is a candidate.
        pub fn set_chilled(who: &T::AccountId) {
            Candidates::<T>::mutate(who, |record| {
                if let Some(record) = record
                    && !record.chilled
                {
                    record.chilled = true;
                    Self::deposit_event(Event::Chilled { who: who.clone() });
                }
            });
        }
    }
}
