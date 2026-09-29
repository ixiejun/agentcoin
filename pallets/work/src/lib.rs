//! # Work settlement
//!
//! Settles paid inference (MVP plan §5.4, refine / accumulate; spec `market/work-settlement`;
//! m5-work-settlement design D4–D8):
//!
//! - A gateway submits a **work report**: the Merkle root of its receipts (which stay off chain),
//!   per-(provider, model) totals, and the vouchers paying for them. The vouchers are redeemed
//!   through the `Credit` interface (D20) into the gateway's account; the totals must equal the
//!   dollars redeemed exactly.
//! - The settled amount `G` (redeemed plus any shortfall, which the gateway covers) is split:
//!   `b` burned through `Emission`, the gateway's fee, and the providers' shares in proportion to
//!   their dollars. Shares and fee stay **held on the gateway's account** until the challenge
//!   period ends; each provider earns market work `k × G_i`.
//! - A report submitted in epoch `s` matures in `s + C`. Its work is verified market work of that
//!   epoch; `Emission` mints the market share to this pallet's pot and reports it back
//!   (`MarketPayout`). Anyone can then **claim** for an account: shares move from the gateways,
//!   fees are released, the emission share is paid from the pot.
//! - A provider jailed before its work is settled (the audit, M6; nothing reaches it in M5) loses
//!   that work and its shares, which are burned when claimed.

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

/// What the benchmarks need from the runtime.
#[cfg(feature = "runtime-benchmarks")]
pub trait BenchmarkHelper {
    /// Makes `gateway` a registered gateway with funds and sets a reference rate.
    fn prepare_gateway(gateway: &sp_runtime::AccountId32);
    /// Makes `provider` able to settle `model`.
    fn prepare_provider(provider: &sp_runtime::AccountId32, model: &ac_primitives::market::ModelId);
    /// `n` vouchers for `gateway` from users `first..first + n` (fresh ones), each with escrow
    /// and a cumulative amount of `micro_usd`.
    fn vouchers(
        gateway: &sp_runtime::AccountId32,
        first: u32,
        n: u32,
        micro_usd: u128,
    ) -> alloc::vec::Vec<ac_primitives::market::SignedVoucher>;
    /// Moves the emission epoch forward to `epoch`.
    fn set_epoch(epoch: ac_primitives::emission::EpochIndex);
}

// FRAME's `pallet` macro expands to code (genesis defaults, storage metadata, call decoding)
// that uses `expect` / `unreachable!` on our spans; hand-written code here uses neither except
// the documented genesis check.
#[allow(clippy::expect_used, clippy::unreachable)]
#[frame_support::pallet]
pub mod pallet {
    use super::WeightInfo;
    use ac_primitives::emission::{EpochIndex, EpochIndexSource, MarketPayout, WorkSource};
    use ac_primitives::market::traits::{
        Credit as CreditInterface, GatewayLookup, OnJail, PriceSource, ProviderLookup,
    };
    use ac_primitives::market::work::{MAX_REPORT_ENTRIES, MAX_REPORT_VOUCHERS, allocate};
    use ac_primitives::market::{
        EpochWork, Held, LifetimeWork, MicroUsd, ProviderWork, ReportEntry, ReportLine,
        ReportRecord, SignedVoucher, WorkParams,
    };
    use alloc::collections::{BTreeMap, BTreeSet};
    use alloc::vec::Vec;
    use frame_support::pallet_prelude::{
        BuildGenesisConfig, ConstU32, DispatchResult, IsType, NMapKey, OptionQuery,
        StorageDoubleMap, StorageMap, StorageNMap, StorageValue, ValueQuery, ensure,
    };
    use frame_support::traits::fungible::{
        Balanced, BalancedHold, Credit, Inspect, InspectHold, Mutate, MutateHold,
    };
    use frame_support::traits::tokens::{Fortitude, Precision, Preservation, Restriction};
    use frame_support::traits::{Get, OnUnbalanced};
    use frame_support::{Blake2_128Concat, BoundedVec, PalletId, Twox64Concat};
    use frame_system::pallet_prelude::OriginFor;
    use sp_runtime::helpers_128bit::multiply_by_rational_with_rounding;
    use sp_runtime::traits::AccountIdConversion;
    use sp_runtime::{AccountId32, Rounding};

