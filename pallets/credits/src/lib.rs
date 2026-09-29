//! # Transparent credits
//!
//! The α implementation of the unified credit interface (decision D20; plan §5.2;
//! m5-market-registry design D7):
//!
//! - A user escrows ATC with an active gateway: the escrow is held on the user's account and
//!   recorded in the (user, gateway) *channel*. Opening a channel fixes its **voucher key**: the
//!   user's registered public key at that moment.
//! - The user pays with **cumulative vouchers**: "the gateway may take up to X dollars from
//!   channel n", signed with the voucher key under `agentcoin/voucher/v1`. The chain keeps the
//!   amount already redeemed, so a gateway keeps only its latest voucher and replays pay nothing.
//! - Settlement redeems vouchers through [`Credit::redeem`] (no transaction does): the increment
//!   is converted at the reference rate rounding down and moved from the escrow to the payee;
//!   what the escrow cannot cover is reported as a shortfall, borne by the gateway.
//! - Withdrawing escrow and changing the voucher key take effect only after a delay (1 day on
//!   live chains), during which the gateway can still redeem earlier vouchers. A channel whose
//!   escrow is fully withdrawn is reset: its number increments and earlier vouchers are void.
//!
//! [`ac_primitives::market::voucher::check_voucher`] implements the rules for both on-chain
//! redemption and the off-chain check the `MarketApi` offers gateways.

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

use parity_scale_codec::{Decode, DecodeWithMemTracking, Encode, MaxEncodedLen};
use scale_info::TypeInfo;

/// Credit parameters, set at genesis.
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
#[serde(rename_all = "camelCase")]
pub struct CreditsParams {
    /// Blocks between requesting a withdrawal (or a key change) and it taking effect.
    pub withdrawal_delay: u64,
}

impl CreditsParams {
    /// Live-chain value: one day of one-second blocks.
    pub const LIVE: Self = Self {
        withdrawal_delay: 86_400,
    };
}

impl Default for CreditsParams {
    fn default() -> Self {
        Self::LIVE
    }
}

/// What the benchmarks need from the runtime.
#[cfg(feature = "runtime-benchmarks")]
pub trait BenchmarkHelper {
    /// Registers `key` as `who`'s account key.
    fn register_key(who: &sp_runtime::AccountId32, key: &ac_crypto::PqPublicKey);
    /// Makes `gateway` an active gateway.
    fn activate_gateway(gateway: &sp_runtime::AccountId32);
    /// Sets the reference rate to `rate` smallest units per dollar.
    fn set_rate(rate: u128);
}

// FRAME's `pallet` macro expands to code (genesis defaults, storage metadata, call decoding)
// that uses `expect` / `unreachable!` on our spans; hand-written code here uses neither except
// the documented genesis check.
#[allow(clippy::expect_used, clippy::unreachable)]
#[frame_support::pallet]
pub mod pallet {
    use super::{CreditsParams, WeightInfo};
    use ac_primitives::market::traits::{
        AccountKeys, Credit, GatewayLookup, PriceSource, Redemption,
    };
    use ac_primitives::market::voucher::{VoucherContext, check_voucher};
    use ac_primitives::market::{
        ChannelRecord, MicroUsd, SignedVoucher, VoucherCheck, VoucherError,
    };
    use frame_support::Identity;
    use frame_support::pallet_prelude::{
        BuildGenesisConfig, DispatchError, DispatchResult, IsType, OptionQuery, StorageDoubleMap,
        StorageValue, ValueQuery, ensure,
    };
    use frame_support::traits::fungible::{Inspect, InspectHold, Mutate, MutateHold};
    use frame_support::traits::tokens::{Fortitude, Precision, Restriction};
    use frame_system::pallet_prelude::{BlockNumberFor, OriginFor};
    use sp_core::H256;
    use sp_runtime::traits::Zero;
    use sp_runtime::{AccountId32, SaturatedConversion, Saturating};

