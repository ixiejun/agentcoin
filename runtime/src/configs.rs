//! Pallet configuration.

use frame_support::{
    derive_impl, parameter_types,
    traits::{ConstU8, ConstU32, ConstU64, ConstU128, VariantCountOf},
    weights::{
        ConstantMultiplier, IdentityFee, Weight,
        constants::{RocksDbWeight, WEIGHT_REF_TIME_PER_SECOND},
    },
};
use frame_system::limits::{BlockLength, BlockWeights};
use pallet_transaction_payment::{ConstFeeMultiplier, FungibleAdapter, Multiplier};
use sp_runtime::{Perbill, traits::IdentityLookup, traits::One};

// `derive_impl` expands to associated types that name these items.
use super::{
    AccountId, AuraPq, Balance, Balances, Block, BlockNumber, EXISTENTIAL_DEPOSIT, Emission, Hash,
    MILLISECS_PER_BLOCK, Nonce, PalletInfo, Runtime, RuntimeCall, RuntimeEvent,
    RuntimeFreezeReason, RuntimeHoldReason, RuntimeOrigin, RuntimeTask, TreasuryDual, VERSION,
    ValidatorSet,
};
use crate::holder_lock::HolderTreasuryLock;
use ac_primitives::Blake3Hasher;
use ac_primitives::emission::{FLOOR_BATCH_BLOCKS, FLOOR_VESTING_BLOCKS, PoaPhase};
use frame_support::traits::EitherOfDiverse;
use frame_support::traits::fungible::{Balanced, Credit};
use frame_support::traits::{Imbalance, OnUnbalanced};
use frame_system::{EnsureNever, EnsureRoot};

const NORMAL_DISPATCH_RATIO: Perbill = Perbill::from_percent(75);

parameter_types! {
    pub const BlockHashCount: BlockNumber = 2400;
    pub const Version: sp_version::RuntimeVersion = VERSION;
    // A third of the 1 s block time for execution leaves room for import and propagation.
    pub RuntimeBlockWeights: BlockWeights = BlockWeights::with_sensible_defaults(
        Weight::from_parts(WEIGHT_REF_TIME_PER_SECOND / 3, u64::MAX),
        NORMAL_DISPATCH_RATIO,
    );
    pub RuntimeBlockLength: BlockLength = BlockLength::builder()
        .max_length(5 * 1024 * 1024)
        .build();
    pub FeeMultiplier: Multiplier = Multiplier::one();
}

#[derive_impl(frame_system::config_preludes::SolochainDefaultConfig)]
impl frame_system::Config for Runtime {
    type Block = Block;
    type BlockWeights = RuntimeBlockWeights;
    type BlockLength = RuntimeBlockLength;
    type AccountId = AccountId;
    type Lookup = IdentityLookup<AccountId>;
    type Nonce = Nonce;
    type Hash = Hash;
    type Hashing = Blake3Hasher;
    type BlockHashCount = BlockHashCount;
    type DbWeight = RocksDbWeight;
    type Version = Version;
    type AccountData = pallet_balances::AccountData<Balance>;
    type MaxConsumers = ConstU32<16>;
    /// The holder treasury is locked (decision D42); Root bypasses this filter, so
    /// `PoaAdmin::dispatch_as_root` applies it again.
    type BaseCallFilter = HolderTreasuryLock;
}

impl pallet_timestamp::Config for Runtime {
    type Moment = u64;
    type OnTimestampSet = AuraPq;
    type MinimumPeriod = ConstU64<{ MILLISECS_PER_BLOCK / 2 }>;
    type WeightInfo = ();
}

impl pallet_aura_pq::Config for Runtime {
    type SlotDuration = ConstU64<MILLISECS_PER_BLOCK>;
    type MaxAuthorities = ConstU32<{ ac_primitives::aura_pq::MAX_AUTHORITIES }>;
}