    /// Balance of the native currency (smallest ATC units).
    pub type Balance = u128;

    /// A stored report.
    pub type ReportOf = ReportRecord<AccountId32, Balance>;

    #[pallet::pallet]
    pub struct Pallet<T>(_);

    #[pallet::config]
    pub trait Config: frame_system::Config<AccountId = AccountId32> {
        /// The overarching event type.
        #[allow(deprecated)] // Kept explicit for compatibility with the SDK's test tooling.
        type RuntimeEvent: From<Event<Self>> + IsType<<Self as frame_system::Config>::RuntimeEvent>;
        /// The overarching hold reason.
        type RuntimeHoldReason: From<HoldReason>;
        /// The native currency; pending fees are held with [`HoldReason::Pending`].
        type Currency: Inspect<AccountId32, Balance = Balance>
            + Mutate<AccountId32>
            + Balanced<AccountId32>
            + InspectHold<AccountId32, Reason = Self::RuntimeHoldReason>
            + MutateHold<AccountId32, Reason = Self::RuntimeHoldReason>
            + BalancedHold<AccountId32, Reason = Self::RuntimeHoldReason>;
        /// The credit interface vouchers are redeemed through (D20).
        type Credit: CreditInterface<AccountId32, Balance, Voucher = SignedVoucher>;
        /// Registered gateways and their fees.
        type Gateways: GatewayLookup<AccountId32>;
        /// Which providers can settle which models.
        type Providers: ProviderLookup<AccountId32>;
        /// The reference rate (reports need one).
        type Price: PriceSource;
        /// The current emission epoch.
        type Epochs: EpochIndexSource;
        /// Where burned amounts go; the runtime burns them through `Emission`.
        type Burn: OnUnbalanced<Credit<AccountId32, Self::Currency>>;
        /// Derives the account market emission is minted to.
        #[pallet::constant]
        type PalletId: Get<PalletId>;
        /// Weights.
        type WeightInfo: WeightInfo;
        /// Prepares gateways, providers and vouchers for the benchmarks.
        #[cfg(feature = "runtime-benchmarks")]
        type BenchmarkHelper: super::BenchmarkHelper;
    }

    /// Why this pallet holds funds.
    #[pallet::composite_enum]
    pub enum HoldReason {
        /// Provider shares and gateway fees waiting for the challenge period to end, held on
        /// the gateway's account.
        Pending,
    }

    /// Settlement parameters.
    #[pallet::storage]
    pub type Params<T: Config> = StorageValue<_, WorkParams, ValueQuery, ParamsDefault>;

    /// Default parameters: the live values.
    pub struct ParamsDefault;
    impl Get<WorkParams> for ParamsDefault {
        fn get() -> WorkParams {
            WorkParams::LIVE
        }
    }

    /// Identifier of the next report.
    #[pallet::storage]
    pub type NextReportId<T: Config> = StorageValue<_, u64, ValueQuery>;

    /// Oldest report not yet pruned.
    #[pallet::storage]
    pub type OldestReport<T: Config> = StorageValue<_, u64, ValueQuery>;

    /// Accepted reports within their retention period.
    #[pallet::storage]
    pub type Reports<T: Config> = StorageMap<_, Twox64Concat, u64, ReportOf, OptionQuery>;

    /// What gateway `g` holds for account `who`, maturing in epoch `e`: `(who, e, g)`.
    #[pallet::storage]
    pub type HeldFor<T: Config> = StorageNMap<
        _,
        (
            NMapKey<Blake2_128Concat, AccountId32>,
            NMapKey<Twox64Concat, EpochIndex>,
            NMapKey<Blake2_128Concat, AccountId32>,
        ),
        Held<Balance>,
        OptionQuery,
    >;

