//! Pallet configuration.

use frame_support::{
    derive_impl, parameter_types,
    traits::{ConstBool, ConstU8, ConstU32, ConstU64, ConstU128, FindAuthor, VariantCountOf},
    weights::{
        ConstantMultiplier, IdentityFee, Weight,
        constants::{RocksDbWeight, WEIGHT_REF_TIME_PER_SECOND},
    },
};
use frame_system::limits::{BlockLength, BlockWeights};
use pallet_transaction_payment::{ConstFeeMultiplier, FungibleAdapter, Multiplier};
use sp_runtime::{FixedU128, Perbill, traits::IdentityLookup, traits::One};

// `derive_impl` expands to associated types that name these items.
use super::{
    ATC, AccountId, AuraPq, Balance, Balances, Block, BlockNumber, EXISTENTIAL_DEPOSIT, Emission,
    Hash, MILLISECS_PER_BLOCK, Nonce, PalletInfo, RandomnessCr, Runtime, RuntimeCall, RuntimeEvent,
    RuntimeFreezeReason, RuntimeHoldReason, RuntimeOrigin, RuntimeTask, StakingPos, Timestamp,
    TreasuryDual, VERSION, ValidatorSet,
};
use crate::evm_filter::EvmOnly;
use crate::holder_lock::HolderTreasuryLock;
use frame_support::traits::InsideBoth;

/// The runtime's call filter: the holder-treasury lock (D42) and the EVM-only rule (m4-evm).
/// Used as the base filter and as the filter of `PoaAdmin::dispatch_as_root`, because Root
/// bypasses the base filter.
pub type RuntimeCallFilter = InsideBoth<HolderTreasuryLock, EvmOnly>;
use ac_primitives::Blake3Hasher;
use ac_primitives::emission::{FLOOR_BATCH_BLOCKS, FLOOR_VESTING_BLOCKS};
use frame_support::traits::EitherOfDiverse;
use frame_support::traits::fungible::{Balanced, Credit};
use frame_support::traits::{Imbalance, OnUnbalanced};
use frame_system::{EnsureNever, EnsureRoot, EnsureSigned};

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
    type BaseCallFilter = RuntimeCallFilter;
    // Every account gets its EVM address mapping when it is created (m4-evm design D4).
    type OnNewAccount = pallet_revive::AutoMapper<Runtime>;
    type OnKilledAccount = pallet_revive::AutoMapper<Runtime>;
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
    pub(crate) fn author() -> Option<AccountId> {
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
    type Staking = StakingPos;
    /// The PoA administration manages the roster until the switch (design D7 of `m3-pos`).
    type AdminOrigin = TreasuryAdminOrigin;
    type MaxAuthorities = ConstU32<{ ac_primitives::aura_pq::MAX_AUTHORITIES }>;
    /// Sets of the last eight epochs stay available for double-signing evidence.
    type HistoryEpochs = ConstU32<8>;
    type WeightInfo = pallet_validator_set::weights::SubstrateWeight<Runtime>;
}

impl pallet_ac_offences::Config for Runtime {
    type RuntimeEvent = RuntimeEvent;
    type ValidatorSet = ValidatorSet;
    type Slots = AuraPq;
    /// Slashes and burns the offender's self-stake (`m3-pos`); PoA authorities without stake
    /// lose nothing.
    type SlashHandler = StakingPos;
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
    /// Verified market work: reports that matured in the epoch (`pallet-work`); verified public
    /// work: accepted units that matured in the epoch (`pallet-public-jobs`).
    type WorkSource = MarketAndPublicWork;
    /// PoA: the security budget rolls over (decision D19); PoS: paid by work points to
    /// validators and their stakers (`m3-pos`).
    type SecurityBudget = StakingPos;
    type Treasury = TreasuryDual;
    /// The market share goes to the settlement pot, claimed by work.
    type MarketPayout = crate::Work;
    /// The public share goes to the public jobs' payout account, claimed by work.
    type PublicPayout = crate::PublicJobs;
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
    type RootCallFilter = RuntimeCallFilter;
    type WeightInfo = pallet_poa_admin::weights::SubstrateWeight<Runtime>;
}

