//! Mock runtime for unit tests: Aura-PQ plus the validator set.

use frame_support::{derive_impl, traits::ConstU32, traits::ConstU64};

use crate as pallet_validator_set;

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
    pub type AuraPq = pallet_aura_pq::Pallet<Test>;

    #[runtime::pallet_index(2)]
    pub type ValidatorSet = pallet_validator_set::Pallet<Test>;
}

#[derive_impl(frame_system::config_preludes::TestDefaultConfig)]
impl frame_system::Config for Test {
    type Block = Block;
}

impl pallet_aura_pq::Config for Test {
    type SlotDuration = ConstU64<1000>;
    type MaxAuthorities = ConstU32<10>;
}

impl pallet_validator_set::Config for Test {
    type RuntimeEvent = RuntimeEvent;
    type BlockAuthorities = AuraPq;
    type MaxAuthorities = ConstU32<10>;
    type HistoryEpochs = ConstU32<2>;
    type WeightInfo = ();
}

/// Empty externalities for the benchmark test suite (benchmarks install their own state).
#[cfg(feature = "runtime-benchmarks")]
pub fn new_bench_ext() -> sp_io::TestExternalities {
    use sp_runtime::BuildStorage;
    RuntimeGenesisConfig::default()
        .build_storage()
        .unwrap()
        .into()
}