    /// A provider's market work maturing in an epoch.
    #[pallet::storage]
    pub type WorkOf<T: Config> = StorageDoubleMap<
        _,
        Blake2_128Concat,
        AccountId32,
        Twox64Concat,
        EpochIndex,
        ProviderWork,
        OptionQuery,
    >;

    /// Verified market work of an epoch and, once settled, its market emission.
    #[pallet::storage]
    pub type EpochWorks<T: Config> =
        StorageMap<_, Twox64Concat, EpochIndex, EpochWork<Balance>, ValueQuery>;

    /// Market emission in the pot not yet claimed.
    #[pallet::storage]
    pub type Unclaimed<T: Config> = StorageValue<_, Balance, ValueQuery>;

    /// Lifetime work of each account.
    #[pallet::storage]
    pub type Lifetime<T: Config> =
        StorageMap<_, Blake2_128Concat, AccountId32, LifetimeWork<Balance>, ValueQuery>;

    #[pallet::genesis_config]
    #[derive(frame_support::DefaultNoBound)]
    pub struct GenesisConfig<T: Config> {
        /// Settlement parameters.
        pub params: WorkParams,
        /// Marker.
        #[serde(skip)]
        pub _config: core::marker::PhantomData<T>,
    }

    #[pallet::genesis_build]
    impl<T: Config> BuildGenesisConfig for GenesisConfig<T> {
        fn build(&self) {
            // Documented genesis check: parameters out of bounds make the chain spec invalid.
            self.params
                .validate()
                .expect("work parameters out of bounds");
            Params::<T>::put(self.params);
            // The pot exists from genesis without holding any ATC (no premine, D9).
            frame_system::Pallet::<T>::inc_providers(&Pallet::<T>::pot());
        }
    }

    #[pallet::event]
    #[pallet::generate_deposit(pub(super) fn deposit_event)]
    pub enum Event<T: Config> {
        /// A work report was accepted.
        ReportAccepted {
            /// Its identifier.
            id: u64,
            /// The gateway.
            gateway: AccountId32,
            /// Merkle root of its receipts.
            root: [u8; 32],
            /// `G`.
            settled: Balance,
            /// Burned now.
            burned: Balance,
            /// Epoch whose settlement makes it claimable.
            matures: EpochIndex,
        },
        /// An epoch's market emission was received.
        MarketSettled {
            /// The epoch.
            epoch: EpochIndex,
            /// Emission to distribute.
            market: Balance,
            /// Verified market work it is distributed over.
            work: u128,
        },
        /// Payments were claimed for an account.
        Claimed {
            /// The account.
            who: AccountId32,
            /// Shares and fees paid.
            fees: Balance,
            /// Market emission paid.
            emission: Balance,
            /// Voided shares burned.
            burned: Balance,
        },
        /// A jailed provider's unsettled work was voided.
        PendingVoided {
            /// The provider.
            who: AccountId32,
            /// Maturity epoch.
            epoch: EpochIndex,
            /// Work removed.
            work: u128,
        },
    }

    #[pallet::error]
    pub enum Error<T> {
        /// The submitter is not a registered gateway.
        NotGateway,
        /// A report needs at least one entry.
        NoEntries,
        /// A report needs at least one voucher.
        NoVouchers,
        /// Fewer receipts than entries.
        TooFewReceipts,
        /// A job kind that cannot be settled yet.
        UnsupportedJobKind,
        /// An entry of zero dollars.
        ZeroAmount,
        /// The same (provider, model) twice.
        DuplicateEntry,
        /// The provider is unregistered, jailed or does not list the model.
        ProviderCannotSettle,
        /// Two vouchers of the same channel.
        DuplicateChannel,
        /// No reference rate is set.
        RateNotSet,
        /// The entries' dollars differ from the dollars the vouchers redeemed.
        TotalsMismatch,
        /// The gateway's free balance cannot pay the burn and hold the shares and fee.
        ShortfallNotCovered,
        /// An amount overflows.
        Overflow,
    }

