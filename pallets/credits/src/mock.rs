//! Mock runtime for unit tests: balances, credits, and settable keys, gateways and rate.
// Test code: an overflow here fails the test loudly (debug builds panic), which is what we want.
#![allow(clippy::arithmetic_side_effects)]

use core::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};

use frame_support::{derive_impl, parameter_types};
use sp_core::H256;
use sp_runtime::traits::IdentityLookup;
use sp_runtime::{AccountId32, BuildStorage};

use ac_crypto::sig::SigningKey;
use ac_crypto::{PqPublicKey, SigAlg};
use ac_primitives::market::traits::{AccountKeys, GatewayLookup, PriceSource};
use ac_primitives::market::voucher::{VOUCHER_CONTEXT, key_fingerprint};
use ac_primitives::market::{AtcPerUsd, MicroUsd, SignedVoucher, VoucherBody};

use crate as pallet_credits;
use crate::CreditsParams;

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
}

#[derive_impl(pallet_balances::config_preludes::TestDefaultConfig)]
impl pallet_balances::Config for Test {
    type Balance = u128;
    type AccountStore = System;
    type ExistentialDeposit = ExistentialDeposit;
    type RuntimeHoldReason = RuntimeHoldReason;
}

std::thread_local! {
    static RATE: RefCell<Option<u128>> = const { RefCell::new(Some(ONE_ATC_PER_USD)) };
    static KEYS: RefCell<BTreeMap<AccountId32, [u8; 32]>> = const { RefCell::new(BTreeMap::new()) };
    static GATEWAYS: RefCell<BTreeSet<AccountId32>> = const { RefCell::new(BTreeSet::new()) };
}

/// 10^18 smallest units (1 ATC) per dollar.
pub const ONE_ATC_PER_USD: u128 = 1_000_000_000_000_000_000;
/// One ATC.
pub const ATC: u128 = 1_000_000_000_000_000_000;

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

/// Account keys settable by tests.
pub struct TestKeys;
impl AccountKeys<AccountId32> for TestKeys {
    fn key_fingerprint(who: &AccountId32) -> Option<[u8; 32]> {
        KEYS.with(|k| k.borrow().get(who).copied())
    }
}

/// Gives `who` the account key `key` (as a key rotation would).
pub fn set_key(who: &AccountId32, key: &PqPublicKey) {
    KEYS.with(|k| k.borrow_mut().insert(who.clone(), key_fingerprint(key)));
}

/// Gateways settable by tests.
pub struct TestGateways;
impl GatewayLookup<AccountId32> for TestGateways {
    fn is_active(who: &AccountId32) -> bool {
        GATEWAYS.with(|g| g.borrow().contains(who))
    }
    fn fee_bps(who: &AccountId32) -> Option<u16> {
        Self::is_active(who).then_some(300)
    }
}

/// Makes `who` an active gateway, or not.
pub fn set_gateway(who: &AccountId32, active: bool) {
    GATEWAYS.with(|g| {
        if active {
            g.borrow_mut().insert(who.clone());
        } else {
            g.borrow_mut().remove(who);
        }
    });
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
    type BenchmarkHelper = TestHelper;
}

/// Benchmark helper of the mock.
#[cfg(feature = "runtime-benchmarks")]
pub struct TestHelper;
#[cfg(feature = "runtime-benchmarks")]
impl crate::BenchmarkHelper for TestHelper {
    fn register_key(who: &AccountId32, key: &PqPublicKey) {
        set_key(who, key);
    }
    fn activate_gateway(gateway: &AccountId32) {
        set_gateway(gateway, true);
    }
    fn set_rate(rate: u128) {
        set_rate(Some(rate));
    }
}

/// The user.
pub fn alice() -> AccountId32 {
    AccountId32::new([1; 32])
}
/// Another user.
pub fn bob() -> AccountId32 {
    AccountId32::new([2; 32])
}
/// The gateway.
pub fn gw() -> AccountId32 {
    AccountId32::new([10; 32])
}
/// The settlement payee (an existing account).
pub fn payee() -> AccountId32 {
    AccountId32::new([20; 32])
}

/// A development key.
pub fn key(name: &str) -> SigningKey {
    SigningKey::from_seed(SigAlg::MlDsa44, &ac_crypto::dev_seed(name).unwrap()).unwrap()
}

/// Starting balance of alice and bob.
pub const FUNDS: u128 = 100 * ATC;

/// Genesis hash of the mock chain.
pub fn genesis() -> H256 {
    System::block_hash(0)
}

/// A voucher of `user` for the gateway, signed with `signer`.
pub fn voucher(
    user: &AccountId32,
    signer: &SigningKey,
    channel: u32,
    micro_usd: u128,
) -> SignedVoucher {
    let body = VoucherBody {
        genesis: genesis(),
        user: user.clone(),
        gateway: gw(),
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

/// Withdrawal delay of the mock.
pub const DELAY: u64 = 10;

/// Externalities at block 1: alice (key "alice") and bob (key "bob") funded, `gw` active, the
/// payee existing, rate 1 ATC per dollar.
pub fn ext() -> sp_io::TestExternalities {
    set_rate(Some(ONE_ATC_PER_USD));
    KEYS.with(|k| k.borrow_mut().clear());
    GATEWAYS.with(|g| g.borrow_mut().clear());
    let storage = RuntimeGenesisConfig {
        system: Default::default(),
        balances: pallet_balances::GenesisConfig {
            balances: vec![(alice(), FUNDS), (bob(), FUNDS), (payee(), 1)],
            ..Default::default()
        },
        credits: pallet_credits::GenesisConfig {
            params: CreditsParams {
                withdrawal_delay: DELAY,
            },
            ..Default::default()
        },
    }
    .build_storage()
    .unwrap_or_default();
    let mut ext = sp_io::TestExternalities::new(storage);
    ext.execute_with(|| {
        System::set_block_number(1);
        set_key(&alice(), &key("alice").public_key().unwrap());
        set_key(&bob(), &key("bob").public_key().unwrap());
        set_gateway(&gw(), true);
    });
    ext
}
