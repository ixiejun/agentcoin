//! # PoA administration
//!
//! The chain's administration during proof of authority (decision D41; design D8 of
//! `m3-economics`): a set of ML-DSA member accounts, fixed at genesis, that decide together with
//! a threshold `t` (`1 ≤ t ≤ members`).
//!
//! - Proposals, votes and closing are handled by the SDK's `pallet-collective`, instance
//!   [`CouncilInstance`] (named `PoaCouncil` in the runtime). Motions are identified by the
//!   chain hash (BLAKE3) of their call; a member who does not vote counts as a no.
//! - [`EnsureCouncilThreshold`] accepts only a collective motion approved by at least
//!   [`Threshold`] members; the runtime uses it as its administration origin.
//! - [`Pallet::dispatch_as_root`] lets such a motion run a call as Root (runtime upgrades with
//!   `System::set_code`, for example), after [`Config::RootCallFilter`] approved it.
//! - Members and threshold change only together, through [`Pallet::set_members`] and
//!   [`Pallet::set_threshold`], so `1 ≤ t ≤ members` always holds and the administration can
//!   never lock itself out.

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

use core::marker::PhantomData;
use frame_support::traits::{EnsureOrigin, OriginTrait};
use pallet_collective::RawOrigin as CouncilOrigin;

/// The `pallet-collective` instance of the PoA council.
pub type CouncilInstance = pallet_collective::Instance1;

/// Default motion duration: 7 days of 1-second blocks.
pub const DEFAULT_MOTION_DURATION: u32 = 7 * 24 * 60 * 60;

/// Accepts a PoA council motion approved by at least [`Threshold`] members.
pub struct EnsureCouncilThreshold<T>(PhantomData<T>);

impl<T: Config, O> EnsureOrigin<O> for EnsureCouncilThreshold<T>
where
    O: OriginTrait + From<CouncilOrigin<T::AccountId, CouncilInstance>>,
    for<'a> &'a O::PalletsOrigin: TryInto<&'a CouncilOrigin<T::AccountId, CouncilInstance>>,
{
    type Success = ();

    fn try_origin(o: O) -> Result<Self::Success, O> {
        let threshold = Threshold::<T>::get();
        // A threshold of 0 means the administration is not configured: nobody passes.
        let approved = matches!(
            o.caller().try_into(),
            Ok(CouncilOrigin::Members(yes, _)) if threshold > 0 && *yes >= threshold
        );
        if approved { Ok(()) } else { Err(o) }
    }

    #[cfg(feature = "runtime-benchmarks")]
    fn try_successful_origin() -> Result<O, ()> {
        let t = Threshold::<T>::get().max(1);
        Ok(O::from(CouncilOrigin::Members(t, t)))
    }
}

// FRAME's `pallet` macro expands to code (genesis defaults, storage metadata) that uses
// `expect` / `unreachable!` on our spans; hand-written code here uses neither except the
// documented genesis check.
#[allow(clippy::expect_used, clippy::unreachable)]
#[frame_support::pallet]
pub mod pallet {
    use super::{CouncilInstance, DEFAULT_MOTION_DURATION, WeightInfo};
    use alloc::boxed::Box;
    use alloc::vec::Vec;
    use frame_support::dispatch::{GetDispatchInfo, RawOrigin};
    use frame_support::pallet_prelude::{
        BuildGenesisConfig, DispatchResult, EnsureOrigin, Get, IsType, Parameter, StorageValue,
        ValueQuery, ensure,
    };
    use frame_support::traits::{ChangeMembers, Contains, UnfilteredDispatchable};
    use frame_system::pallet_prelude::{BlockNumberFor, OriginFor};
    use pallet_collective::WeightInfo as _;
    use sp_runtime::SaturatedConversion;

    type Collective<T> = pallet_collective::Pallet<T, CouncilInstance>;

    #[pallet::pallet]
    pub struct Pallet<T>(_);