    /// Balance of the native currency (smallest ATC units).
    pub type Balance = u128;

    /// A channel record of this runtime.
    pub type ChannelOf<T> = ChannelRecord<Balance, BlockNumberFor<T>>;

    #[pallet::pallet]
    pub struct Pallet<T>(_);

    #[pallet::config]
    pub trait Config: frame_system::Config<AccountId = AccountId32, Hash = H256> {
        /// The overarching event type.
        #[allow(deprecated)] // Kept explicit for compatibility with the SDK's test tooling.
        type RuntimeEvent: From<Event<Self>> + IsType<<Self as frame_system::Config>::RuntimeEvent>;
        /// The overarching hold reason.
        type RuntimeHoldReason: From<HoldReason>;
        /// The native currency; escrow is held with [`HoldReason::Escrow`].
        type Currency: Inspect<AccountId32, Balance = Balance>
            + Mutate<AccountId32>
            + InspectHold<AccountId32, Reason = Self::RuntimeHoldReason>
            + MutateHold<AccountId32, Reason = Self::RuntimeHoldReason>;
        /// Registered gateways.
        type Gateways: GatewayLookup<AccountId32>;
        /// Accounts' current keys.
        type Keys: AccountKeys<AccountId32>;
        /// The reference rate.
        type Price: PriceSource;
        /// Weights.
        type WeightInfo: WeightInfo;
        /// Registers keys and gateways and sets a rate for the benchmarks.
        #[cfg(feature = "runtime-benchmarks")]
        type BenchmarkHelper: super::BenchmarkHelper;
    }

    /// Why this pallet holds funds.
    #[pallet::composite_enum]
    pub enum HoldReason {
        /// Escrow of a credit channel.
        #[codec(index = 0)]
        Escrow,
    }

    /// Credit channels by (user, gateway). Account IDs are hashes, so `Identity` keys cannot be
    /// chosen to unbalance the trie.
    #[pallet::storage]
    pub type Channels<T: Config> = StorageDoubleMap<
        _,
        Identity,
        AccountId32,
        Identity,
        AccountId32,
        ChannelOf<T>,
        OptionQuery,
    >;

    /// Parameters set at genesis.
    #[pallet::storage]
    pub type Params<T: Config> = StorageValue<_, CreditsParams, ValueQuery>;

    #[pallet::genesis_config]
    pub struct GenesisConfig<T: Config> {
        /// Parameters.
        pub params: CreditsParams,
        /// Unused type marker.
        #[serde(skip)]
        pub _marker: core::marker::PhantomData<T>,
    }

    impl<T: Config> Default for GenesisConfig<T> {
        fn default() -> Self {
            Self {
                params: CreditsParams::LIVE,
                _marker: core::marker::PhantomData,
            }
        }
    }

    #[pallet::genesis_build]
    impl<T: Config> BuildGenesisConfig for GenesisConfig<T> {
        fn build(&self) {
            if self.params.withdrawal_delay == 0 {
                // Genesis is built once, off chain: a bad parameter must stop the chain from
                // being created at all.
                #[allow(clippy::panic)]
                {
                    panic!("invalid credits genesis parameters: {:?}", self.params);
                }
            }
            Params::<T>::put(self.params);
        }
    }

