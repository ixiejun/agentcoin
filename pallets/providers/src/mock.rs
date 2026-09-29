//! Mock runtime for unit tests: balances, the providers pallet, a settable rate and a model set.
// Test code: an overflow here fails the test loudly (debug builds panic), which is what we want.
#![allow(clippy::arithmetic_side_effects)]

use core::cell::RefCell;

use frame_support::traits::Imbalance;
use frame_support::traits::OnUnbalanced;
use frame_support::traits::fungible::Credit;
use frame_support::{derive_impl, parameter_types};
use sp_runtime::BuildStorage;

use ac_crypto::{KemAlg, KemPublicKey};
use ac_primitives::market::records::{ModelPrice, Tier};
use ac_primitives::market::traits::{ModelLookup, PriceSource};
use ac_primitives::market::{AtcPerUsd, MicroUsd, ModelId, PricePerMTok};

use crate as pallet_providers;
use crate::ProviderParams;

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
    pub type Providers = pallet_providers::Pallet<Test>;
}

#[derive_impl(frame_system::config_preludes::TestDefaultConfig)]
impl frame_system::Config for Test {
    type Block = Block;
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
    /// Reference rate of the mock (smallest units per dollar).
    pub static RATE: RefCell<Option<u128>> = const { RefCell::new(Some(1_000)) };
    /// Total burned through the slash sink.
    pub static BURNED: RefCell<u128> = const { RefCell::new(0) };
    /// Accounts the jail hook was called for, in order.
    pub static JAILED: RefCell<Vec<u64>> = const { RefCell::new(Vec::new()) };
}

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

/// Models whose first byte is non-zero exist.
pub struct TestModels;
impl ModelLookup for TestModels {
    fn exists(id: &ModelId) -> bool {
        id.0[0] != 0
    }
}

/// Counts burned stake.
pub struct TestBurn;
impl OnUnbalanced<Credit<u64, Balances>> for TestBurn {
    fn on_nonzero_unbalanced(amount: Credit<u64, Balances>) {
        BURNED.with(|b| *b.borrow_mut() += amount.peek());
    }
}

/// Records jail notifications.
pub struct TestOnJail;
impl ac_primitives::market::traits::OnJail<u64> for TestOnJail {
    fn on_jail(who: &u64) {
        JAILED.with(|j| j.borrow_mut().push(*who));
    }
}

impl pallet_providers::Config for Test {
    type RuntimeEvent = RuntimeEvent;
    type RuntimeHoldReason = RuntimeHoldReason;
    type Currency = Balances;
    type Models = TestModels;
    type Price = TestPrice;
    type Slash = TestBurn;
    type OnJail = TestOnJail;
    type WeightInfo = ();
    #[cfg(feature = "runtime-benchmarks")]
    type BenchmarkHelper = ();
}

/// A funded account.
pub const ALICE: u64 = 1;
/// A funded account.
pub const BOB: u64 = 2;
/// Starting balance.
pub const FUNDS: u128 = 10_000_000;
/// Heartbeat interval of the mock.
pub const HEARTBEAT: u64 = 10;
/// Unbonding period of the mock.
pub const UNBOND: u64 = 20;
/// T2 threshold at the default mock rate: $100 × 1,000 units per dollar.
pub const T2_MIN: u128 = 100_000;
/// T1 threshold at the default mock rate.
pub const T1_MIN: u128 = 1_000_000;

/// A registered model.
pub const MODEL_A: ModelId = ModelId([1; 32]);
/// Another registered model.
pub const MODEL_B: ModelId = ModelId([2; 32]);
/// An unregistered model.
pub const UNKNOWN: ModelId = ModelId([0; 32]);

/// An X-Wing key.
pub fn kem() -> KemPublicKey {
    KemPublicKey::new(KemAlg::XWing, &[7; 1216]).unwrap()
}

/// A price entry.
pub fn price(model: ModelId, input: u128, output: u128) -> ModelPrice {
    ModelPrice {
        model,
        price: PricePerMTok {
            input: MicroUsd(input),
            output: MicroUsd(output),
        },
    }
}

/// Registers `who` as T2 with `models` and `stake`.
pub fn register(who: u64, models: &[ModelPrice], stake: u128) -> sp_runtime::DispatchResult {
    Providers::register(
        RuntimeOrigin::signed(who),
        registration(Tier::T2, models, stake),
    )
}

/// A registration with a valid endpoint and key.
pub fn registration(tier: Tier, models: &[ModelPrice], stake: u128) -> crate::Registration {
    crate::Registration {
        tier,
        endpoint: frame_support::BoundedVec::truncate_from(b"https://p.example".to_vec()),
        kem_pk: kem(),
        models: frame_support::BoundedVec::truncate_from(models.to_vec()),
        stake,
        attestation: None,
    }
}

/// Externalities at block 1, mock rate 1,000 units per dollar, nothing burned.
pub fn ext() -> sp_io::TestExternalities {
    set_rate(Some(1_000));
    BURNED.with(|b| *b.borrow_mut() = 0);
    let storage = RuntimeGenesisConfig {
        system: Default::default(),
        balances: pallet_balances::GenesisConfig {
            balances: vec![(ALICE, FUNDS), (BOB, FUNDS)],
            ..Default::default()
        },
        providers: pallet_providers::GenesisConfig {
            params: ProviderParams {
                heartbeat_interval: HEARTBEAT,
                unbond_blocks: UNBOND,
                ..ProviderParams::LIVE
            },
            ..Default::default()
        },
    }
    .build_storage()
    .unwrap_or_default();
    let mut ext = sp_io::TestExternalities::new(storage);
    ext.execute_with(|| System::set_block_number(1));
    ext
}

/// Burned so far.
pub fn burned() -> u128 {
    BURNED.with(|b| *b.borrow())
}

/// Issuance plus everything burned: constant unless something is minted.
pub fn issuance_and_burned() -> u128 {
    Balances::total_issuance() + burned()
}