impl pallet_staking_pos::Config for Runtime {
    type RuntimeEvent = RuntimeEvent;
    type RuntimeHoldReason = RuntimeHoldReason;
    type Currency = Balances;
    type Epochs = ValidatorSet;
    type Author = AuraPq;
    type Reveals = RandomnessCr;
    /// Slashed stake is burned and counted in `Emission::TotalBurned`.
    type Slash = Emission;
    /// A full live-chain queue (about 12,000 payouts) drains in about 50 blocks.
    type PayoutsPerBlock = ConstU32<256>;
    /// Live-chain caps (design D10 of `m3-pos`); presets may set smaller ones.
    type MaxCandidates = ConstU32<500>;
    /// The worst-case election (500 candidates, 750 nominators with 16 targets each, K = 100)
    /// fits one block's execution budget (task 3.4, issue I-005).
    type MaxNominators = ConstU32<750>;
    /// Unbonding requests per account before they merge into the latest one.
    type MaxUnlocking = ConstU32<32>;
    /// Storage bound of the active-set size K (I-004: K is a parameter, never hard-coded).
    type MaxWinners = ConstU32<1_000>;
    type WeightInfo = pallet_staking_pos::weights::SubstrateWeight<Runtime>;
}

/// Storage deposit of `items` storage items and `bytes` bytes: 0.01 ATC per item plus
/// 0.0001 ATC per byte (m4-evm design D2). Widening `u32 -> u128` casts cannot truncate.
pub const fn contract_deposit(items: u32, bytes: u32) -> Balance {
    (items as Balance)
        .saturating_mul(ATC / 100)
        .saturating_add((bytes as Balance).saturating_mul(ATC / 10_000))
}

parameter_types! {
    pub const DepositPerByte: Balance = contract_deposit(0, 1);
    pub const DepositPerItem: Balance = contract_deposit(1, 0);
    pub const DepositPerChildTrieItem: Balance = contract_deposit(1, 0);
    pub const CodeHashLockupDepositPercent: Perbill = Perbill::from_percent(30);
    pub const MaxEthExtrinsicWeight: FixedU128 = FixedU128::from_rational(9, 10);
}

/// `block.coinbase`: the account of the block's Aura-PQ author, the same account that receives
/// the author's share of the fees.
pub struct AuraPqAuthor;

impl FindAuthor<AccountId> for AuraPqAuthor {
    fn find_author<'a, I>(_digests: I) -> Option<AccountId>
    where
        I: 'a + IntoIterator<Item = (frame_support::ConsensusEngineId, &'a [u8])>,
    {
        DealWithFees::author()
    }
}

/// The currency `pallet-revive` sees: `Balances`, except that amounts revive would mint are paid
/// by the transaction signer and burns go through `Emission` (m4-evm design D5).
pub type ReviveCurrency = pallet_evm_support::ReviveCurrency<
    Balances,
    pallet_evm_support::CurrentPayer<Runtime>,
    Emission,
>;

impl pallet_revive::Config for Runtime {
    type Time = Timestamp;
    type Balance = Balance;
    type Currency = ReviveCurrency;
    type OnBurn = Emission;
    type RuntimeEvent = RuntimeEvent;
    type RuntimeCall = RuntimeCall;
    type RuntimeOrigin = RuntimeOrigin;
    type RuntimeHoldReason = RuntimeHoldReason;
    type WeightInfo = pallet_revive::weights::SubstrateWeight<Self>;
    // `pq_verify`, `blake3` and the reserved `stark_verify` address (design D6).
    type Precompiles = pallet_evm_support::PqPrecompiles<Self>;
    type FindAuthor = AuraPqAuthor;
    type DepositPerByte = DepositPerByte;
    type DepositPerItem = DepositPerItem;
    type DepositPerChildTrieItem = DepositPerChildTrieItem;
    type CodeHashLockupDepositPercent = CodeHashLockupDepositPercent;
    type AddressMapper = pallet_revive::AccountId32Mapper<Self>;
    // EVM bytecode only: PolkaVM uploads are refused by the call filter (design D3).
    type AllowEVMBytecode = ConstBool<true>;
    type UploadOrigin = EnsureSigned<AccountId>;
    type InstantiateOrigin = EnsureSigned<AccountId>;
    type RuntimeMemory = ConstU32<{ 128 * 1024 * 1024 }>;
    type PVFMemory = ConstU32<{ 512 * 1024 * 1024 }>;
    type ChainId = ConstU64<{ ac_primitives::evm::EVM_CHAIN_ID }>;
    // ATC and wei both have 18 decimals: one wei is one smallest ATC unit, so there is no dust.
    type NativeToEthRatio = ConstU32<1>;
    // Contract transactions are native transactions: fees go through `pallet-transaction-payment`
    // and `DealWithFees`, not through revive's Ethereum fee emulation.
    type FeeInfo = ();
    type Deposit = ();
    type MaxEthExtrinsicWeight = MaxEthExtrinsicWeight;
    type DebugEnabled = ConstBool<false>;
    type AutoMap = ConstBool<true>;
    type GasScale = ConstU32<1>;
}

