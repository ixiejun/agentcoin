//! Mock runtime for unit tests: balances and staking, with a fixed epoch length of 10 blocks.
// Test code: AGENT.md §5.3 permits unwrap and unchecked arithmetic here.
#![allow(clippy::unwrap_used, clippy::arithmetic_side_effects)]

use ac_crypto::sig::SigningKey;
use ac_crypto::{PqPublicKey, PqSignature, SigAlg};
use ac_primitives::ac_bft::{Authority, SetId};
use ac_primitives::epoch::{EpochIndex, epoch_of};
use ac_primitives::staking::ChainPhase;
use ac_primitives::staking::{VALIDATOR_POP_CONTEXT, pop_statement};
use ac_primitives::validator_set::{CurrentAuthor, RevealTracker, ValidatorSetInterface};
use frame_support::traits::ConstU32;
use frame_support::traits::fungible::Credit;
use frame_support::traits::{Imbalance, OnUnbalanced};
use frame_support::{derive_impl, parameter_types};
use sp_runtime::BuildStorage;

use crate as pallet_staking_pos;
use crate::StakingParams;

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
    pub type StakingPos = pallet_staking_pos::Pallet<Test>;
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
}

/// Epoch length of the mock.
pub const EPOCH: u64 = 10;

std::thread_local! {
    /// Phase reported by the mock validator set.
    pub static PHASE: core::cell::RefCell<ChainPhase> = const { core::cell::RefCell::new(ChainPhase::Poa) };
    /// Author of the current block.
    pub static AUTHOR: core::cell::RefCell<Option<PqPublicKey>> = const { core::cell::RefCell::new(None) };
    /// Key identifiers reported as missed reveals in the current block.
    pub static MISSED: core::cell::RefCell<Vec<[u8; 32]>> = const { core::cell::RefCell::new(Vec::new()) };
    /// Total burned through the slash sink.
    pub static BURNED: core::cell::RefCell<u128> = const { core::cell::RefCell::new(0) };
}

/// Sets the phase of the mock validator set.
pub fn set_phase(phase: ChainPhase) {
    PHASE.with(|p| *p.borrow_mut() = phase);
}

/// Sets the author of the next blocks.
pub fn set_author(key: Option<PqPublicKey>) {
    AUTHOR.with(|a| *a.borrow_mut() = key);
}

/// Sets the missed reveals reported in the current block.
pub fn set_missed(missed: Vec<[u8; 32]>) {
    MISSED.with(|m| *m.borrow_mut() = missed);
}

/// Total burned through slashing.
pub fn burned() -> u128 {
    BURNED.with(|b| *b.borrow())
}

/// Epochs of the mock: fixed length, no authority set.
pub struct TestEpochs;
impl ValidatorSetInterface for TestEpochs {
    fn current() -> (SetId, Vec<Authority>) {
        (0, Vec::new())
    }
    fn historical(_set_id: SetId) -> Option<Vec<Authority>> {
        None
    }
    fn is_recent_member(_key: &PqPublicKey) -> bool {
        false
    }
    fn disable(_key: &PqPublicKey) -> bool {
        false
    }
    fn current_epoch() -> EpochIndex {
        epoch_of(System::block_number(), EPOCH).unwrap_or(0)
    }
    fn epoch_length() -> u64 {
        EPOCH
    }
    fn phase() -> ChainPhase {
        PHASE.with(|p| *p.borrow())
    }
}

/// Author of the mock.
pub struct TestAuthor;
impl CurrentAuthor for TestAuthor {
    fn current_author() -> Option<PqPublicKey> {
        AUTHOR.with(|a| a.borrow().clone())
    }
    fn current_slot() -> u64 {
        0
    }
}

/// Missed reveals of the mock.
pub struct TestReveals;
impl RevealTracker for TestReveals {
    fn missed_now() -> Vec<[u8; 32]> {
        MISSED.with(|m| m.borrow().clone())
    }
}

/// Burns slashed credit and records the amount.
pub struct TestBurn;
impl OnUnbalanced<Credit<u64, Balances>> for TestBurn {
    fn on_nonzero_unbalanced(amount: Credit<u64, Balances>) {
        BURNED.with(|b| *b.borrow_mut() += amount.peek());
    }
}

impl pallet_staking_pos::Config for Test {
    type RuntimeEvent = RuntimeEvent;
    type RuntimeHoldReason = RuntimeHoldReason;
    type Currency = Balances;
    type Epochs = TestEpochs;
    type Author = TestAuthor;
    type Reveals = TestReveals;
    type Slash = TestBurn;
    type PayoutsPerBlock = ConstU32<2>;
    type MaxCandidates = ConstU32<500>;
    type MaxNominators = ConstU32<2_000>;
    type MaxUnlocking = ConstU32<4>;
    type MaxWinners = ConstU32<1_000>;
    type WeightInfo = ();
}

/// Endowed accounts `1..=ACCOUNTS`.
pub const ACCOUNTS: u64 = 40;
/// Endowment of each account.
pub const ENDOWMENT: u128 = 250_000;
/// Total issuance of the mock: 10,000,000, so the minimum self-stake is 10,000 and the minimum
/// nomination 100.
pub const ISSUANCE: u128 = ENDOWMENT * ACCOUNTS as u128;
/// Minimum self-stake at [`ISSUANCE`].
pub const MIN_SELF: u128 = 10_000;
/// Minimum nomination at [`ISSUANCE`].
pub const MIN_NOM: u128 = 100;

/// Parameters of the mock: small lists, short periods.
pub fn params() -> StakingParams {
    StakingParams {
        max_candidates: 5,
        max_nominators: 5,
        self_unbond_blocks: 28,
        nomination_unbond_min: 2,
        nomination_unbond_max: 28,
        commission_delay: 7,
    }
}

/// Validator signing key of seed `seed`.
pub fn validator(seed: u8) -> SigningKey {
    let name = format!("validator-{seed}");
    SigningKey::from_seed(SigAlg::MlDsa65, &ac_crypto::dev_seed(&name).unwrap()).unwrap()
}

/// Validator key and its proof of possession for account `who`.
pub fn key_and_proof(who: u64, seed: u8) -> (PqPublicKey, PqSignature) {
    let key = validator(seed);
    let public = key.public_key().unwrap();
    let genesis = System::block_hash(0);
    let statement = pop_statement(&genesis, &who, &public);
    let proof = key
        .sign_deterministic(&statement, VALIDATOR_POP_CONTEXT)
        .unwrap();
    (public, proof)
}

/// Externalities with [`params`], every account endowed, at block 1.
pub fn ext() -> sp_io::TestExternalities {
    ext_with(params())
}

/// Externalities with `params`.
pub fn ext_with(params: StakingParams) -> sp_io::TestExternalities {
    set_phase(ChainPhase::Poa);
    set_author(None);
    set_missed(Vec::new());
    BURNED.with(|b| *b.borrow_mut() = 0);
    let storage = RuntimeGenesisConfig {
        system: Default::default(),
        balances: pallet_balances::GenesisConfig {
            balances: (1..=ACCOUNTS).map(|a| (a, ENDOWMENT)).collect(),
            ..Default::default()
        },
        staking_pos: pallet_staking_pos::GenesisConfig {
            params,
            ..Default::default()
        },
    }
    .build_storage()
    .unwrap();
    let mut ext = sp_io::TestExternalities::new(storage);
    ext.execute_with(|| System::set_block_number(1));
    ext
}