    #[pallet::event]
    #[pallet::generate_deposit(pub(super) fn deposit_event)]
    pub enum Event<T: Config> {
        /// Escrow was added to a channel.
        Deposited {
            /// The user.
            user: AccountId32,
            /// The gateway.
            gateway: AccountId32,
            /// Amount added.
            amount: Balance,
        },
        /// A withdrawal was requested.
        WithdrawalRequested {
            /// The user.
            user: AccountId32,
            /// The gateway.
            gateway: AccountId32,
            /// Total pending.
            amount: Balance,
            /// First block it can be withdrawn.
            unlock_at: BlockNumberFor<T>,
        },
        /// Escrow was released to the user.
        Withdrawn {
            /// The user.
            user: AccountId32,
            /// The gateway.
            gateway: AccountId32,
            /// Amount released.
            amount: Balance,
        },
        /// A channel was emptied and reset; earlier vouchers are void.
        ChannelReset {
            /// The user.
            user: AccountId32,
            /// The gateway.
            gateway: AccountId32,
            /// The new channel number.
            number: u32,
        },
        /// A voucher-key change was requested.
        KeyChangeRequested {
            /// The user.
            user: AccountId32,
            /// The gateway.
            gateway: AccountId32,
            /// Block the new key takes effect.
            effective_at: BlockNumberFor<T>,
        },
        /// A voucher was redeemed.
        Redeemed {
            /// The user.
            user: AccountId32,
            /// The gateway.
            gateway: AccountId32,
            /// Amount moved to the payee.
            paid: Balance,
            /// Value the escrow could not cover.
            shortfall: Balance,
        },
    }

    #[pallet::error]
    pub enum Error<T> {
        /// The gateway is not registered or not active.
        GatewayNotActive,
        /// The account has no registered key.
        NoKey,
        /// A zero amount.
        ZeroAmount,
        /// The account cannot cover the escrow.
        InsufficientBalance,
        /// The user has no channel with this gateway.
        NoChannel,
        /// More than the channel's escrow.
        AmountTooLarge,
        /// No withdrawal is pending.
        NoWithdrawal,
        /// The withdrawal delay has not passed yet.
        TooEarly,
        /// Voucher signed for another chain.
        WrongGenesis,
        /// Voucher made out to another gateway.
        WrongGateway,
        /// Voucher of an earlier channel number.
        WrongChannel,
        /// Voucher not signed with the channel's voucher key.
        WrongKey,
        /// Voucher signature does not verify.
        BadSignature,
        /// No reference rate.
        RateNotSet,
        /// The ATC amount overflows.
        PriceOverflow,
        /// The payment could not be moved to the payee.
        TransferFailed,
    }

    impl<T> From<VoucherError> for Error<T> {
        fn from(e: VoucherError) -> Self {
            match e {
                VoucherError::WrongGenesis => Self::WrongGenesis,
                VoucherError::WrongGateway => Self::WrongGateway,
                VoucherError::NoChannel => Self::NoChannel,
                VoucherError::WrongChannel => Self::WrongChannel,
                VoucherError::WrongKey => Self::WrongKey,
                VoucherError::BadSignature => Self::BadSignature,
                VoucherError::RateNotSet => Self::RateNotSet,
                _ => Self::PriceOverflow,
            }
        }
    }

    #[pallet::call]
    impl<T: Config> Pallet<T> {
        /// Escrows `amount` with `gateway`, opening the channel if needed.
        #[pallet::call_index(0)]
        #[pallet::weight(T::WeightInfo::deposit())]
        pub fn deposit(
            origin: OriginFor<T>,
            gateway: AccountId32,
            amount: Balance,
        ) -> DispatchResult {
            let user = frame_system::ensure_signed(origin)?;
            ensure!(amount > 0, Error::<T>::ZeroAmount);
            ensure!(
                T::Gateways::is_active(&gateway),
                Error::<T>::GatewayNotActive
            );
            let key = T::Keys::key_fingerprint(&user).ok_or(Error::<T>::NoKey)?;
            let mut ch = Self::current(&user, &gateway).unwrap_or(ChannelOf::<T> {
                escrow: 0,
                number: 0,
                redeemed: MicroUsd::ZERO,
                key,
                pending_withdrawal: None,
                pending_key: None,
            });
            // A fresh or reset channel takes the account's current key; an open one keeps its
            // voucher key so the gateway's vouchers stay valid.
            if ch.escrow == 0 && ch.redeemed == MicroUsd::ZERO && ch.pending_withdrawal.is_none() {
                ch.key = key;
            }
            T::Currency::hold(&HoldReason::Escrow.into(), &user, amount)
                .map_err(|_| Error::<T>::InsufficientBalance)?;
            ch.escrow = ch.escrow.saturating_add(amount);
            Channels::<T>::insert(&user, &gateway, ch);
            Self::deposit_event(Event::Deposited {
                user,
                gateway,
                amount,
            });
            Ok(())
        }

