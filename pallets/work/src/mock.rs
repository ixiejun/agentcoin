//! Mock runtime for unit tests: balances, the real credits pallet (so vouchers follow the real
//! redemption rules), work settlement, and settable gateways, providers, keys, rate and epoch.
// Test code: an overflow here fails the test loudly (debug builds panic), which is what we want.
#![allow(clippy::arithmetic_side_effects)]

use core::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};

use frame_support::traits::fungible::{Credit, Mutate};
use frame_support::traits::{Imbalance, OnUnbalanced};
use frame_support::{PalletId, derive_impl, parameter_types};
use sp_runtime::traits::IdentityLookup;
use sp_runtime::{AccountId32, BuildStorage};

use ac_crypto::SigAlg;
use ac_crypto::sig::SigningKey;
use ac_primitives::emission::{EpochIndex, EpochIndexSource, MarketPayout, WorkSource};
use ac_primitives::market::traits::{AccountKeys, GatewayLookup, PriceSource, ProviderLookup};
use ac_primitives::market::voucher::{VOUCHER_CONTEXT, key_fingerprint};
use ac_primitives::market::work::JobKind;
use ac_primitives::market::{
    AtcPerUsd, MicroUsd, ModelId, ReportEntry, SignedVoucher, VoucherBody, WorkParams,
};

use crate as pallet_work;

type Block = frame_system::mocking::MockBlock<Test>;

#[frame_support::runtime]
mod runtime {
    #[runtime::runtime]
    #[runtime::derive(
        RuntimeCall,
        RuntimeEvent,
        RuntimeError,
        RuntimeOrigin,
        RuntimeFreezeReason,
        RuntimeHoldReason,
        RuntimeSlashReason,
        RuntimeLockId,
        RuntimeTask,
        RuntimeViewFunction
    )]
    pub struct Test;

    #[runtime::pallet_index(0)]
    pub type System = frame_system::Pallet<Test>;

    #[runtime::pallet_index(1)]
    pub type Balances = pallet_balances::Pallet<Test>;

    #[runtime::pallet_index(2)]
    pub type Credits = pallet_credits::Pallet<Test>;

    #[runtime::pallet_index(3)]
    pub type Work = pallet_work::Pallet<Test>;
}

#[derive_impl(frame_system::config_preludes::TestDefaultConfig)]
impl frame_system::Config for Test {
    type Block = Block;
    type AccountId = AccountId32;
    type Lookup = IdentityLookup<AccountId32>;
    type AccountData = pallet_balances::AccountData<u128>;
}

parameter_types! {
    pub const ExistentialDeposit: u128 = 1;
    pub const WorkPalletId: PalletId = PalletId(*b"ac/work0");
}

#[derive_impl(pallet_balances::config_preludes::TestDefaultConfig)]
impl pallet_balances::Config for Test {
    type Balance = u128;
    type AccountStore = System;
    type ExistentialDeposit = ExistentialDeposit;
    type RuntimeHoldReason = RuntimeHoldReason;
}

std::thread_local! {
    static RATE: RefCell<Option<u128>> = const { RefCell::new(Some(UNITS_PER_USD)) };
    static KEYS: RefCell<BTreeMap<AccountId32, [u8; 32]>> = const { RefCell::new(BTreeMap::new()) };
    static GATEWAYS: RefCell<BTreeMap<AccountId32, (bool, u16)>> =
        const { RefCell::new(BTreeMap::new()) };
    static OFFERS: RefCell<BTreeSet<(AccountId32, ModelId)>> = const { RefCell::new(BTreeSet::new()) };
    static EPOCH: RefCell<EpochIndex> = const { RefCell::new(0) };
    /// Total burned through the burn sink.
    pub static BURNED: RefCell<u128> = const { RefCell::new(0) };
}

/// One smallest unit per micro-dollar: 10^6 units per dollar.
pub const UNITS_PER_USD: u128 = 1_000_000;

/// Rate settable by tests.
pub struct TestPrice;
impl PriceSource for TestPrice {
    fn atc_per_usd() -> Option<AtcPerUsd> {
        RATE.with(|r| r.borrow().map(AtcPerUsd))
    }
}

/// Sets the mock rate.
pub fn set_rate(rate: Option<u128>) {
    RATE.with(|r| *r.borrow_mut() = rate);
}

/// Account keys.
pub struct TestKeys;
impl AccountKeys<AccountId32> for TestKeys {
    fn key_fingerprint(who: &AccountId32) -> Option<[u8; 32]> {
        KEYS.with(|k| k.borrow().get(who).copied())
    }
}

/// Gateways: (active, fee).
pub struct TestGateways;
impl GatewayLookup<AccountId32> for TestGateways {
    fn is_active(who: &AccountId32) -> bool {
        GATEWAYS.with(|g| g.borrow().get(who).is_some_and(|(a, _)| *a))
    }
    fn fee_bps(who: &AccountId32) -> Option<u16> {
        GATEWAYS.with(|g| g.borrow().get(who).map(|(_, f)| *f))
    }
    fn is_registered(who: &AccountId32) -> bool {
        GATEWAYS.with(|g| g.borrow().contains_key(who))
    }
}