    #[pallet::config]
    pub trait Config: frame_system::Config + pallet_collective::Config<CouncilInstance> {
        /// The overarching event type.
        #[allow(deprecated)] // Kept explicit for compatibility with the SDK's test tooling.
        type RuntimeEvent: From<Event<Self>> + IsType<<Self as frame_system::Config>::RuntimeEvent>;
        /// The runtime call type.
        type RuntimeCall: Parameter
            + UnfilteredDispatchable<RuntimeOrigin = <Self as frame_system::Config>::RuntimeOrigin>
            + GetDispatchInfo
            + From<frame_system::Call<Self>>;
        /// The administration: [`super::EnsureCouncilThreshold`] in the runtime.
        type AdminOrigin: EnsureOrigin<<Self as frame_system::Config>::RuntimeOrigin>;
        /// Calls that may run as Root; everything else is refused by `dispatch_as_root`
        /// (Root bypasses the runtime's base call filter).
        type RootCallFilter: Contains<<Self as Config>::RuntimeCall>;
        /// Weights.
        type WeightInfo: WeightInfo;
    }

    /// Number of member approvals a motion needs (`1 ≤ t ≤ members`; 0 only while the
    /// administration is not configured).
    #[pallet::storage]
    pub type Threshold<T: Config> = StorageValue<_, u32, ValueQuery>;

    /// Motion duration in blocks; a member who has not voted by then counts as a no.
    #[pallet::storage]
    pub type MotionDuration<T: Config> = StorageValue<_, u32, ValueQuery, DefaultMotionDuration>;

    /// Default of [`MotionDuration`].
    #[pallet::type_value]
    pub fn DefaultMotionDuration() -> u32 {
        DEFAULT_MOTION_DURATION
    }

    #[pallet::genesis_config]
    pub struct GenesisConfig<T: Config> {
        /// Threshold over the council's genesis members. 0 with no members leaves the
        /// administration unset (SDK tooling default; the node refuses such a live chain).
        pub threshold: u32,
        /// Motion duration in blocks.
        pub motion_duration: u32,
        /// Unused type marker.
        #[serde(skip)]
        pub _marker: core::marker::PhantomData<T>,
    }

    impl<T: Config> Default for GenesisConfig<T> {
        fn default() -> Self {
            Self {
                threshold: 0,
                motion_duration: DEFAULT_MOTION_DURATION,
                _marker: core::marker::PhantomData,
            }
        }
    }

    #[pallet::genesis_build]
    impl<T: Config> BuildGenesisConfig for GenesisConfig<T> {
        fn build(&self) {
            // The council's genesis (members) is built first: it comes earlier in the runtime.
            let members = pallet_collective::Members::<T, CouncilInstance>::get().len();
            let valid = (self.threshold == 0 && members == 0)
                || Pallet::<T>::check_threshold(self.threshold, members).is_ok();
            if !valid || self.motion_duration == 0 {
                // Genesis is built once, off chain, from a chain spec: a bad parameter must
                // stop the chain from being created at all (spec "缺少成员的正式链创世").
                #[allow(clippy::panic)]
                {
                    panic!(
                        "invalid PoA admin genesis: threshold {} over {members} members, motion \
                         duration {}",
                        self.threshold, self.motion_duration
                    );
                }
            }
            Threshold::<T>::put(self.threshold);
            MotionDuration::<T>::put(self.motion_duration);
        }
    }

    #[pallet::event]
    #[pallet::generate_deposit(pub(super) fn deposit_event)]
    pub enum Event<T: Config> {
        /// A call ran as Root on behalf of the administration.
        DispatchedAsRoot {
            /// Its result.
            result: DispatchResult,
        },
        /// The threshold changed.
        ThresholdSet {
            /// New threshold.
            threshold: u32,
        },
        /// The members (and threshold) changed.
        MembersSet {
            /// New number of members.
            members: u32,
            /// New threshold.
            threshold: u32,
        },
    }

    #[pallet::error]
    pub enum Error<T> {
        /// The call may not run as Root.
        CallFiltered,
        /// The threshold is not between 1 and the number of members.
        InvalidThreshold,
        /// More members than the council supports.
        TooManyMembers,
        /// A member is listed twice.
        DuplicateMember,
    }