impl pallet_balances::Config for Runtime {
    type MaxLocks = ConstU32<50>;
    type MaxReserves = ();
    type ReserveIdentifier = [u8; 8];
    type Balance = Balance;
    type RuntimeEvent = RuntimeEvent;
    // Dust of reaped accounts is burned through `Emission`, so it counts in `TotalBurned` and
    // Δissuance = minted − burned holds exactly (chain/native-token "总量守恒").
    type DustRemoval = Emission;
    type ExistentialDeposit = ConstU128<EXISTENTIAL_DEPOSIT>;
    type AccountStore = frame_system::Pallet<Runtime>;
    type WeightInfo = pallet_balances::weights::SubstrateWeight<Runtime>;
    type FreezeIdentifier = RuntimeFreezeReason;
    type MaxFreezes = VariantCountOf<RuntimeFreezeReason>;
    type RuntimeHoldReason = RuntimeHoldReason;
    type RuntimeFreezeReason = RuntimeFreezeReason;
    type DoneSlashHandler = ();
}

/// Price of one byte of transaction length: 10^11 (a 3.8 KB first transaction costs about
/// 0.0004 ATC). Economic parameters are revisited in M3.
pub const TRANSACTION_BYTE_FEE: Balance = 100_000_000_000;

/// Share of every fee and tip paid to the block author, in percent (plan §8); the rest,
/// including the rounding remainder, is burned.
pub const AUTHOR_FEE_PERCENT: Balance = 20;

/// Fee distribution (spec economics/fee-distribution): 20% (rounded down) of every fee and tip
/// to the block author, the rest burned through [`Emission`] so it counts in
/// `Emission::TotalBurned`. The author's share is burned too when there is no author or its
/// account cannot receive it (below the existential deposit of a new account).
pub struct DealWithFees;

impl DealWithFees {
    /// Account of the current block's author: derived from its block-sealing key.
    fn author() -> Option<AccountId> {
        pallet_aura_pq::CurrentAuthor::<Runtime>::get()
            .map(|key| pallet_pq_accounts::derived_account(&key))
    }
}

impl OnUnbalanced<Credit<AccountId, Balances>> for DealWithFees {
    fn on_nonzero_unbalanced(amount: Credit<AccountId, Balances>) {
        let share = amount
            .peek()
            .saturating_mul(AUTHOR_FEE_PERCENT)
            .checked_div(100)
            .unwrap_or(0);
        let (to_author, to_burn) = amount.split(share);
        Emission::on_unbalanced(to_burn);
        match Self::author() {
            Some(author) => {
                if let Err(refused) = <Balances as Balanced<AccountId>>::resolve(&author, to_author)
                {
                    Emission::on_unbalanced(refused);
                }
            }
            None => Emission::on_unbalanced(to_author),
        }
    }
}

impl pallet_transaction_payment::Config for Runtime {
    type RuntimeEvent = RuntimeEvent;
    // Fees and tips alike: 20% to the author, 80% burned (m3-economics design D7).
    type OnChargeTransaction = FungibleAdapter<Balances, DealWithFees>;
    type OperationalFeeMultiplier = ConstU8<5>;
    type WeightToFee = IdentityFee<Balance>;
    type LengthToFee = ConstantMultiplier<Balance, ConstU128<TRANSACTION_BYTE_FEE>>;
    type FeeMultiplierUpdate = ConstFeeMultiplier<FeeMultiplier>;
    type WeightInfo = pallet_transaction_payment::weights::SubstrateWeight<Runtime>;
}

impl pallet_pq_accounts::Config for Runtime {
    type RuntimeEvent = RuntimeEvent;
    type WeightInfo = pallet_pq_accounts::weights::SubstrateWeight<Runtime>;
}

impl pallet_validator_set::Config for Runtime {
    type RuntimeEvent = RuntimeEvent;
    type BlockAuthorities = AuraPq;
    type MaxAuthorities = ConstU32<{ ac_primitives::aura_pq::MAX_AUTHORITIES }>;
    /// Sets of the last eight epochs stay available for double-signing evidence.
    type HistoryEpochs = ConstU32<8>;
    type WeightInfo = pallet_validator_set::weights::SubstrateWeight<Runtime>;
}

impl pallet_ac_offences::Config for Runtime {
    type RuntimeEvent = RuntimeEvent;
    type ValidatorSet = ValidatorSet;
    type Slots = AuraPq;
    /// PoA validators hold no stake (decisions D9, D19): nothing to slash until M3.
    type SlashHandler = ();
    type MaxAuthorities = ConstU32<{ ac_primitives::aura_pq::MAX_AUTHORITIES }>;
    /// Seal evidence older than eight 600-slot epochs is rejected (matches `HistoryEpochs`).
    type MaxEvidenceAge = ConstU64<4_800>;
    type WeightInfo = pallet_ac_offences::weights::SubstrateWeight<Runtime>;
}

