//! # Treasury (dual)
//!
//! The treasury of AgentCoin (plan §5.1; decisions D18, D42; design D6 of `m3-economics`): three
//! keyless accounts derived from fixed `PalletId`s, funded only by emission settlement.
//!
//! - **Community grants** (`ac/trcom`) receives [`CommunityShare`] of the proportional treasury
//!   share (default 40%) and is spent by the chain's administration with [`Pallet::spend`].
//! - **Holder treasury** (`ac/trhld`) receives the rest and is **locked** until on-chain holder
//!   voting exists (M8): this pallet has no call that moves funds out of it, and the runtime's
//!   call filter rejects forced transfers from it.
//! - **Floor** (`ac/trflr`) receives the top-up to 5% of the scheduled amount. Top-ups are
//!   grouped into batches of [`Config::BatchBlocks`]; each batch vests linearly over
//!   [`Config::VestingBlocks`] from its end, and [`Pallet::spend_floor`] can only spend the
//!   vested part, for an audit or for cold start.
//!
//! Spending is a plain transfer: the treasury never mints or burns.

#![cfg_attr(not(feature = "std"), no_std)]

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

use frame_support::PalletId;
use parity_scale_codec::{Decode, DecodeWithMemTracking, Encode, MaxEncodedLen};
use scale_info::TypeInfo;

/// `PalletId` of the community-grants account. Published: never change it.
pub const COMMUNITY_PALLET_ID: PalletId = PalletId(*b"ac/trcom");
/// `PalletId` of the holder treasury. Published: never change it.
pub const HOLDER_PALLET_ID: PalletId = PalletId(*b"ac/trhld");
/// `PalletId` of the treasury floor. Published: never change it.
pub const FLOOR_PALLET_ID: PalletId = PalletId(*b"ac/trflr");

/// Default community share of the proportional treasury part, in basis points (decision D42).
pub const DEFAULT_COMMUNITY_SHARE: u32 = 4_000;

/// What a floor spend is for (spec economics/treasury "保底支出注明用途"). Encoded in calls
/// and events: the discriminants never change.
#[derive(
    Clone,
    Copy,
    PartialEq,
    Eq,
    Debug,
    Encode,
    Decode,
    DecodeWithMemTracking,
    MaxEncodedLen,
    TypeInfo,
)]
pub enum FloorPurpose {
    /// Security audits.
    Audit = 0,
    /// Cold start of the network.
    ColdStart = 1,
}

/// Treasury account a spend comes from.
#[derive(
    Clone,
    Copy,
    PartialEq,
    Eq,
    Debug,
    Encode,
    Decode,
    DecodeWithMemTracking,
    MaxEncodedLen,
    TypeInfo,
)]
pub enum SpendSource {
    /// Community grants.
    Community = 0,
    /// Treasury floor.
    Floor = 1,
}

/// Floor top-ups of one batch, vesting from `end`.
#[derive(
    Clone,
    Copy,
    PartialEq,
    Eq,
    Debug,
    Default,
    Encode,
    Decode,
    DecodeWithMemTracking,
    MaxEncodedLen,
    TypeInfo,
)]
pub struct FloorBatch {
    /// Last block of the batch; vesting starts here.
    pub end: u64,
    /// Amount deposited during the batch.
    pub amount: u128,
}

