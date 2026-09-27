//! Mock runtime for unit tests: Aura-PQ plus the validator set, with a test staking ledger.
// Test code: AGENT.md §5.3 permits unchecked conversions here.
#![allow(clippy::cast_possible_truncation)]

use frame_support::{derive_impl, traits::ConstU32, traits::ConstU64};

use ac_crypto::PqPublicKey;
use ac_primitives::validator_set::StakingInterface;

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

std::thread_local! {
    /// What the test staking ledger reports: total active, issuance, qualified candidates.
    pub static STAKE: core::cell::RefCell<(u128, u128, u32)> = const { core::cell::RefCell::new((0, 1, 0)) };
    /// What an election returns.
    pub static ELECTED: core::cell::RefCell<Vec<(PqPublicKey, u128)>> = const { core::cell::RefCell::new(Vec::new()) };
    /// Elections run: (seats, preview).
    pub static ELECTIONS: core::cell::RefCell<Vec<(u32, bool)>> = const { core::cell::RefCell::new(Vec::new()) };
}

/// A staking ledger driven by the tests.
pub struct TestStaking;
impl StakingInterface for TestStaking {
    fn total_active() -> u128 {
        STAKE.with(|s| s.borrow().0)
    }
    fn total_issuance() -> u128 {
        STAKE.with(|s| s.borrow().1)
    }
    fn qualified_candidates() -> u32 {
        STAKE.with(|s| s.borrow().2)
    }
    fn elect(seats: u32, preview: bool) -> Vec<(PqPublicKey, u128)> {
        ELECTIONS.with(|e| e.borrow_mut().push((seats, preview)));
        ELECTED.with(|e| e.borrow().iter().take(seats as usize).cloned().collect())
    }
}

/// Sets what the test staking ledger reports.
pub fn set_stake(active: u128, issuance: u128, qualified: u32) {
    STAKE.with(|s| *s.borrow_mut() = (active, issuance, qualified));
}

/// Sets what elections return.
pub fn set_elected(elected: Vec<(PqPublicKey, u128)>) {
    ELECTED.with(|e| *e.borrow_mut() = elected);
}

/// Elections run so far.
pub fn elections() -> Vec<(u32, bool)> {
    ELECTIONS.with(|e| e.borrow().clone())
}

/// Resets the test staking ledger.
pub fn reset_staking() {
    set_stake(0, 1, 0);
    set_elected(Vec::new());
    ELECTIONS.with(|e| e.borrow_mut().clear());
}

impl pallet_validator_set::Config for Test {
    type RuntimeEvent = RuntimeEvent;
    type BlockAuthorities = AuraPq;
    type Staking = TestStaking;
    type AdminOrigin = frame_system::EnsureRoot<u64>;
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
