//! Mock runtime for unit tests. Root stands in for the PoA administration.

use frame_support::derive_impl;
use frame_system::EnsureRoot;
use sp_runtime::BuildStorage;

use ac_primitives::market::AtcPerUsd;

use crate as pallet_ref_rate;
use crate::RefRateParams;

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
    pub type RefRate = pallet_ref_rate::Pallet<Test>;
}

#[derive_impl(frame_system::config_preludes::TestDefaultConfig)]
impl frame_system::Config for Test {
    type Block = Block;
}

impl pallet_ref_rate::Config for Test {
    type RuntimeEvent = RuntimeEvent;
    type AdminOrigin = EnsureRoot<u64>;
    type WeightInfo = ();
}

/// Minimum interval of the mock.
pub const INTERVAL: u64 = 10;

/// Externalities at block 1 with an optional initial rate.
pub fn ext(initial: Option<u128>) -> sp_io::TestExternalities {
    let storage = RuntimeGenesisConfig {
        system: Default::default(),
        ref_rate: pallet_ref_rate::GenesisConfig {
            initial: initial.map(AtcPerUsd),
            params: RefRateParams {
                min_interval: INTERVAL,
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