impl pallet_evm_support::Config for Runtime {
    type WeightInfo = pallet_evm_support::weights::SubstrateWeight<Runtime>;
}

// ---- Inference market (m5-market-registry) ----

impl pallet_ref_rate::Config for Runtime {
    type RuntimeEvent = RuntimeEvent;
    /// The PoA administration sets the rate (design D3; M8 moves it to token governance).
    type AdminOrigin = AdminOrigin;
    type WeightInfo = pallet_ref_rate::weights::SubstrateWeight<Runtime>;
}

impl pallet_model_registry::Config for Runtime {
    type RuntimeEvent = RuntimeEvent;
    type RuntimeHoldReason = RuntimeHoldReason;
    type Currency = Balances;
    /// The contract storage formula (m4-evm design D2): 0.01 ATC per item, 0.0001 ATC per byte.
    type DepositPerItem = ConstU128<{ contract_deposit(1, 0) }>;
    type DepositPerByte = ConstU128<{ contract_deposit(0, 1) }>;
    type WeightInfo = pallet_model_registry::weights::SubstrateWeight<Runtime>;
}

impl pallet_providers::Config for Runtime {
    type RuntimeEvent = RuntimeEvent;
    type RuntimeHoldReason = RuntimeHoldReason;
    type Currency = Balances;
    type Models = crate::ModelRegistry;
    type Price = crate::RefRate;
    /// Slashed stake is burned and counted in `Emission::TotalBurned`.
    type Slash = Emission;
    type OnJail = crate::Work;
    type WeightInfo = pallet_providers::weights::SubstrateWeight<Runtime>;
    #[cfg(feature = "runtime-benchmarks")]
    type BenchmarkHelper = MarketBenchmarkHelper;
}

impl pallet_gateways::Config for Runtime {
    type RuntimeEvent = RuntimeEvent;
    type RuntimeHoldReason = RuntimeHoldReason;
    type Currency = Balances;
    type Price = crate::RefRate;
    type WeightInfo = pallet_gateways::weights::SubstrateWeight<Runtime>;
    #[cfg(feature = "runtime-benchmarks")]
    type BenchmarkHelper = MarketBenchmarkHelper;
}

impl pallet_credits::Config for Runtime {
    type RuntimeEvent = RuntimeEvent;
    type RuntimeHoldReason = RuntimeHoldReason;
    type Currency = Balances;
    type Gateways = crate::Gateways;
    type Keys = PqAccountKeys;
    type Price = crate::RefRate;
    type WeightInfo = pallet_credits::weights::SubstrateWeight<Runtime>;
    #[cfg(feature = "runtime-benchmarks")]
    type BenchmarkHelper = MarketBenchmarkHelper;
}

parameter_types! {
    /// Derives the settlement pot the market work emission is minted to.
    pub const WorkPalletId: frame_support::PalletId = frame_support::PalletId(*b"ac/work0");
}

impl pallet_work::Config for Runtime {
    type RuntimeEvent = RuntimeEvent;
    type RuntimeHoldReason = RuntimeHoldReason;
    type Currency = Balances;
    type Credit = crate::Credits;
    type Gateways = crate::Gateways;
    type Providers = crate::Providers;
    type Price = crate::RefRate;
    type Epochs = Emission;
    /// Burned shares and the burn ratio go through `Emission`, counted in `TotalBurned`.
    type Burn = Emission;
    type PalletId = WorkPalletId;
    type WeightInfo = pallet_work::weights::SubstrateWeight<Runtime>;
    #[cfg(feature = "runtime-benchmarks")]
    type BenchmarkHelper = MarketBenchmarkHelper;
}

