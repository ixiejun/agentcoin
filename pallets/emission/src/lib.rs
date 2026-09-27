//! # Emission
//!
//! Scheduled emission of ATC (plan §5.1; decisions D11, D14–D16, D18, D19; design D2–D4 of
//! `m3-economics`) — the only way the runtime mints.
//!
//! - Blocks are grouped into emission epochs of [`EpochLength`] blocks (a genesis parameter that
//!   divides the four-year period). The first block of each epoch settles the previous one with
//!   [`ac_primitives::emission::settle`], the same function the node invariants and the economic
//!   simulation use.
//! - The security budget goes to [`Config::SecurityBudget`] (nobody during PoA: it rolls over),
//!   the market and public shares follow [`Config::WorkSource`] (no work before M5), and the
//!   treasury part goes to the accounts named by [`Config::Treasury`]. What is not minted stays
//!   in [`Reserve`].
//! - Every burn in the runtime goes through this pallet's `OnUnbalanced` implementation, which
//!   destroys the credit and adds it to [`TotalBurned`]. `TotalBurned` is a **published
//!   well-known storage key** read by the node invariants: never rename or re-encode it, and the
//!   pallet must stay named `Emission` in the runtime.

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

// FRAME's `pallet` macro expands to code (genesis defaults, storage metadata) that uses
// `expect` / `unreachable!` on our spans; hand-written code here uses neither except the
// documented genesis check.
#[allow(clippy::expect_used, clippy::unreachable)]
#[frame_support::pallet]
pub mod pallet {
    use super::WeightInfo;
    use ac_primitives::emission::{
        EmissionSchedule, EpochIndex, EpochInput, SecurityBudget, TreasuryDeposit, WorkSource,
        settle,
    };
    use alloc::vec::Vec;
    use frame_support::pallet_prelude::{
        BuildGenesisConfig, Get, Hooks, IsType, OptionQuery, StorageValue, ValueQuery, Weight,
    };
    use frame_support::traits::fungible::{Balanced, Credit, Mutate};
    use frame_support::traits::{Imbalance, OnUnbalanced};
    use frame_system::pallet_prelude::BlockNumberFor;
    use sp_runtime::SaturatedConversion;

    /// Balance type of the pallet: ATC in smallest units.
    pub type Balance = u128;

    #[pallet::pallet]
    pub struct Pallet<T>(_);

    #[pallet::config]
    pub trait Config: frame_system::Config {
        /// The overarching event type.
        #[allow(deprecated)] // Kept explicit for compatibility with the SDK's test tooling.
        type RuntimeEvent: From<Event<Self>> + IsType<<Self as frame_system::Config>::RuntimeEvent>;
        /// The native currency.
        type Currency: Mutate<Self::AccountId, Balance = Balance> + Balanced<Self::AccountId>;
        /// Verified work for the market and public shares (`()`: none).
        type WorkSource: WorkSource;
        /// Recipients of the security budget (`PoaPhase`: nobody).
        type SecurityBudget: SecurityBudget<Self::AccountId>;
        /// Treasury accounts.
        type Treasury: TreasuryDeposit<Self::AccountId>;
        /// Weights.
        type WeightInfo: WeightInfo;
    }

    /// Emission epoch length in blocks. Genesis parameter, written once; read by the node
    /// invariants from the chain spec.
    #[pallet::storage]
    pub type EpochLength<T: Config> = StorageValue<_, u64, OptionQuery>;

    /// Rollover reserve: scheduled amounts not minted yet.
    #[pallet::storage]
    pub type Reserve<T: Config> = StorageValue<_, Balance, ValueQuery>;

    /// Total minted by emission since genesis.
    #[pallet::storage]
    pub type TotalMinted<T: Config> = StorageValue<_, Balance, ValueQuery>;

    /// Total burned since genesis. **Well-known key** read by the node invariants
    /// (`twox128("Emission") ‖ twox128("TotalBurned")`, SCALE `u128`); written at genesis so
    /// it always exists.
    #[pallet::storage]
    pub type TotalBurned<T: Config> = StorageValue<_, Balance, OptionQuery>;

    /// Last settled epoch.
    #[pallet::storage]
    pub type LastSettled<T: Config> = StorageValue<_, EpochIndex, OptionQuery>;

    #[pallet::genesis_config]
    #[derive(frame_support::DefaultNoBound)]
    pub struct GenesisConfig<T: Config> {
        /// Emission epoch length; must divide the four-year period. 0 leaves emission unset (SDK
        /// tooling default; the node refuses to run such a chain).
        pub epoch_length: u64,
        /// Unused type marker.
        #[serde(skip)]
        pub _marker: core::marker::PhantomData<T>,
    }

    #[pallet::genesis_build]
    impl<T: Config> BuildGenesisConfig for GenesisConfig<T> {
        fn build(&self) {
            TotalBurned::<T>::put(0);
            if self.epoch_length == 0 {
                return;
            }
            if let Err(e) = EmissionSchedule::new(self.epoch_length) {
                // Genesis is built once, off chain, from a chain spec: a bad parameter must stop
                // the chain from being created at all (spec "非法纪元长度").
                #[allow(clippy::panic)]
                {
                    panic!("invalid emission genesis: {e}");
                }
            }
            EpochLength::<T>::put(self.epoch_length);
        }
    }

