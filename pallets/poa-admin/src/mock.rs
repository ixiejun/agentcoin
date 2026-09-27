//! Mock runtime for unit tests: the council (collective instance 1) and the administration.

use frame_support::traits::Contains;
use frame_support::weights::Weight;
use frame_support::{derive_impl, parameter_types};
use frame_system::EnsureNever;
use sp_runtime::BuildStorage;

use crate as pallet_poa_admin;

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
    pub type Council = pallet_collective::Pallet<Test, Instance1>;

    #[runtime::pallet_index(2)]
    pub type PoaAdmin = pallet_poa_admin::Pallet<Test>;
}

#[derive_impl(frame_system::config_preludes::TestDefaultConfig)]
impl frame_system::Config for Test {
    type Block = Block;
}

parameter_types! {
    pub MaxProposalWeight: Weight = Weight::from_parts(1_000_000_000_000, u64::MAX);
}

impl pallet_collective::Config<pallet_collective::Instance1> for Test {
    type RuntimeOrigin = RuntimeOrigin;
    type Proposal = RuntimeCall;
    type RuntimeEvent = RuntimeEvent;
    type MotionDuration = pallet_poa_admin::MotionDurationOf<Test>;
    type MaxProposals = frame_support::traits::ConstU32<10>;
    type MaxMembers = frame_support::traits::ConstU32<5>;
    type DefaultVote = pallet_collective::PrimeDefaultVote;
    type WeightInfo = ();
    type SetMembersOrigin = EnsureNever<()>;
    type MaxProposalWeight = MaxProposalWeight;
    type DisapproveOrigin = crate::EnsureCouncilThreshold<Test>;
    type KillOrigin = crate::EnsureCouncilThreshold<Test>;
    type Consideration = ();
}

/// Refuses `System::set_storage` as Root, standing in for the runtime's holder-treasury lock.
pub struct NoSetStorage;
impl Contains<RuntimeCall> for NoSetStorage {
    fn contains(call: &RuntimeCall) -> bool {
        !matches!(
            call,
            RuntimeCall::System(frame_system::Call::set_storage { .. })
        )
    }
}

impl pallet_poa_admin::Config for Test {
    type RuntimeEvent = RuntimeEvent;
    type RuntimeCall = RuntimeCall;
    type AdminOrigin = crate::EnsureCouncilThreshold<Test>;
    type RootCallFilter = NoSetStorage;
    type WeightInfo = ();
}

/// Council members of the mock.
pub const ALICE: u64 = 1;
/// Council member.
pub const BOB: u64 = 2;
/// Council member.
pub const CHARLIE: u64 = 3;
/// Not a member at genesis.
pub const DAVE: u64 = 4;
/// Motion duration of the mock.
pub const MOTION: u64 = 20;

/// Externalities with members alice, bob and charlie, threshold 2.
pub fn ext() -> sp_io::TestExternalities {
    let storage = RuntimeGenesisConfig {
        system: Default::default(),
        council: pallet_collective::GenesisConfig {
            members: vec![ALICE, BOB, CHARLIE],
            ..Default::default()
        },
        poa_admin: pallet_poa_admin::GenesisConfig {
            threshold: 2,
            motion_duration: 20,
            ..Default::default()
        },
    }
    .build_storage()
    .unwrap_or_default();
    let mut ext = sp_io::TestExternalities::new(storage);
    ext.execute_with(|| System::set_block_number(1));
    ext
}