parameter_types! {
    /// Audit rounds whose verdicts and used request IDs are kept: 7 days of 20-minute rounds
    /// (spec `market/audit` "计数与查询": at least 7 days; m6-auditor-agent design D10).
    pub const AuditRetentionRounds: u32 = 504;
    /// Most verdicts per round: 2 per provider for up to 10,000 providers.
    pub const AuditMaxVerdictsPerRound: u32 = 20_000;
    /// Entries pruned per block (benchmarked weight `prune(200)`).
    pub const AuditPruneLimit: u32 = 200;
}

/// Seeds audit rounds from the chain's commit-reveal randomness.
pub struct AuditRandomness;
impl pallet_audit::AuditRandomness for AuditRandomness {
    fn random(subject: &[u8]) -> Option<sp_core::H256> {
        RandomnessCr::random(subject).map(|(_, value)| value)
    }
}

impl pallet_audit::Config for Runtime {
    type RuntimeEvent = RuntimeEvent;
    type RuntimeHoldReason = RuntimeHoldReason;
    type Currency = Balances;
    /// The only caller of `ProviderPenalty` (slash and jail).
    type Providers = crate::Providers;
    type Gateways = crate::Gateways;
    type Keys = PqAccountKeys;
    type Price = crate::RefRate;
    type Randomness = AuditRandomness;
    /// Slashed auditor stake is burned and counted in `Emission::TotalBurned`.
    type Slash = Emission;
    type AdminOrigin = TreasuryAdminOrigin;
    type RetentionRounds = AuditRetentionRounds;
    type MaxVerdictsPerRound = AuditMaxVerdictsPerRound;
    type PruneLimit = AuditPruneLimit;
    type WeightInfo = pallet_audit::weights::SubstrateWeight<Runtime>;
    #[cfg(feature = "runtime-benchmarks")]
    type BenchmarkHelper = MarketBenchmarkHelper;
}

/// Verified work of an epoch: the market's from `pallet-work`, the public from
/// `pallet-public-jobs`.
pub struct MarketAndPublicWork;
impl ac_primitives::emission::WorkSource for MarketAndPublicWork {
    fn verified_work(epoch: ac_primitives::emission::EpochIndex) -> (u128, u128) {
        use ac_primitives::emission::WorkSource;
        (
            <crate::Work as WorkSource>::verified_work(epoch).0,
            <crate::PublicJobs as WorkSource>::verified_work(epoch).1,
        )
    }
}

parameter_types! {
    /// Settled unit records pruned per block (benchmarked weight `prune(64)`).
    pub const PublicJobsPruneLimit: u32 = 64;
}

/// Seeds public job rounds from the chain's commit-reveal randomness.
pub struct PublicJobsRandomness;
impl pallet_public_jobs::JobsRandomness for PublicJobsRandomness {
    fn random(subject: &[u8]) -> Option<sp_core::H256> {
        RandomnessCr::random(subject).map(|(_, value)| value)
    }
}

impl pallet_public_jobs::Config for Runtime {
    type RuntimeEvent = RuntimeEvent;
    type RuntimeHoldReason = RuntimeHoldReason;
    type Currency = Balances;
    type Models = crate::ModelRegistry;
    type Price = crate::RefRate;
    type Randomness = PublicJobsRandomness;
    type Epochs = Emission;
    /// Slashed locked rewards are burned and counted in `Emission::TotalBurned`.
    type Burn = Emission;
    /// The PoA administration publishes jobs until holder voting (M8).
    type PublisherOrigin = TreasuryAdminOrigin;
    type AdminOrigin = TreasuryAdminOrigin;
    type PruneLimit = PublicJobsPruneLimit;
    type WeightInfo = pallet_public_jobs::weights::SubstrateWeight<Runtime>;
    #[cfg(feature = "runtime-benchmarks")]
    type BenchmarkHelper = MarketBenchmarkHelper;
}

/// Accounts' current keys, from `pallet-pq-accounts`: the fingerprint vouchers are checked
/// against (`ac_primitives::market::voucher::key_fingerprint` computes the same value).
pub struct PqAccountKeys;
impl ac_primitives::market::traits::AccountKeys<AccountId> for PqAccountKeys {
    fn key_fingerprint(who: &AccountId) -> Option<[u8; 32]> {
        pallet_pq_accounts::Keys::<Runtime>::get(who)
            .map(|r| pallet_pq_accounts::key_fingerprint(&r.public_key))
    }
}

/// Market benchmark setup: registered models, keys and gateways, and a reference rate, written
/// straight into storage.
#[cfg(feature = "runtime-benchmarks")]
pub struct MarketBenchmarkHelper;