/// Registers `who` as a gateway (active or exiting) with `fee_bps`.
pub fn set_gateway(who: &AccountId32, active: bool, fee_bps: u16) {
    GATEWAYS.with(|g| g.borrow_mut().insert(who.clone(), (active, fee_bps)));
}

/// Providers able to settle a model.
pub struct TestProviders;
impl ProviderLookup<AccountId32> for TestProviders {
    fn can_settle(who: &AccountId32, model: &ModelId) -> bool {
        OFFERS.with(|o| o.borrow().contains(&(who.clone(), *model)))
    }
}

/// Lets `who` settle `model`.
pub fn offer(who: &AccountId32, model: ModelId) {
    OFFERS.with(|o| o.borrow_mut().insert((who.clone(), model)));
}

/// The emission epoch.
pub struct TestEpochs;
impl EpochIndexSource for TestEpochs {
    fn current_epoch() -> EpochIndex {
        EPOCH.with(|e| *e.borrow())
    }
}

/// Moves to emission epoch `epoch`.
pub fn set_epoch(epoch: EpochIndex) {
    EPOCH.with(|e| *e.borrow_mut() = epoch);
}

/// Counts burned amounts.
pub struct TestBurn;
impl OnUnbalanced<Credit<AccountId32, Balances>> for TestBurn {
    fn on_nonzero_unbalanced(amount: Credit<AccountId32, Balances>) {
        BURNED.with(|b| *b.borrow_mut() += amount.peek());
    }
}

/// Total burned so far.
pub fn burned() -> u128 {
    BURNED.with(|b| *b.borrow())
}

impl pallet_credits::Config for Test {
    type RuntimeEvent = RuntimeEvent;
    type RuntimeHoldReason = RuntimeHoldReason;
    type Currency = Balances;
    type Gateways = TestGateways;
    type Keys = TestKeys;
    type Price = TestPrice;
    type WeightInfo = ();
    #[cfg(feature = "runtime-benchmarks")]
    type BenchmarkHelper = CreditsHelper;
}

/// Credits benchmark helper (unused by these tests).
#[cfg(feature = "runtime-benchmarks")]
pub struct CreditsHelper;
#[cfg(feature = "runtime-benchmarks")]
impl pallet_credits::BenchmarkHelper for CreditsHelper {
    fn register_key(who: &AccountId32, key: &ac_crypto::PqPublicKey) {
        KEYS.with(|k| k.borrow_mut().insert(who.clone(), key_fingerprint(key)));
    }
    fn activate_gateway(gateway: &AccountId32) {
        set_gateway(gateway, true, 300);
    }
    fn set_rate(rate: u128) {
        set_rate(Some(rate));
    }
}

impl pallet_work::Config for Test {
    type RuntimeEvent = RuntimeEvent;
    type RuntimeHoldReason = RuntimeHoldReason;
    type Currency = Balances;
    type Credit = Credits;
    type Gateways = TestGateways;
    type Providers = TestProviders;
    type Price = TestPrice;
    type Epochs = TestEpochs;
    type Burn = TestBurn;
    type PalletId = WorkPalletId;
    type WeightInfo = ();
    #[cfg(feature = "runtime-benchmarks")]
    type BenchmarkHelper = WorkHelper;
}

/// Benchmark helper of the mock.
#[cfg(feature = "runtime-benchmarks")]
pub struct WorkHelper;
#[cfg(feature = "runtime-benchmarks")]
impl crate::BenchmarkHelper for WorkHelper {
    fn prepare_gateway(gateway: &AccountId32) {
        set_gateway(gateway, true, 500);
        let _ = Balances::mint_into(gateway, 1_000_000 * UNITS_PER_USD);
        set_rate(Some(UNITS_PER_USD));
    }
    fn prepare_provider(provider: &AccountId32, model: &ModelId) {
        offer(provider, *model);
    }
    fn vouchers(gateway: &AccountId32, first: u32, n: u32, micro_usd: u128) -> Vec<SignedVoucher> {
        (first..first + n)
            .map(|i| {
                let mut raw = [7u8; 32];
                raw[..4].copy_from_slice(&i.to_le_bytes());
                let user = AccountId32::new(raw);
                let _ = Balances::mint_into(&user, 10 * micro_usd + 10);
                let signer = key("alice");
                KEYS.with(|k| {
                    k.borrow_mut()
                        .insert(user.clone(), key_fingerprint(&signer.public_key().unwrap()))
                });
                Credits::deposit(
                    RuntimeOrigin::signed(user.clone()),
                    gateway.clone(),
                    micro_usd,
                )
                .unwrap();
                voucher_for(&user, gateway, &signer, 0, micro_usd)
            })
            .collect()
    }
    fn set_epoch(epoch: EpochIndex) {
        set_epoch(epoch);
    }
}

