//! Mock runtime for unit tests: balances and the treasury, with short floor batches.

use frame_support::{derive_impl, parameter_types};
use frame_system::EnsureRoot;
use sp_runtime::traits::IdentityLookup;
use sp_runtime::{AccountId32, BuildStorage};

use crate as pallet_treasury_dual;

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
    pub type Treasury = pallet_treasury_dual::Pallet<Test>;
}

#[derive_impl(frame_system::config_preludes::TestDefaultConfig)]
impl frame_system::Config for Test {
    type Block = Block;
    // 32-byte accounts, as in the runtime: `PalletId` accounts must not collide.
    type AccountId = AccountId32;
    type Lookup = IdentityLookup<AccountId32>;
    type AccountData = pallet_balances::AccountData<u128>;
}

parameter_types! {
    pub const ExistentialDeposit: u128 = 10;
}

#[derive_impl(pallet_balances::config_preludes::TestDefaultConfig)]
impl pallet_balances::Config for Test {
    type Balance = u128;
    type AccountStore = System;
    type ExistentialDeposit = ExistentialDeposit;
}

/// Floor batch length of the mock.
pub const BATCH: u64 = 10;
/// Vesting period of the mock ("two years").
pub const VESTING: u64 = 100;

parameter_types! {
    pub const BatchBlocks: u64 = BATCH;
    pub const VestingBlocks: u64 = VESTING;
    pub const MaxBatches: u32 = 12; // VESTING / BATCH + 2
}

impl pallet_treasury_dual::Config for Test {
    type RuntimeEvent = RuntimeEvent;
    type Currency = Balances;
    type AdminOrigin = EnsureRoot<AccountId32>;
    type BatchBlocks = BatchBlocks;
    type VestingBlocks = VestingBlocks;
    type MaxBatches = MaxBatches;
    type WeightInfo = ();
}

/// Externalities with the default community share.
pub fn ext() -> sp_io::TestExternalities {
    let storage = RuntimeGenesisConfig::default()
        .build_storage()
        .unwrap_or_default();
    let mut ext = sp_io::TestExternalities::new(storage);
    ext.execute_with(|| System::set_block_number(1));
    ext
}