#[cfg(feature = "runtime-benchmarks")]
impl MarketBenchmarkHelper {
    fn put_rate(rate: u128) {
        pallet_ref_rate::Rate::<Runtime>::put((ac_primitives::market::AtcPerUsd(rate), 0));
    }
}

#[cfg(feature = "runtime-benchmarks")]
impl pallet_providers::BenchmarkHelper for MarketBenchmarkHelper {
    fn register_model(id: ac_primitives::market::ModelId) {
        use ac_primitives::market::model::QuantType;
        let Ok(manifest) = ac_primitives::market::ModelManifest::new(
            b"bench",
            b"bench",
            QuantType::Bf16,
            alloc::vec![[1; 32]],
        ) else {
            return;
        };
        pallet_model_registry::Models::<Runtime>::insert(
            id,
            pallet_model_registry::RecordOf::<Runtime> {
                owner: AccountId::new([0; 32]),
                manifest,
                lineage: None,
                license_tag: Default::default(),
                royalty: None,
                deposit: 0,
                registered_at: 0,
            },
        );
    }
    fn set_rate(rate: u128) {
        Self::put_rate(rate);
    }
}

#[cfg(feature = "runtime-benchmarks")]
impl pallet_gateways::BenchmarkHelper for MarketBenchmarkHelper {
    fn set_rate(rate: u128) {
        Self::put_rate(rate);
    }
}

#[cfg(feature = "runtime-benchmarks")]
impl pallet_work::BenchmarkHelper for MarketBenchmarkHelper {
    fn prepare_gateway(gateway: &AccountId) {
        use ac_primitives::market::records::GatewayStatus;
        use frame_support::traits::fungible::Mutate;
        pallet_gateways::Gateways::<Runtime>::insert(
            gateway,
            pallet_gateways::RecordOf::<Runtime> {
                endpoint: frame_support::BoundedVec::truncate_from(b"bench".to_vec()),
                fee_bps: 500,
                stake: 0,
                unlocking: Default::default(),
                status: GatewayStatus::Active,
                registered_at: 0,
            },
        );
        Balances::set_balance(gateway, 1_000_000 * crate::ATC);
        Self::put_rate(crate::ATC); // 1 crate::ATC per dollar
    }

    fn prepare_provider(provider: &AccountId, model: &ac_primitives::market::ModelId) {
        use ac_primitives::market::records::{ModelPrice, ProviderStatus, Tier};
        use ac_primitives::market::{MicroUsd, PricePerMTok};
        use frame_support::traits::fungible::Mutate;
        let Ok(kem_pk) = ac_crypto::KemPublicKey::new(ac_crypto::KemAlg::XWing, &[7; 1216]) else {
            return;
        };
        pallet_providers::Providers::<Runtime>::insert(
            provider,
            pallet_providers::RecordOf::<Runtime> {
                tier: Tier::T2,
                endpoint: frame_support::BoundedVec::truncate_from(b"bench".to_vec()),
                kem_pk,
                models: frame_support::BoundedVec::truncate_from(alloc::vec![ModelPrice {
                    model: *model,
                    price: PricePerMTok {
                        input: MicroUsd(1),
                        output: MicroUsd(1),
                    },
                }]),
                stake: 0,
                unlocking: Default::default(),
                status: ProviderStatus::Active,
                last_heartbeat: 0,
                metrics: Default::default(),
                attestation: None,
                registered_at: 0,
            },
        );
        Balances::set_balance(provider, crate::ATC);
    }

