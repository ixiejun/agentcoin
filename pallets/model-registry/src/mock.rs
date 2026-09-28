//! Mock runtime for unit tests: balances and the registry.

use frame_support::{derive_impl, parameter_types};
use sp_runtime::BuildStorage;

use crate as pallet_model_registry;

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
    pub type ModelRegistry = pallet_model_registry::Pallet<Test>;
}

#[derive_impl(frame_system::config_preludes::TestDefaultConfig)]
impl frame_system::Config for Test {
    type Block = Block;
    type AccountData = pallet_balances::AccountData<u128>;
}

parameter_types! {
    pub const ExistentialDeposit: u128 = 1;
    pub const DepositPerItem: u128 = 1_000;
    pub const DepositPerByte: u128 = 10;
}

#[derive_impl(pallet_balances::config_preludes::TestDefaultConfig)]
impl pallet_balances::Config for Test {
    type Balance = u128;
    type AccountStore = System;
    type ExistentialDeposit = ExistentialDeposit;
    type RuntimeHoldReason = RuntimeHoldReason;
}

impl pallet_model_registry::Config for Test {
    type RuntimeEvent = RuntimeEvent;
    type RuntimeHoldReason = RuntimeHoldReason;
    type Currency = Balances;
    type DepositPerItem = DepositPerItem;
    type DepositPerByte = DepositPerByte;
    type WeightInfo = ();
}

/// A funded account.
pub const ALICE: u64 = 1;
/// A funded account.
pub const BOB: u64 = 2;
/// An account with almost nothing.
pub const POOR: u64 = 3;
/// Starting balance of the funded accounts.
pub const FUNDS: u128 = 1_000_000_000;

/// Externalities at block 1.
pub fn ext() -> sp_io::TestExternalities {
    let storage = RuntimeGenesisConfig {
        system: Default::default(),
        balances: pallet_balances::GenesisConfig {
            balances: vec![(ALICE, FUNDS), (BOB, FUNDS), (POOR, 100)],
            ..Default::default()
        },
    }
    .build_storage()
    .unwrap_or_default();
    let mut ext = sp_io::TestExternalities::new(storage);
    ext.execute_with(|| System::set_block_number(1));
    ext
}
