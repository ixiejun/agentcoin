//! Mock runtime for unit tests: balances, the gateways pallet and a settable rate.

use core::cell::RefCell;

use frame_support::{derive_impl, parameter_types};
use sp_runtime::BuildStorage;

use ac_primitives::market::AtcPerUsd;
use ac_primitives::market::traits::PriceSource;

use crate as pallet_gateways;
use crate::GatewayParams;

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
    pub type Gateways = pallet_gateways::Pallet<Test>;
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

impl pallet_gateways::Config for Test {
    type RuntimeEvent = RuntimeEvent;
    type RuntimeHoldReason = RuntimeHoldReason;
    type Currency = Balances;
    type Price = TestPrice;
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
/// Unbonding period of the mock.
pub const UNBOND: u64 = 20;
/// Gateway threshold at the default mock rate: $1,000 × 1,000 units per dollar.
pub const MIN: u128 = 1_000_000;

/// Registers `who` with a 3% fee and `stake`.
pub fn register(who: u64, stake: u128) -> sp_runtime::DispatchResult {
    Gateways::register(
        RuntimeOrigin::signed(who),
        frame_support::BoundedVec::truncate_from(b"https://g.example".to_vec()),
        300,
        stake,
    )
}

/// Externalities at block 1, mock rate 1,000 units per dollar.
pub fn ext() -> sp_io::TestExternalities {
    set_rate(Some(1_000));
    let storage = RuntimeGenesisConfig {
        system: Default::default(),
        balances: pallet_balances::GenesisConfig {
            balances: vec![(ALICE, FUNDS), (BOB, FUNDS)],
            ..Default::default()
        },
        gateways: pallet_gateways::GenesisConfig {
            params: GatewayParams {
                unbond_blocks: UNBOND,
                ..GatewayParams::LIVE
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