    fn vouchers(
        gateway: &AccountId,
        first: u32,
        n: u32,
        micro_usd: u128,
    ) -> alloc::vec::Vec<ac_primitives::market::SignedVoucher> {
        use ac_primitives::market::voucher::VOUCHER_CONTEXT;
        use ac_primitives::market::{MicroUsd, SignedVoucher, VoucherBody};
        use frame_support::traits::fungible::Mutate;
        // ML-DSA-87: the largest signatures and the slowest verification.
        let Ok(seed) = ac_crypto::dev_seed("bench-user") else {
            return alloc::vec::Vec::new();
        };
        let Ok(signer) = ac_crypto::sig::SigningKey::from_seed(ac_crypto::SigAlg::MlDsa87, &seed)
        else {
            return alloc::vec::Vec::new();
        };
        let Ok(public_key) = signer.public_key() else {
            return alloc::vec::Vec::new();
        };
        let genesis = pallet_credits::Pallet::<Runtime>::genesis();
        (first..first.saturating_add(n))
            .filter_map(|i| {
                let user: AccountId = frame_benchmarking::account("work-user", i, 0);
                <Self as pallet_credits::BenchmarkHelper>::register_key(&user, &public_key);
                Balances::set_balance(&user, 1_000 * crate::ATC);
                // Escrow twice the voucher's value at 1 crate::ATC per dollar.
                let escrow = micro_usd.saturating_mul(2_000_000_000_000);
                pallet_credits::Pallet::<Runtime>::deposit(
                    RuntimeOrigin::signed(user.clone()),
                    gateway.clone(),
                    escrow,
                )
                .ok()?;
                let body = VoucherBody {
                    genesis,
                    user,
                    gateway: gateway.clone(),
                    channel: 0,
                    cumulative: MicroUsd(micro_usd),
                };
                let signature = signer
                    .sign_deterministic(&body.payload().ok()?, VOUCHER_CONTEXT)
                    .ok()?;
                Some(SignedVoucher {
                    body,
                    public_key: public_key.clone(),
                    signature,
                })
            })
            .collect()
    }

    fn set_epoch(epoch: ac_primitives::emission::EpochIndex) {
        let length = Emission::schedule().map_or(1, |s| s.epoch_length());
        let block = epoch.saturating_mul(length).saturating_add(1);
        frame_system::Pallet::<Runtime>::set_block_number(
            sp_runtime::SaturatedConversion::saturated_into(block),
        );
    }
}

#[cfg(feature = "runtime-benchmarks")]
impl pallet_credits::BenchmarkHelper for MarketBenchmarkHelper {
    fn register_key(who: &AccountId, key: &ac_crypto::PqPublicKey) {
        pallet_pq_accounts::Keys::<Runtime>::insert(
            who,
            pallet_pq_accounts::KeyRecord {
                public_key: key.clone(),
                rotations: 0,
            },
        );
    }
    fn activate_gateway(gateway: &AccountId) {
        use ac_primitives::market::records::GatewayStatus;
        pallet_gateways::Gateways::<Runtime>::insert(
            gateway,
            pallet_gateways::RecordOf::<Runtime> {
                endpoint: frame_support::BoundedVec::truncate_from(b"bench".to_vec()),
                fee_bps: 0,
                stake: 0,
                unlocking: Default::default(),
                status: GatewayStatus::Active,
                registered_at: 0,
            },
        );
    }
    fn set_rate(rate: u128) {
        Self::put_rate(rate);
    }
}

#[cfg(feature = "runtime-benchmarks")]
impl pallet_public_jobs::BenchmarkHelper for MarketBenchmarkHelper {
    fn set_rate(rate: u128) {
        Self::put_rate(rate);
    }
    fn register_model(model: ac_primitives::market::ModelId) {
        <Self as pallet_providers::BenchmarkHelper>::register_model(model);
    }
    fn set_randomness(seed: sp_core::H256) {
        pallet_randomness_cr::Latest::<Runtime>::put((0, seed));
    }
    fn set_epoch(_epoch: ac_primitives::emission::EpochIndex) {
        // The emission epoch follows the block number; benchmarks start in epoch 0.
    }
}

#[cfg(feature = "runtime-benchmarks")]
impl pallet_audit::BenchmarkHelper for MarketBenchmarkHelper {
    fn set_rate(rate: u128) {
        Self::put_rate(rate);
    }
    fn set_key(who: &AccountId, key: &ac_crypto::PqPublicKey) {
        <Self as pallet_credits::BenchmarkHelper>::register_key(who, key);
    }
    fn register_provider(
        who: &AccountId,
        model: ac_primitives::market::ModelId,
        price: ac_primitives::market::PricePerMTok,
    ) {
        <Self as pallet_work::BenchmarkHelper>::prepare_provider(who, &model);
        pallet_providers::Providers::<Runtime>::mutate(who, |p| {
            if let Some(p) = p {
                for m in p.models.iter_mut() {
                    m.price = price;
                }
            }
        });
    }
    fn register_gateway(who: &AccountId) {
        <Self as pallet_credits::BenchmarkHelper>::activate_gateway(who);
    }
    fn set_randomness(seed: sp_core::H256) {
        pallet_randomness_cr::Latest::<Runtime>::put((0, seed));
    }
}