impl pallet_randomness_cr::Config for Runtime {
    type RuntimeEvent = RuntimeEvent;
    type Author = AuraPq;
    type Epochs = ValidatorSet;
    type MaxAuthorities = ConstU32<{ ac_primitives::aura_pq::MAX_AUTHORITIES }>;
    /// Randomness of the last sixteen epochs stays queryable.
    type KeepEpochs = ConstU32<16>;
    type WeightInfo = pallet_randomness_cr::weights::SubstrateWeight<Runtime>;
}

impl pallet_emission::Config for Runtime {
    type RuntimeEvent = RuntimeEvent;
    type Currency = Balances;
    /// No verified work before M5 (`pallet-work`).
    type WorkSource = ();
    /// PoA: the security budget rolls over (decision D19) until `m3-pos`.
    type SecurityBudget = PoaPhase;
    type Treasury = TreasuryDual;
    type WeightInfo = pallet_emission::weights::SubstrateWeight<Runtime>;
}

parameter_types! {
    pub const FloorBatchBlocks: u64 = FLOOR_BATCH_BLOCKS;
    pub const FloorVestingBlocks: u64 = FLOOR_VESTING_BLOCKS;
    // Batches still vesting: VestingBlocks / BatchBlocks + 2 (the open batch and a partial one).
    pub const MaxFloorBatches: u32 = 26;
}

impl pallet_treasury_dual::Config for Runtime {
    type RuntimeEvent = RuntimeEvent;
    type Currency = Balances;
    type AdminOrigin = TreasuryAdminOrigin;
    type BatchBlocks = FloorBatchBlocks;
    type VestingBlocks = FloorVestingBlocks;
    type MaxBatches = MaxFloorBatches;
    type WeightInfo = pallet_treasury_dual::weights::SubstrateWeight<Runtime>;
}

/// The chain's administration during PoA: a PoA-council motion approved by at least the
/// threshold of members (decision D41).
pub type AdminOrigin = pallet_poa_admin::EnsureCouncilThreshold<Runtime>;

/// Treasury spends: the administration, directly or through `PoaAdmin::dispatch_as_root`.
pub type TreasuryAdminOrigin = EitherOfDiverse<EnsureRoot<AccountId>, AdminOrigin>;

parameter_types! {
    // Half a block: a motion can never exhaust a block on its own.
    pub MaxProposalWeight: Weight = Perbill::from_percent(50) * RuntimeBlockWeights::get().max_block;
}

/// Maximum number of PoA council members.
pub const MAX_COUNCIL_MEMBERS: u32 = 16;

impl pallet_collective::Config<pallet_collective::Instance1> for Runtime {
    type RuntimeOrigin = RuntimeOrigin;
    type Proposal = RuntimeCall;
    type RuntimeEvent = RuntimeEvent;
    /// Genesis parameter of `PoaAdmin`: 7 days on live chains, 20 blocks on development chains.
    type MotionDuration = pallet_poa_admin::MotionDurationOf<Runtime>;
    type MaxProposals = ConstU32<32>;
    type MaxMembers = ConstU32<MAX_COUNCIL_MEMBERS>;
    /// No prime member is ever set, so a member who does not vote counts as a no.
    type DefaultVote = pallet_collective::PrimeDefaultVote;
    type WeightInfo = pallet_collective::weights::SubstrateWeight<Runtime>;
    /// Members change only through `PoaAdmin::set_members`, which keeps the threshold valid.
    type SetMembersOrigin = EnsureNever<()>;
    type MaxProposalWeight = MaxProposalWeight;
    type DisapproveOrigin = AdminOrigin;
    type KillOrigin = AdminOrigin;
    type Consideration = ();
}

impl pallet_poa_admin::Config for Runtime {
    type RuntimeEvent = RuntimeEvent;
    type RuntimeCall = RuntimeCall;
    type AdminOrigin = AdminOrigin;
    type RootCallFilter = HolderTreasuryLock;
    type WeightInfo = pallet_poa_admin::weights::SubstrateWeight<Runtime>;
}