    #[pallet::call]
    impl<T: Config> Pallet<T> {
        /// Submits a work report (spec "工作报告的提交").
        #[pallet::call_index(0)]
        #[pallet::weight(T::WeightInfo::submit_report(
            u32::try_from(vouchers.len()).unwrap_or(MAX_REPORT_VOUCHERS),
            u32::try_from(entries.len()).unwrap_or(MAX_REPORT_ENTRIES),
        ))]
        pub fn submit_report(
            origin: OriginFor<T>,
            root: [u8; 32],
            receipt_count: u32,
            entries: BoundedVec<ReportEntry<AccountId32>, ConstU32<MAX_REPORT_ENTRIES>>,
            vouchers: BoundedVec<SignedVoucher, ConstU32<MAX_REPORT_VOUCHERS>>,
        ) -> DispatchResult {
            let gateway = frame_system::ensure_signed(origin)?;
            Self::do_submit(gateway, root, receipt_count, entries, vouchers)
        }

        /// Claims matured payments of `who` for the given (epoch, gateway) items (spec "领取").
        #[pallet::call_index(1)]
        #[pallet::weight(T::WeightInfo::claim(u32::try_from(items.len()).unwrap_or(64)))]
        pub fn claim(
            origin: OriginFor<T>,
            who: AccountId32,
            items: BoundedVec<(EpochIndex, AccountId32), ConstU32<64>>,
        ) -> DispatchResult {
            frame_system::ensure_signed(origin)?;
            Self::do_claim(who, items)
        }
    }

    /// `value × num / den` rounded down; zero when `den` is zero.
    fn part(value: u128, num: u128, den: u128) -> u128 {
        if den == 0 {
            return 0;
        }
        multiply_by_rational_with_rounding(value, num, den, Rounding::Down).unwrap_or(0)
    }

    impl<T: Config> Pallet<T> {
        /// The account market emission is minted to.
        pub fn pot() -> AccountId32 {
            T::PalletId::get().into_account_truncating()
        }

        fn do_submit(
            gateway: AccountId32,
            root: [u8; 32],
            receipt_count: u32,
            entries: BoundedVec<ReportEntry<AccountId32>, ConstU32<MAX_REPORT_ENTRIES>>,
            vouchers: BoundedVec<SignedVoucher, ConstU32<MAX_REPORT_VOUCHERS>>,
        ) -> DispatchResult {
            ensure!(T::Gateways::is_registered(&gateway), Error::<T>::NotGateway);
            let fee_bps = T::Gateways::fee_bps(&gateway).ok_or(Error::<T>::NotGateway)?;
            ensure!(!entries.is_empty(), Error::<T>::NoEntries);
            ensure!(!vouchers.is_empty(), Error::<T>::NoVouchers);
            ensure!(
                usize::try_from(receipt_count).unwrap_or(usize::MAX) >= entries.len(),
                Error::<T>::TooFewReceipts
            );
            ensure!(T::Price::atc_per_usd().is_some(), Error::<T>::RateNotSet);
            let mut keys = BTreeSet::new();
            for e in &entries {
                ensure!(e.kind.is_supported(), Error::<T>::UnsupportedJobKind);
                ensure!(e.usd != MicroUsd::ZERO, Error::<T>::ZeroAmount);
                ensure!(
                    keys.insert((e.provider.clone(), e.model)),
                    Error::<T>::DuplicateEntry
                );
                ensure!(
                    T::Providers::can_settle(&e.provider, &e.model),
                    Error::<T>::ProviderCannotSettle
                );
            }
            let mut users = BTreeSet::new();
            for v in &vouchers {
                ensure!(
                    users.insert(v.body.user.clone()),
                    Error::<T>::DuplicateChannel
                );
            }

            // Redeem into the gateway's own account (it is staked, so it exists).
            let (mut paid, mut shortfall, mut redeemed) = (0u128, 0u128, 0u128);
            for v in &vouchers {
                let r = T::Credit::redeem(&gateway, v, &gateway)?;
                paid = paid.checked_add(r.paid).ok_or(Error::<T>::Overflow)?;
                shortfall = shortfall
                    .checked_add(r.shortfall)
                    .ok_or(Error::<T>::Overflow)?;
                redeemed = redeemed.checked_add(r.usd.0).ok_or(Error::<T>::Overflow)?;
            }
            let usd: Vec<MicroUsd> = entries.iter().map(|e| e.usd).collect();
            let total = usd
                .iter()
                .try_fold(0u128, |a, u| a.checked_add(u.0))
                .ok_or(Error::<T>::Overflow)?;
            ensure!(total == redeemed, Error::<T>::TotalsMismatch);

            let g = paid.checked_add(shortfall).ok_or(Error::<T>::Overflow)?;
            let params = Params::<T>::get();
            let alloc = allocate(
                g,
                params.burn_bps,
                u32::from(fee_bps),
                params.emission_bps,
                &usd,
            )
            .map_err(|_| Error::<T>::Overflow)?;

            // Pay the allocation from the gateway's free balance: this is also where the gateway
            // covers the shortfall.
            if alloc.burn > 0 {
                let burned = T::Currency::withdraw(
                    &gateway,
                    alloc.burn,
                    Precision::Exact,
                    Preservation::Preserve,
                    Fortitude::Polite,
                )
                .map_err(|_| Error::<T>::ShortfallNotCovered)?;
                T::Burn::on_unbalanced(burned);
            }
            let shares_total = alloc
                .shares
                .iter()
                .try_fold(0u128, |a, s| a.checked_add(*s))
                .ok_or(Error::<T>::Overflow)?;
            let held = shares_total
                .checked_add(alloc.gateway_fee)
                .ok_or(Error::<T>::Overflow)?;
            if held > 0 {
                T::Currency::hold(&HoldReason::Pending.into(), &gateway, held)
                    .map_err(|_| Error::<T>::ShortfallNotCovered)?;
            }

            let submitted = T::Epochs::current_epoch();
            let matures = submitted.saturating_add(u64::from(params.challenge_epochs));
            let mut lines = Vec::with_capacity(entries.len());
            let mut epoch_work = 0u128;
            for ((entry, share), work) in entries.into_iter().zip(alloc.shares).zip(alloc.work) {
                Self::add_held(&entry.provider, matures, &gateway, share, 0);
                WorkOf::<T>::mutate(&entry.provider, matures, |w| {
                    let w = w.get_or_insert_with(ProviderWork::default);
                    w.work = w.work.saturating_add(work);
                });
                Lifetime::<T>::mutate(&entry.provider, |l| {
                    l.pending = l.pending.saturating_add(work);
                });
                epoch_work = epoch_work.saturating_add(work);
                lines.push(ReportLine { entry, share, work });
            }
            if alloc.gateway_fee > 0 {
                Self::add_held(&gateway, matures, &gateway, 0, alloc.gateway_fee);
            }
            EpochWorks::<T>::mutate(matures, |w| {
                w.verified = w.verified.saturating_add(epoch_work);
            });

            let id = NextReportId::<T>::get();
            NextReportId::<T>::put(id.saturating_add(1));
            Reports::<T>::insert(
                id,
                ReportRecord {
                    gateway: gateway.clone(),
                    submitted,
                    matures,
                    root,
                    receipt_count,
                    settled: g,
                    burned: alloc.burn,
                    gateway_fee: alloc.gateway_fee,
                    lines: BoundedVec::truncate_from(lines),
                },
            );
            Self::prune(submitted, params.retention_epochs);
            Self::deposit_event(Event::ReportAccepted {
                id,
                gateway,
                root,
                settled: g,
                burned: alloc.burn,
                matures,
            });
            Ok(())
        }

        /// Adds to what `gateway` holds for `who`; a new held entry of a provider is counted in
        /// its [`ProviderWork::held`].
        fn add_held(
            who: &AccountId32,
            epoch: EpochIndex,
            gateway: &AccountId32,
            shares: Balance,
            fees: Balance,
        ) {
            let key = (who.clone(), epoch, gateway.clone());
            let is_new = !HeldFor::<T>::contains_key(&key);
            HeldFor::<T>::mutate(&key, |h| {
                let h = h.get_or_insert_with(Held::default);
                h.shares = h.shares.saturating_add(shares);
                h.fees = h.fees.saturating_add(fees);
            });
            if is_new && shares > 0 {
                WorkOf::<T>::mutate(who, epoch, |w| {
                    let w = w.get_or_insert_with(ProviderWork::default);
                    w.held = w.held.saturating_add(1);
                });
            }
        }

        /// Removes up to four reports past their retention period (design D8).
        fn prune(now: EpochIndex, retention: u32) {
            for _ in 0..4 {
                let id = OldestReport::<T>::get();
                if id >= NextReportId::<T>::get() {
                    return;
                }
                match Reports::<T>::get(id) {
                    Some(r) if r.matures.saturating_add(u64::from(retention)) >= now => return,
                    Some(_) => Reports::<T>::remove(id),
                    None => {}
                }
                OldestReport::<T>::put(id.saturating_add(1));
            }
        }

        fn do_claim(
            who: AccountId32,
            items: BoundedVec<(EpochIndex, AccountId32), ConstU32<64>>,
        ) -> DispatchResult {
            let reason: T::RuntimeHoldReason = HoldReason::Pending.into();
            let (mut fees, mut emission, mut burned) = (0u128, 0u128, 0u128);
            let mut voided: BTreeMap<EpochIndex, bool> = BTreeMap::new();
            for (epoch, gateway) in items {
                let Some(market) = EpochWorks::<T>::get(epoch).market else {
                    continue;
                };
                // The emission share, once per epoch; the work record also says whether the
                // shares are void.
                let is_void = match voided.get(&epoch) {
                    Some(v) => *v,
                    None => {
                        let v = Self::claim_emission(&who, epoch, market, &mut emission)?;
                        voided.insert(epoch, v);
                        v
                    }
                };
                let key = (who.clone(), epoch, gateway.clone());
                let Some(held) = HeldFor::<T>::take(&key) else {
                    continue;
                };
                if held.shares > 0 {
                    if is_void {
                        let (credit, _) = T::Currency::slash(&reason, &gateway, held.shares);
                        burned = burned.saturating_add(held.shares);
                        T::Burn::on_unbalanced(credit);
                    } else {
                        T::Currency::transfer_on_hold(
                            &reason,
                            &gateway,
                            &who,
                            held.shares,
                            Precision::Exact,
                            Restriction::Free,
                            Fortitude::Polite,
                        )?;
                        fees = fees.saturating_add(held.shares);
                    }
                    Self::release_work_record(&who, epoch);
                }
                if held.fees > 0 {
                    T::Currency::release(&reason, &gateway, held.fees, Precision::Exact)?;
                    fees = fees.saturating_add(held.fees);
                }
            }
            if fees > 0 || emission > 0 || burned > 0 {
                Lifetime::<T>::mutate(&who, |l| {
                    l.claimed = l.claimed.saturating_add(fees).saturating_add(emission);
                });
                Self::deposit_event(Event::Claimed {
                    who,
                    fees,
                    emission,
                    burned,
                });
            }
            Ok(())
        }

        /// Pays `who`'s emission share of settled `epoch` (once) and returns whether its work
        /// there is void.
        fn claim_emission(
            who: &AccountId32,
            epoch: EpochIndex,
            market: Balance,
            paid: &mut Balance,
        ) -> Result<bool, sp_runtime::DispatchError> {
            let Some(mut w) = WorkOf::<T>::get(who, epoch) else {
                return Ok(false);
            };
            if !w.emission_claimed {
                w.emission_claimed = true;
                if !w.voided {
                    let verified = EpochWorks::<T>::get(epoch).verified;
                    let share = part(market, w.work, verified);
                    if share > 0 {
                        T::Currency::transfer(&Self::pot(), who, share, Preservation::Preserve)?;
                        Unclaimed::<T>::mutate(|u| *u = u.saturating_sub(share));
                        *paid = paid.saturating_add(share);
                    }
                    Lifetime::<T>::mutate(who, |l| {
                        l.pending = l.pending.saturating_sub(w.work);
                        l.verified = l.verified.saturating_add(w.work);
                    });
                }
                Self::store_work_record(who, epoch, w);
            }
            Ok(w.voided)
        }

        /// One held entry of `who` for `epoch` was settled.
        fn release_work_record(who: &AccountId32, epoch: EpochIndex) {
            if let Some(mut w) = WorkOf::<T>::get(who, epoch) {
                w.held = w.held.saturating_sub(1);
                Self::store_work_record(who, epoch, w);
            }
        }

        /// Stores a work record, or removes it once nothing is left to claim.
        fn store_work_record(who: &AccountId32, epoch: EpochIndex, w: ProviderWork) {
            if w.emission_claimed && w.held == 0 {
                WorkOf::<T>::remove(who, epoch);
            } else {
                WorkOf::<T>::insert(who, epoch, w);
            }
        }

        /// A stored report.
        pub fn report(id: u64) -> Option<ReportOf> {
            Reports::<T>::get(id)
        }

        /// What gateways hold for `who` (runtime API only: iterates `who`'s entries).
        pub fn held(who: &AccountId32) -> Vec<(EpochIndex, AccountId32, Held<Balance>)> {
            HeldFor::<T>::iter_prefix((who.clone(),))
                .map(|((e, g), h)| (e, g, h))
                .collect()
        }

        /// `who`'s unclaimed work by epoch (runtime API only).
        pub fn work(who: &AccountId32) -> Vec<(EpochIndex, ProviderWork)> {
            WorkOf::<T>::iter_prefix(who).collect()
        }

        /// Verified market work of `epoch`.
        pub fn epoch_work(epoch: EpochIndex) -> EpochWork<Balance> {
            EpochWorks::<T>::get(epoch)
        }

        /// `who`'s lifetime work.
        pub fn lifetime(who: &AccountId32) -> LifetimeWork<Balance> {
            Lifetime::<T>::get(who)
        }

        /// Settlement parameters.
        pub fn params() -> WorkParams {
            Params::<T>::get()
        }
    }

    impl<T: Config> WorkSource for Pallet<T> {
        fn verified_work(epoch: EpochIndex) -> (u128, u128) {
            (EpochWorks::<T>::get(epoch).verified, 0)
        }
    }

    impl<T: Config> MarketPayout<AccountId32> for Pallet<T> {
        fn account() -> Option<AccountId32> {
            Some(Self::pot())
        }

        fn settled(epoch: EpochIndex, minted: u128, work: u128) {
            // The pot keeps one existential deposit forever, so claims never reap it (design D6).
            let pot = T::Currency::balance(&Self::pot());
            let floor = Unclaimed::<T>::get().saturating_add(T::Currency::minimum_balance());
            let market = minted.min(pot.saturating_sub(floor));
            Unclaimed::<T>::mutate(|u| *u = u.saturating_add(market));
            EpochWorks::<T>::mutate(epoch, |w| {
                w.verified = work;
                w.market = Some(market);
            });
            Self::deposit_event(Event::MarketSettled {
                epoch,
                market,
                work,
            });
        }
    }

    impl<T: Config> OnJail<AccountId32> for Pallet<T> {
        fn on_jail(who: &AccountId32) {
            let now = T::Epochs::current_epoch();
            let last = now.saturating_add(u64::from(Params::<T>::get().challenge_epochs));
            let mut epoch = now;
            while epoch <= last {
                if EpochWorks::<T>::get(epoch).market.is_none()
                    && let Some(mut w) = WorkOf::<T>::get(who, epoch)
                    && !w.voided
                {
                    w.voided = true;
                    EpochWorks::<T>::mutate(epoch, |e| {
                        e.verified = e.verified.saturating_sub(w.work);
                    });
                    Lifetime::<T>::mutate(who, |l| l.pending = l.pending.saturating_sub(w.work));
                    WorkOf::<T>::insert(who, epoch, w);
                    Self::deposit_event(Event::PendingVoided {
                        who: who.clone(),
                        epoch,
                        work: w.work,
                    });
                }
                epoch = epoch.saturating_add(1);
            }
        }
    }
}
