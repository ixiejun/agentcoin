//! Mock runtime for unit tests: balances and emission, with test treasury and security budget.

use ac_primitives::emission::{Phase, SecurityBudget, TreasuryDeposit};
use core::cell::RefCell;
use frame_support::{derive_impl, parameter_types};
use sp_runtime::BuildStorage;

use crate as pallet_emission;

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
    pub type Emission = pallet_emission::Pallet<Test>;
}

#[derive_impl(frame_system::config_preludes::TestDefaultConfig)]
impl frame_system::Config for Test {
    type Block = Block;
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

/// Treasury accounts of the mock: community 100, holder 101, floor 102; 40% / 60% split.
pub const COMMUNITY: u64 = 100;
/// Holder treasury.
pub const HOLDER: u64 = 101;
/// Floor account.
pub const FLOOR: u64 = 102;
/// Validator paid in PoS.
pub const VALIDATOR: u64 = 7;

pub struct TestTreasury;
impl TreasuryDeposit<u64> for TestTreasury {
    fn recipients(proportional: u128, floor_topup: u128) -> [(u64, u128); 3] {
        let (community, holder) = ac_primitives::emission::split_treasury(proportional, 4_000);
        [
            (COMMUNITY, community),
            (HOLDER, holder),
            (FLOOR, floor_topup),
        ]
    }

    fn floor_minted(amount: u128) {
        FLOOR_MINTED.with(|f| {
            let total = f.borrow().saturating_add(amount);
            *f.borrow_mut() = total;
        });
    }
}

std::thread_local! {
    /// Phase reported by the test security budget.
    pub static PHASE: RefCell<Phase> = const { RefCell::new(Phase::Poa) };
    /// Verified work reported by the test work source.
    pub static WORK: RefCell<(u128, u128)> = const { RefCell::new((0, 0)) };
    /// Floor amounts reported as minted.
    pub static FLOOR_MINTED: RefCell<u128> = const { RefCell::new(0) };
}

pub struct TestBudget;
impl SecurityBudget<u64> for TestBudget {
    fn phase() -> Phase {
        PHASE.with(|p| *p.borrow())
    }

    fn recipients(amount: u128) -> Vec<(u64, u128)> {
        match Self::phase() {
            Phase::Pos => vec![(VALIDATOR, amount)],
            _ => Vec::new(),
        }
    }
}

pub struct TestWork;
impl ac_primitives::emission::WorkSource for TestWork {
    fn verified_work(_epoch: u64) -> (u128, u128) {
        WORK.with(|w| *w.borrow())
    }
}

impl pallet_emission::Config for Test {
    type RuntimeEvent = RuntimeEvent;
    type Currency = Balances;
    type WorkSource = TestWork;
    type SecurityBudget = TestBudget;
    type Treasury = TestTreasury;
    type WeightInfo = ();
}

/// Externalities with emission epoch length `length`.
pub fn ext(length: u64) -> sp_io::TestExternalities {
    PHASE.with(|p| *p.borrow_mut() = Phase::Poa);
    WORK.with(|w| *w.borrow_mut() = (0, 0));
    FLOOR_MINTED.with(|f| *f.borrow_mut() = 0);
    let storage = RuntimeGenesisConfig {
        system: Default::default(),
        balances: Default::default(),
        emission: pallet_emission::GenesisConfig {
            epoch_length: length,
            ..Default::default()
        },
    }
    .build_storage()
    .unwrap_or_default();
    let mut ext = sp_io::TestExternalities::new(storage);
    ext.execute_with(|| System::set_block_number(1));
    ext
}