// FRAME's `pallet` macro expands to code (genesis defaults, storage metadata) that uses
// `expect` / `unreachable!` on our spans; hand-written code here uses neither except the
// documented genesis check.
#[allow(clippy::expect_used, clippy::unreachable)]
#[frame_support::pallet]
pub mod pallet {
    use super::{
        COMMUNITY_PALLET_ID, DEFAULT_COMMUNITY_SHARE, FLOOR_PALLET_ID, FloorBatch, FloorPurpose,
        HOLDER_PALLET_ID, SpendSource, WeightInfo,
    };
    use ac_primitives::emission::{BPS, TreasuryDeposit, split_treasury, vested_over};
    use frame_support::pallet_prelude::{
        BoundedVec, BuildGenesisConfig, DispatchResult, EnsureOrigin, Get, IsType, StorageValue,
        ValueQuery, ensure,
    };
    use frame_support::traits::fungible::{Inspect, Mutate};
    use frame_support::traits::tokens::Preservation;
    use frame_system::pallet_prelude::OriginFor;
    use sp_runtime::SaturatedConversion;
    use sp_runtime::traits::AccountIdConversion;

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
        type Currency: Mutate<Self::AccountId, Balance = Balance>;
        /// The chain's administration (the PoA multisig).
        type AdminOrigin: EnsureOrigin<Self::RuntimeOrigin>;
        /// Length of a floor batch in blocks (30 days on live chains).
        #[pallet::constant]
        type BatchBlocks: Get<u64>;
        /// Vesting period of a floor batch in blocks (2 years on live chains).
        #[pallet::constant]
        type VestingBlocks: Get<u64>;
        /// Maximum number of floor batches still vesting: `VestingBlocks / BatchBlocks + 2`.
        #[pallet::constant]
        type MaxBatches: Get<u32>;
        /// Weights.
        type WeightInfo: WeightInfo;
    }

    /// Share of the proportional treasury part going to community grants, in basis points;
    /// the rest (and the rounding remainder) goes to the holder treasury. Genesis parameter.
    #[pallet::storage]
    pub type CommunityShare<T: Config> = StorageValue<_, u32, ValueQuery, DefaultCommunityShare>;

    /// Default of [`CommunityShare`].
    #[pallet::type_value]
    pub fn DefaultCommunityShare() -> u32 {
        DEFAULT_COMMUNITY_SHARE
    }

    /// Floor batches that are still vesting, oldest first.
    #[pallet::storage]
    pub type FloorBatches<T: Config> =
        StorageValue<_, BoundedVec<FloorBatch, T::MaxBatches>, ValueQuery>;

    /// Total of the floor batches that have fully vested and were removed from
    /// [`FloorBatches`].
    #[pallet::storage]
    pub type FloorMatured<T: Config> = StorageValue<_, Balance, ValueQuery>;

    /// Total spent from the floor.
    #[pallet::storage]
    pub type FloorSpent<T: Config> = StorageValue<_, Balance, ValueQuery>;

    #[pallet::genesis_config]
    pub struct GenesisConfig<T: Config> {
        /// Community share of the proportional part in basis points (at most 10,000).
        pub community_share: u32,
        /// Unused type marker.
        #[serde(skip)]
        pub _marker: core::marker::PhantomData<T>,
    }

    impl<T: Config> Default for GenesisConfig<T> {
        fn default() -> Self {
            Self {
                community_share: DEFAULT_COMMUNITY_SHARE,
                _marker: core::marker::PhantomData,
            }
        }
    }

    #[pallet::genesis_build]
    impl<T: Config> BuildGenesisConfig for GenesisConfig<T> {
        fn build(&self) {
            if u128::from(self.community_share) > BPS {
                // Genesis is built once, off chain, from a chain spec: a bad parameter must
                // stop the chain from being created at all.
                #[allow(clippy::panic)]
                {
                    panic!(
                        "invalid treasury genesis: community share {} above 10000 bps",
                        self.community_share
                    );
                }
            }
            CommunityShare::<T>::put(self.community_share);
        }
    }

    #[pallet::event]
    #[pallet::generate_deposit(pub(super) fn deposit_event)]
    pub enum Event<T: Config> {
        /// A floor top-up was recorded in a vesting batch.
        FloorDeposited {
            /// Amount deposited.
            amount: Balance,
            /// End of its batch, where vesting starts.
            batch_end: u64,
        },
        /// Treasury funds were spent.
        Spent {
            /// Source account.
            source: SpendSource,
            /// Recipient.
            to: T::AccountId,
            /// Amount transferred.
            amount: Balance,
            /// Purpose, for floor spends.
            purpose: Option<FloorPurpose>,
        },
    }

    #[pallet::error]
    pub enum Error<T> {
        /// The amount exceeds the vested, unspent part of the floor.
        NotVested,
    }

    #[pallet::call]
    impl<T: Config> Pallet<T> {
        /// Transfers `amount` from community grants to `to`. Requires the administration.
        #[pallet::call_index(0)]
        #[pallet::weight(T::WeightInfo::spend())]
        pub fn spend(origin: OriginFor<T>, to: T::AccountId, amount: Balance) -> DispatchResult {
            T::AdminOrigin::ensure_origin(origin)?;
            T::Currency::transfer(
                &Self::community_account(),
                &to,
                amount,
                Preservation::Expendable,
            )?;
            Self::deposit_event(Event::Spent {
                source: SpendSource::Community,
                to,
                amount,
                purpose: None,
            });
            Ok(())
        }

        /// Transfers `amount` of the vested floor to `to` for `purpose`. Requires the
        /// administration.
        #[pallet::call_index(1)]
        #[pallet::weight(T::WeightInfo::spend_floor())]
        pub fn spend_floor(
            origin: OriginFor<T>,
            purpose: FloorPurpose,
            to: T::AccountId,
            amount: Balance,
        ) -> DispatchResult {
            T::AdminOrigin::ensure_origin(origin)?;
            ensure!(amount <= Self::floor_spendable(), Error::<T>::NotVested);
            T::Currency::transfer(
                &Self::floor_account(),
                &to,
                amount,
                Preservation::Expendable,
            )?;
            FloorSpent::<T>::mutate(|s| *s = s.saturating_add(amount));
            Self::deposit_event(Event::Spent {
                source: SpendSource::Floor,
                to,
                amount,
                purpose: Some(purpose),
            });
            Ok(())
        }
    }

    impl<T: Config> Pallet<T> {
        /// Community-grants account.
        pub fn community_account() -> T::AccountId {
            COMMUNITY_PALLET_ID.into_account_truncating()
        }

        /// Holder-treasury account.
        pub fn holder_account() -> T::AccountId {
            HOLDER_PALLET_ID.into_account_truncating()
        }

        /// Floor account.
        pub fn floor_account() -> T::AccountId {
            FLOOR_PALLET_ID.into_account_truncating()
        }

        /// Balance of `who`.
        pub fn balance(who: &T::AccountId) -> Balance {
            T::Currency::balance(who)
        }

        fn now() -> u64 {
            frame_system::Pallet::<T>::block_number().saturated_into()
        }

        /// Vested floor at the current block, spent or not.
        pub fn floor_vested() -> Balance {
            let now = Self::now();
            let vesting = T::VestingBlocks::get();
            FloorBatches::<T>::get()
                .iter()
                .fold(FloorMatured::<T>::get(), |acc, b| {
                    acc.saturating_add(vested_over(b.amount, b.end, now, vesting))
                })
        }

        /// Floor that can be spent now: vested minus spent.
        pub fn floor_spendable() -> Balance {
            Self::floor_vested().saturating_sub(FloorSpent::<T>::get())
        }

        /// End of the batch containing block `now`: the next multiple of the batch length.
        fn batch_end(now: u64) -> u64 {
            let len = T::BatchBlocks::get();
            if len == 0 {
                return now;
            }
            now.div_ceil(len).saturating_mul(len)
        }

        /// Records `amount` minted to the floor at the current block.
        pub(crate) fn record_floor(amount: Balance) {
            let now = Self::now();
            let end = Self::batch_end(now);
            let vesting = T::VestingBlocks::get();
            let mut batches = FloorBatches::<T>::get();
            // Fully vested batches leave the list; their total stays counted as vested.
            let mut matured = 0u128;
            batches.retain(|b| {
                let done = now.saturating_sub(b.end) >= vesting && now >= b.end;
                if done {
                    matured = matured.saturating_add(b.amount);
                }
                !done
            });
            FloorMatured::<T>::mutate(|m| *m = m.saturating_add(matured));
            match batches.last_mut() {
                Some(last) if last.end == end => last.amount = last.amount.saturating_add(amount),
                _ => {
                    if let Err(batch) = batches.try_push(FloorBatch { end, amount }) {
                        // Cannot happen with `MaxBatches` as documented; if it does, merge into
                        // the newest batch and move its end later, which only delays vesting.
                        if let Some(last) = batches.last_mut() {
                            last.amount = last.amount.saturating_add(batch.amount);
                            last.end = last.end.max(batch.end);
                        }
                    }
                }
            }
            FloorBatches::<T>::put(batches);
            Self::deposit_event(Event::FloorDeposited {
                amount,
                batch_end: end,
            });
        }
    }

    impl<T: Config> TreasuryDeposit<T::AccountId> for Pallet<T> {
        fn recipients(proportional: u128, floor_topup: u128) -> [(T::AccountId, u128); 3] {
            let (community, holder) =
                split_treasury(proportional, u128::from(CommunityShare::<T>::get()));
            [
                (Self::community_account(), community),
                (Self::holder_account(), holder),
                (Self::floor_account(), floor_topup),
            ]
        }

        fn floor_minted(amount: u128) {
            Self::record_floor(amount);
        }
    }
}