    #[pallet::event]
    #[pallet::generate_deposit(pub(super) fn deposit_event)]
    pub enum Event<T: Config> {
        /// An emission epoch was settled.
        EpochSettled {
            /// Settled epoch.
            epoch: EpochIndex,
            /// Its scheduled amount.
            scheduled: Balance,
            /// Security budget minted to validators.
            security: Balance,
            /// Market-work emission.
            market: Balance,
            /// Public-work emission.
            public: Balance,
            /// Treasury share proportional to work emission.
            treasury: Balance,
            /// Top-up to the treasury floor.
            floor_topup: Balance,
            /// Total minted.
            minted: Balance,
            /// Reserve afterwards.
            reserve: Balance,
        },
    }

    #[pallet::hooks]
    impl<T: Config> Hooks<BlockNumberFor<T>> for Pallet<T> {
        fn on_initialize(n: BlockNumberFor<T>) -> Weight {
            let Some(schedule) = Self::schedule() else {
                return T::DbWeight::get().reads(1);
            };
            match schedule.settled_epoch(n.saturated_into()) {
                Some(epoch) => {
                    Self::settle_epoch(&schedule, epoch);
                    T::WeightInfo::settle_epoch()
                }
                None => T::DbWeight::get().reads(1),
            }
        }
    }

    impl<T: Config> Pallet<T> {
        /// The emission curve, if the genesis set one.
        pub fn schedule() -> Option<EmissionSchedule> {
            EpochLength::<T>::get().and_then(|l| EmissionSchedule::new(l).ok())
        }

        /// Emission epoch of the next block.
        pub fn current_epoch() -> EpochIndex {
            let next = frame_system::Pallet::<T>::block_number()
                .saturated_into::<u64>()
                .saturating_add(1);
            Self::schedule().map_or(0, |s| s.settled_epochs(next))
        }

        /// Total burned since genesis.
        pub fn total_burned() -> Balance {
            TotalBurned::<T>::get().unwrap_or(0)
        }

        /// Mints `amount` to `who`; `false` (nothing minted) if the currency refuses, for
        /// example below the existential deposit of a new account.
        fn mint(who: &T::AccountId, amount: Balance) -> bool {
            amount == 0 || T::Currency::mint_into(who, amount).is_ok()
        }

        /// Settles `epoch`: computes the outcome, mints it and records the reserve. Parts the
        /// currency refuses to mint go back to the reserve, so reserve + minted always equals
        /// what the formula allots.
        pub(crate) fn settle_epoch(schedule: &EmissionSchedule, epoch: EpochIndex) {
            let scheduled = schedule.scheduled(epoch);
            let input = EpochInput::new(
                scheduled,
                Reserve::<T>::get(),
                T::WorkSource::verified_work(epoch),
                T::SecurityBudget::phase(),
            );
            let out = settle(&input);
            let mut payments: Vec<(T::AccountId, Balance)> =
                T::SecurityBudget::recipients(out.security);
            let mut paid_security = payments
                .iter()
                .fold(0u128, |a, (_, v)| a.saturating_add(*v));
            if paid_security > out.security {
                // A recipient list above the budget is a bug in the validator set; pay nothing
                // rather than more than the formula allows.
                payments.clear();
                paid_security = 0;
            }
            let [community, holder, (floor, floor_amount)] =
                T::Treasury::recipients(out.treasury_proportional, out.treasury_floor_topup);
            payments.push(community);
            payments.push(holder);
            // Market and public work are paid by their modules from M5 on; until then they
            // are zero because no work is verified.
            let mut minted = 0u128;
            for (who, amount) in &payments {
                if Self::mint(who, *amount) {
                    minted = minted.saturating_add(*amount);
                }
            }
            if floor_amount > 0 && Self::mint(&floor, floor_amount) {
                minted = minted.saturating_add(floor_amount);
                T::Treasury::floor_minted(floor_amount);
            }
            // Everything the formula allotted but that was not minted (unpaid security budget,
            // refused mints) returns to the reserve: reserve' = reserve + S − minted.
            let reserve = out.reserve.saturating_add(out.total.saturating_sub(minted));
            #[cfg(ac_test_overmint)]
            {
                // Test-only fault (m3-economics 8.1), compiled only with `--cfg ac_test_overmint`
                // by `tests/overmint-runtime`: mint twice the scheduled amount more.
                use parity_scale_codec::Decode;
                let extra = scheduled.saturating_mul(2);
                if let Ok(sink) =
                    T::AccountId::decode(&mut sp_runtime::traits::TrailingZeroInput::zeroes())
                    && Self::mint(&sink, extra)
                {
                    minted = minted.saturating_add(extra);
                }
            }
            Reserve::<T>::put(reserve);
            TotalMinted::<T>::mutate(|m| *m = m.saturating_add(minted));
            LastSettled::<T>::put(epoch);
            Self::deposit_event(Event::EpochSettled {
                epoch,
                scheduled,
                security: paid_security,
                market: out.market,
                public: out.public,
                treasury: out.treasury_proportional,
                floor_topup: out.treasury_floor_topup,
                minted,
                reserve,
            });
        }
    }

    /// Burns: dropping the credit reduces the issuance; the amount is added to [`TotalBurned`].
    impl<T: Config> OnUnbalanced<Credit<T::AccountId, T::Currency>> for Pallet<T> {
        fn on_nonzero_unbalanced(amount: Credit<T::AccountId, T::Currency>) {
            let value = amount.peek();
            drop(amount);
            TotalBurned::<T>::mutate(|b| *b = Some(b.unwrap_or(0).saturating_add(value)));
        }
    }
}