/// A user.
pub fn alice() -> AccountId32 {
    AccountId32::new([1; 32])
}
/// Another user.
pub fn bob() -> AccountId32 {
    AccountId32::new([2; 32])
}
/// The gateway (fee 5%).
pub fn gw() -> AccountId32 {
    AccountId32::new([10; 32])
}
/// Another registered account that is not a gateway.
pub fn stranger() -> AccountId32 {
    AccountId32::new([11; 32])
}
/// A provider.
pub fn p1() -> AccountId32 {
    AccountId32::new([21; 32])
}
/// Another provider.
pub fn p2() -> AccountId32 {
    AccountId32::new([22; 32])
}
/// The served model.
pub const MODEL: ModelId = ModelId([0x42; 32]);
/// A model no provider lists.
pub const OTHER_MODEL: ModelId = ModelId([0x43; 32]);

/// A development key.
pub fn key(name: &str) -> SigningKey {
    SigningKey::from_seed(SigAlg::MlDsa44, &ac_crypto::dev_seed(name).unwrap()).unwrap()
}

/// Starting balance of users.
pub const FUNDS: u128 = 100 * UNITS_PER_USD;
/// Starting balance of the gateway.
pub const GW_FUNDS: u128 = 1_000 * UNITS_PER_USD;
/// Starting balance of providers (free balance above the deposit).
pub const P_FUNDS: u128 = 10;

/// Settlement parameters of the mock: b 20%, k 0.5, challenge 2, retention 3.
pub const PARAMS: WorkParams = WorkParams {
    burn_bps: 2_000,
    emission_bps: 5_000,
    challenge_epochs: 2,
    retention_epochs: 3,
};

/// A voucher of `user` for `gateway`, signed with `signer`.
pub fn voucher_for(
    user: &AccountId32,
    gateway: &AccountId32,
    signer: &SigningKey,
    channel: u32,
    micro_usd: u128,
) -> SignedVoucher {
    let body = VoucherBody {
        genesis: System::block_hash(0),
        user: user.clone(),
        gateway: gateway.clone(),
        channel,
        cumulative: MicroUsd(micro_usd),
    };
    let signature = signer
        .sign_deterministic(&body.payload().unwrap(), VOUCHER_CONTEXT)
        .unwrap();
    SignedVoucher {
        body,
        public_key: signer.public_key().unwrap(),
        signature,
    }
}

/// A voucher of alice (key "alice") or bob (key "bob") for the gateway.
pub fn voucher(user: &AccountId32, micro_usd: u128) -> SignedVoucher {
    let name = if *user == alice() { "alice" } else { "bob" };
    voucher_for(user, &gw(), &key(name), 0, micro_usd)
}

/// An inference entry.
pub fn entry(provider: &AccountId32, micro_usd: u128) -> ReportEntry<AccountId32> {
    ReportEntry {
        kind: JobKind::Inference,
        provider: provider.clone(),
        model: MODEL,
        usd: MicroUsd(micro_usd),
        in_tokens: 1_000,
        out_tokens: 2_000,
    }
}

/// Mints `market` into the pot and settles `epoch` as `Emission` would.
pub fn settle_epoch(epoch: EpochIndex, market: u128) {
    let pot = Work::pot();
    let minted = if market > 0 && Balances::mint_into(&pot, market).is_ok() {
        market
    } else {
        0
    };
    let (work, _) = <Work as WorkSource>::verified_work(epoch);
    <Work as MarketPayout<AccountId32>>::settled(epoch, minted, work);
}

/// Externalities at block 1, epoch 0: users with keys and funds, `gw` a gateway with fee 5%,
/// p1 and p2 providers of `MODEL`, rate 10^6 units per dollar.
pub fn ext() -> sp_io::TestExternalities {
    set_rate(Some(UNITS_PER_USD));
    set_epoch(0);
    KEYS.with(|k| k.borrow_mut().clear());
    GATEWAYS.with(|g| g.borrow_mut().clear());
    OFFERS.with(|o| o.borrow_mut().clear());
    BURNED.with(|b| *b.borrow_mut() = 0);
    let storage = RuntimeGenesisConfig {
        system: Default::default(),
        balances: pallet_balances::GenesisConfig {
            balances: vec![
                (alice(), FUNDS),
                (bob(), FUNDS),
                (gw(), GW_FUNDS),
                (stranger(), GW_FUNDS),
                (p1(), P_FUNDS),
                (p2(), P_FUNDS),
            ],
            ..Default::default()
        },
        credits: pallet_credits::GenesisConfig {
            params: pallet_credits::CreditsParams {
                withdrawal_delay: 10,
            },
            ..Default::default()
        },
        work: pallet_work::GenesisConfig {
            params: PARAMS,
            ..Default::default()
        },
    }
    .build_storage()
    .unwrap_or_default();
    let mut ext = sp_io::TestExternalities::new(storage);
    ext.execute_with(|| {
        System::set_block_number(1);
        for (who, name) in [(alice(), "alice"), (bob(), "bob")] {
            KEYS.with(|k| {
                k.borrow_mut()
                    .insert(who, key_fingerprint(&key(name).public_key().unwrap()))
            });
        }
        set_gateway(&gw(), true, 500);
        offer(&p1(), MODEL);
        offer(&p2(), MODEL);
    });
    ext
}