        /// Requests to withdraw `amount` after the delay; merges with a pending request and
        /// restarts the delay.
        #[pallet::call_index(1)]
        #[pallet::weight(T::WeightInfo::request_withdrawal())]
        pub fn request_withdrawal(
            origin: OriginFor<T>,
            gateway: AccountId32,
            amount: Balance,
        ) -> DispatchResult {
            let user = frame_system::ensure_signed(origin)?;
            ensure!(amount > 0, Error::<T>::ZeroAmount);
            let mut ch = Self::current(&user, &gateway).ok_or(Error::<T>::NoChannel)?;
            let total = ch
                .pending_withdrawal
                .map_or(0, |(a, _)| a)
                .saturating_add(amount);
            ensure!(total <= ch.escrow, Error::<T>::AmountTooLarge);
            let unlock_at = Self::after_delay();
            ch.pending_withdrawal = Some((total, unlock_at));
            Channels::<T>::insert(&user, &gateway, ch);
            Self::deposit_event(Event::WithdrawalRequested {
                user,
                gateway,
                amount: total,
                unlock_at,
            });
            Ok(())
        }

        /// Withdraws the pending amount (or what is left of the escrow) once the delay passed.
        /// A channel left empty is reset.
        #[pallet::call_index(2)]
        #[pallet::weight(T::WeightInfo::withdraw())]
        pub fn withdraw(origin: OriginFor<T>, gateway: AccountId32) -> DispatchResult {
            let user = frame_system::ensure_signed(origin)?;
            let mut ch = Self::current(&user, &gateway).ok_or(Error::<T>::NoChannel)?;
            let (amount, unlock_at) = ch.pending_withdrawal.ok_or(Error::<T>::NoWithdrawal)?;
            ensure!(
                frame_system::Pallet::<T>::block_number() >= unlock_at,
                Error::<T>::TooEarly
            );
            let amount = amount.min(ch.escrow);
            T::Currency::release(&HoldReason::Escrow.into(), &user, amount, Precision::Exact)?;
            ch.escrow = ch.escrow.saturating_sub(amount);
            ch.pending_withdrawal = None;
            let reset = ch.escrow == 0;
            if reset {
                ch.number = ch.number.saturating_add(1);
                ch.redeemed = MicroUsd::ZERO;
                ch.pending_key = None;
            }
            let number = ch.number;
            Channels::<T>::insert(&user, &gateway, ch);
            Self::deposit_event(Event::Withdrawn {
                user: user.clone(),
                gateway: gateway.clone(),
                amount,
            });
            if reset {
                Self::deposit_event(Event::ChannelReset {
                    user,
                    gateway,
                    number,
                });
            }
            Ok(())
        }

        /// Requests that the channel's voucher key become the account's current key after the
        /// delay.
        #[pallet::call_index(3)]
        #[pallet::weight(T::WeightInfo::request_key_change())]
        pub fn request_key_change(origin: OriginFor<T>, gateway: AccountId32) -> DispatchResult {
            let user = frame_system::ensure_signed(origin)?;
            let key = T::Keys::key_fingerprint(&user).ok_or(Error::<T>::NoKey)?;
            let mut ch = Self::current(&user, &gateway).ok_or(Error::<T>::NoChannel)?;
            let effective_at = Self::after_delay();
            ch.pending_key = Some((key, effective_at));
            Channels::<T>::insert(&user, &gateway, ch);
            Self::deposit_event(Event::KeyChangeRequested {
                user,
                gateway,
                effective_at,
            });
            Ok(())
        }
    }