    #[pallet::call]
    impl<T: Config> Pallet<T> {
        /// Runs `call` as Root. Requires the administration.
        #[pallet::call_index(0)]
        #[pallet::weight({
            let info = call.get_dispatch_info();
            (
                <T as Config>::WeightInfo::dispatch_as_root().saturating_add(info.call_weight),
                info.class,
            )
        })]
        pub fn dispatch_as_root(
            origin: OriginFor<T>,
            call: Box<<T as Config>::RuntimeCall>,
        ) -> DispatchResult {
            T::AdminOrigin::ensure_origin(origin)?;
            ensure!(T::RootCallFilter::contains(&call), Error::<T>::CallFiltered);
            let result = call
                .dispatch_bypass_filter(RawOrigin::Root.into())
                .map(|_| ())
                .map_err(|e| e.error);
            Self::deposit_event(Event::DispatchedAsRoot { result });
            Ok(())
        }

        /// Changes the threshold to `threshold` (`1 ≤ threshold ≤ members`). Requires the
        /// administration.
        #[pallet::call_index(1)]
        #[pallet::weight(<T as Config>::WeightInfo::set_threshold())]
        pub fn set_threshold(origin: OriginFor<T>, threshold: u32) -> DispatchResult {
            T::AdminOrigin::ensure_origin(origin)?;
            let members = pallet_collective::Members::<T, CouncilInstance>::get().len();
            Self::check_threshold(threshold, members)?;
            Threshold::<T>::put(threshold);
            Self::deposit_event(Event::ThresholdSet { threshold });
            Ok(())
        }

        /// Replaces the members by `members` and the threshold by `threshold`
        /// (`1 ≤ threshold ≤ members`). Votes of removed members are dropped from open
        /// motions. Requires the administration.
        #[pallet::call_index(2)]
        #[pallet::weight({
            let max = <T as pallet_collective::Config<CouncilInstance>>::MaxMembers::get();
            let new = u32::try_from(members.len()).unwrap_or(u32::MAX).min(max);
            <T as Config>::WeightInfo::set_members(new).saturating_add(
                <T as pallet_collective::Config<CouncilInstance>>::WeightInfo::set_members(
                    max,
                    new,
                    <T as pallet_collective::Config<CouncilInstance>>::MaxProposals::get(),
                ),
            )
        })]
        pub fn set_members(
            origin: OriginFor<T>,
            members: Vec<T::AccountId>,
            threshold: u32,
        ) -> DispatchResult {
            T::AdminOrigin::ensure_origin(origin)?;
            let max = <T as pallet_collective::Config<CouncilInstance>>::MaxMembers::get();
            ensure!(
                u32::try_from(members.len()).is_ok_and(|n| n <= max),
                Error::<T>::TooManyMembers
            );
            let mut sorted = members;
            sorted.sort();
            let count = sorted.len();
            sorted.dedup();
            ensure!(sorted.len() == count, Error::<T>::DuplicateMember);
            Self::check_threshold(threshold, count)?;
            let old = pallet_collective::Members::<T, CouncilInstance>::get();
            <Collective<T> as ChangeMembers<T::AccountId>>::set_members_sorted(&sorted, &old);
            Threshold::<T>::put(threshold);
            Self::deposit_event(Event::MembersSet {
                members: count.saturated_into(),
                threshold,
            });
            Ok(())
        }
    }

    impl<T: Config> Pallet<T> {
        /// Checks `1 ≤ threshold ≤ members`.
        pub(crate) fn check_threshold(threshold: u32, members: usize) -> DispatchResult {
            let members = u32::try_from(members).unwrap_or(u32::MAX);
            ensure!(
                threshold >= 1 && threshold <= members,
                Error::<T>::InvalidThreshold
            );
            Ok(())
        }
    }

    /// [`MotionDuration`] as the council's `MotionDuration` parameter.
    pub struct MotionDurationOf<T>(core::marker::PhantomData<T>);

    impl<T: Config> Get<BlockNumberFor<T>> for MotionDurationOf<T> {
        fn get() -> BlockNumberFor<T> {
            MotionDuration::<T>::get().saturated_into()
        }
    }
}
