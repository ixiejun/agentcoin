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
    AccountId, AuraPq, Balance, Balances, Block, BlockNumber, EXISTENTIAL_DEPOSIT, Hash,
    MILLISECS_PER_BLOCK, Nonce, PalletInfo, Runtime, RuntimeCall, RuntimeEvent,
    RuntimeFreezeReason, RuntimeHoldReason, RuntimeOrigin, RuntimeTask, VERSION, ValidatorSet,
};
use ac_primitives::Blake3Hasher;

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
    type DustRemoval = ();
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

impl pallet_transaction_payment::Config for Runtime {
    type RuntimeEvent = RuntimeEvent;
    // `()` as the imbalance handler burns every fee and tip (M1: 100% burn, native-token spec).
    type OnChargeTransaction = FungibleAdapter<Balances, ()>;
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