    impl<T: Config> Pallet<T> {
        fn after_delay() -> BlockNumberFor<T> {
            frame_system::Pallet::<T>::block_number()
                .saturating_add(Params::<T>::get().withdrawal_delay.saturated_into())
        }

        /// The channel with a due key change applied.
        fn current(user: &AccountId32, gateway: &AccountId32) -> Option<ChannelOf<T>> {
            let mut ch = Channels::<T>::get(user, gateway)?;
            let now = frame_system::Pallet::<T>::block_number();
            if let Some((key, at)) = ch.pending_key
                && now >= at
            {
                ch.key = key;
                ch.pending_key = None;
            }
            Some(ch)
        }

        /// `user`'s channel with `gateway`, as stored.
        #[must_use]
        pub fn channel(user: &AccountId32, gateway: &AccountId32) -> Option<ChannelOf<T>> {
            Channels::<T>::get(user, gateway)
        }

        /// The genesis hash vouchers must carry.
        #[must_use]
        pub fn genesis() -> H256 {
            frame_system::Pallet::<T>::block_hash(BlockNumberFor::<T>::zero())
        }

        /// Checks `voucher` exactly as [`Credit::redeem`] would, for the gateway it names.
        ///
        /// # Errors
        ///
        /// Why the voucher would be refused.
        pub fn check(voucher: &SignedVoucher) -> Result<VoucherCheck, VoucherError> {
            let body = &voucher.body;
            let now = frame_system::Pallet::<T>::block_number();
            let view = Channels::<T>::get(&body.user, &body.gateway).map(|c| c.view_at(now));
            let genesis = Self::genesis();
            check_voucher(
                voucher,
                &VoucherContext {
                    genesis: &genesis,
                    gateway: &body.gateway,
                    channel: view.as_ref(),
                    rate: T::Price::atc_per_usd(),
                },
            )
        }
    }

    impl<T: Config> Credit<AccountId32, Balance> for Pallet<T> {
        type Voucher = SignedVoucher;

        fn redeem(
            gateway: &AccountId32,
            voucher: &SignedVoucher,
            payee: &AccountId32,
        ) -> Result<Redemption<Balance>, DispatchError> {
            let user = &voucher.body.user;
            // Settlement pays into existing accounts only (design D7): a redemption never
            // creates one.
            ensure!(
                frame_system::Pallet::<T>::account_exists(payee),
                Error::<T>::TransferFailed
            );
            let mut ch = Self::current(user, gateway).ok_or(Error::<T>::NoChannel)?;
            let now = frame_system::Pallet::<T>::block_number();
            let view = ch.view_at(now);
            let genesis = Self::genesis();
            let checked = check_voucher(
                voucher,
                &VoucherContext {
                    genesis: &genesis,
                    gateway,
                    channel: Some(&view),
                    rate: T::Price::atc_per_usd(),
                },
            )
            .map_err(Error::<T>::from)?;
            if checked.increment == MicroUsd::ZERO {
                return Ok(Redemption::default());
            }
            let paid = checked.increment_atc.min(ch.escrow);
            let shortfall = checked.increment_atc.saturating_sub(paid);
            if paid > 0 {
                T::Currency::transfer_on_hold(
                    &HoldReason::Escrow.into(),
                    user,
                    payee,
                    paid,
                    Precision::Exact,
                    Restriction::Free,
                    Fortitude::Polite,
                )
                .map_err(|_| Error::<T>::TransferFailed)?;
            }
            ch.escrow = ch.escrow.saturating_sub(paid);
            ch.redeemed = voucher.body.cumulative;
            Channels::<T>::insert(user, gateway, ch);
            Self::deposit_event(Event::Redeemed {
                user: user.clone(),
                gateway: gateway.clone(),
                paid,
                shortfall,
            });
            Ok(Redemption { paid, shortfall })
        }
    }
}
