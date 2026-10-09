//! # Audits
//!
//! On-chain audits of the inference market (plan §5.5; m6-audit-chain; spec `market/audit`):
//!
//! - **Auditors** register with a stake in US dollars (live chains: $1,000, converted at the
//!   reference rate rounding up) that unbonds over 7 days and stays slashable until then.
//!   Provider and gateway accounts cannot register.
//! - **Rounds** of [`AuditParams::round_blocks`] blocks (live: 1,200, 20 minutes). The first block of a round
//!   stores the roster (auditors staked at or above the threshold and not exiting, in account
//!   order) and a seed from the chain's randomness. Each provider's auditors of the round are
//!   drawn from them with [`sample`]; anyone can recompute the draw.
//! - **Verdicts**: an assigned auditor submits one verdict per provider and round with a receipt
//!   that provider and gateway signed (checked here: chain, provider, keys, signatures, fee,
//!   unused request ID). A failure carries the commitment to its evidence; the evidence (prompt,
//!   answer, proofs) never goes on chain (red line 6).
//! - **Disputes**: failing verdicts of two different auditors against the same provider in this
//!   round or the previous one open a dispute. `N` reviewers drawn from the roster (live: 5)
//!   vote; `Q` votes (live: 3) decide. Confirmed: the provider is slashed (10%) and jailed
//!   through [`ProviderPenalty`], which voids its unsettled work. Rejected: each accuser is
//!   slashed (10%) and made to exit. After the deadline anyone closes an undecided dispute.
//! - **Payments**: each accepted verdict and each winning vote is paid a fixed dollar amount
//!   (live: $0.05, rounded down) from the audit pot, which the treasury floor funds ("audit"
//!   spends). The pot never mints; an empty pot skips the payment.
//! - **Statistical judgment** (m6-audit-sprt): verdicts judged by the thresholds carry
//!   [`AuditStats`]; each adds its log-likelihood ratio to the provider's CUSUM
//!   ([`SprtState`]). When enabled, a state at the bound opens a statistical dispute, decided
//!   like the others; a rejected one slashes no auditor. Verdicts a state or an open dispute
//!   still refers to outlive their retention until it lets them go.
//!
//! No call slashes, jails or decides a dispute directly, and the administration only adjusts
//! the stake threshold, the payment, the accepted thresholds version and the statistical
//! judgment's version and switch, within guardrails (D6).
//!
//! [`sample`]: ac_primitives::market::audit::sample
//! [`AuditStats`]: ac_primitives::market::audit::AuditStats
//! [`SprtState`]: ac_primitives::market::audit::SprtState
//! [`ProviderPenalty`]: ac_primitives::market::traits::ProviderPenalty

#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

// Deliberate glob re-export (G.MOD.03 deviation): `construct_runtime` resolves hidden items that
// the `#[pallet]` macro generates inside `pallet`, so they cannot be listed explicitly.
pub use pallet::*;

/// Runs the README example as a doctest.
#[cfg(doctest)]
#[doc = include_str!("../README.md")]
pub struct ReadmeDoctests;

#[cfg(feature = "runtime-benchmarks")]
mod benchmarking;
pub mod migrations;
#[cfg(test)]
mod mock;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_stats;
pub mod weights;

pub use weights::WeightInfo;

/// An auditor's evidence endpoint and the X-Wing key reviewers seal their requests to.
pub type AuditorEndpoint = (
    ac_primitives::market::records::Endpoint,
    ac_crypto::KemPublicKey,
);

/// Most open disputes one `open_disputes` query returns.
pub const MAX_OPEN_DISPUTES_PAGE: u32 = 256;

/// Most statistical states of an older version removed per block after a version change: each
/// may release up to `MAX_SPRT_ENTRIES` retained verdict lists, so a few fill a block's share.
pub const STALE_CLEAR_LIMIT: u32 = 2;

use ac_primitives::market::audit::{
    AdjustableParams, AuditParams, CURRENT_STATS, StatsConfig, VerdictStats,
};
use ac_primitives::market::{MicroUsd, SignedReceipt};
use parity_scale_codec::{Decode, DecodeWithMemTracking, Encode};
use scale_info::TypeInfo;
use sp_core::H256;
use sp_runtime::{AccountId32, Perbill};

/// Audit parameters as written in a chain specification (design D10). Slash ratios are whole
/// percentages.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuditGenesis {
    /// Blocks per round.
    pub round_blocks: u32,
    /// Auditors drawn per provider and round.
    pub assign: u8,
    /// Reviewers of a dispute.
    pub reviewers: u8,
    /// Votes that decide a dispute.
    pub quorum: u8,
    /// Blocks reviewers have to vote.
    pub vote_blocks: u32,
    /// Percent of a confirmed provider's stake slashed.
    pub provider_slash_percent: u8,
    /// Percent of a rejected accuser's stake slashed.
    pub auditor_slash_percent: u8,
    /// Blocks an auditor's stake unbonds for.
    pub unbond_blocks: u32,
    /// Auditor stake threshold.
    pub stake_usd: MicroUsd,
    /// Payment per accepted verdict or winning vote.
    pub payment_usd: MicroUsd,
    /// Re-check thresholds version verdicts must use.
    pub thresholds_version: u16,
    /// The statistical judgment's parameter version verdicts must use (m6-audit-sprt design
    /// D10).
    #[serde(default = "AuditGenesis::current_stats_version")]
    pub stats_version: u16,
    /// Whether the statistical judgment opens disputes (live chains: off until calibrated
    /// hardware covers them).
    #[serde(default)]
    pub stats_enabled: bool,
}

impl AuditGenesis {
    /// Draft values of live chains.
    pub const LIVE: Self = Self {
        round_blocks: 1_200,
        assign: 2,
        reviewers: 5,
        quorum: 3,
        vote_blocks: 600,
        provider_slash_percent: 10,
        auditor_slash_percent: 10,
        unbond_blocks: 604_800,
        stake_usd: MicroUsd(1_000_000_000),
        payment_usd: MicroUsd(50_000),
        // ac_market_proto::AUDIT_THRESHOLDS (m6-toploc-gpu-calibration: calibrated single-audit bounds).
        thresholds_version: 4,
        stats_version: CURRENT_STATS.version,
        stats_enabled: false,
    };

    fn current_stats_version() -> u16 {
        CURRENT_STATS.version
    }

    /// The statistical judgment's configuration.
    #[must_use]
    pub fn stats(&self) -> StatsConfig {
        StatsConfig {
            version: self.stats_version,
            enabled: self.stats_enabled,
        }
    }

    /// The fixed and the adjustable parameters.
    #[must_use]
    pub fn split(&self) -> (AuditParams, AdjustableParams) {
        (
            AuditParams {
                round_blocks: self.round_blocks,
                assign: self.assign,
                reviewers: self.reviewers,
                quorum: self.quorum,
                vote_blocks: self.vote_blocks,
                provider_slash: Perbill::from_percent(u32::from(self.provider_slash_percent)),
                auditor_slash: Perbill::from_percent(u32::from(self.auditor_slash_percent)),
                unbond_blocks: self.unbond_blocks,
            },
            AdjustableParams {
                stake_usd: self.stake_usd,
                payment_usd: self.payment_usd,
                thresholds_version: self.thresholds_version,
            },
        )
    }
}

impl Default for AuditGenesis {
    fn default() -> Self {
        Self::LIVE
    }
}

/// A verdict as submitted (one argument instead of six, G.FUD.01).
#[derive(Clone, Debug, PartialEq, Eq, Encode, Decode, DecodeWithMemTracking, TypeInfo)]
pub struct VerdictSubmission {
    /// The audited provider.
    pub provider: AccountId32,
    /// The current round (verdicts are accepted only in the round they were assigned in).
    pub round: ac_primitives::market::audit::RoundIndex,
    /// The outcome.
    pub outcome: ac_primitives::market::audit::VerdictOutcome,
    /// The re-check thresholds version used.
    pub thresholds_version: u16,
    /// Commitment to the evidence: required for a failure, absent otherwise.
    pub evidence: Option<[u8; 32]>,
    /// The receipt of the audited request, signed by provider and gateway.
    pub receipt: SignedReceipt,
    /// The statistics: required for a verdict judged by the thresholds, absent otherwise.
    pub stats: Option<VerdictStats>,
}

/// What the benchmarks need from the runtime: a rate, account keys, providers with prices and
/// gateways, randomness and pot funds.
#[cfg(feature = "runtime-benchmarks")]
pub trait BenchmarkHelper {
    /// Sets the reference rate to `rate` smallest units per dollar.
    fn set_rate(rate: u128);
    /// Makes `key` the account key of `who`.
    fn set_key(who: &AccountId32, key: &ac_crypto::PqPublicKey);
    /// Registers `who` as a provider of `model` at `price`.
    fn register_provider(
        who: &AccountId32,
        model: ac_primitives::market::ModelId,
        price: ac_primitives::market::PricePerMTok,
    );
    /// Registers `who` as a gateway.
    fn register_gateway(who: &AccountId32);
    /// Makes randomness available.
    fn set_randomness(seed: H256);
}

/// Randomness the rounds are seeded from.
pub trait AuditRandomness {
    /// A value derived from the latest randomness for `subject`, `None` before the first.
    fn random(subject: &[u8]) -> Option<H256>;
}

// FRAME's `pallet` macro expands to code (genesis defaults, storage metadata, call decoding)
// that uses `expect` / `unreachable!` on our spans; hand-written code here uses neither except
// the documented genesis check.
#[allow(clippy::expect_used, clippy::unreachable)]
#[frame_support::pallet]
pub mod pallet {
    use super::{
        AuditGenesis, AuditRandomness, AuditorEndpoint, MAX_OPEN_DISPUTES_PAGE, STALE_CLEAR_LIMIT,
        VerdictSubmission, WeightInfo,
    };
    use ac_crypto::KemAlg;
    use ac_primitives::market::audit::{
        Accuser, AdjustableParams, AuditParams, AuditorRecord, AuditorStats, AuditorStatus,
        DisputeKind, DisputeOutcome, DisputeRecord, Draw, MAX_ASSIGN, MAX_AUDITORS,
        MAX_DISPUTE_ACCUSERS, MAX_REVIEWERS, Pool, ProviderAuditStats, ROUND_SEED_SUBJECT,
        RoundIndex, SprtEntry, SprtState, StatsConfig, VerdictOutcome, VerdictRecord, VerdictStats,
        Vote, round_of, round_start, sample, stats_params,
    };
    use ac_primitives::market::receipt::{ReceiptContext, check_receipt};
    use ac_primitives::market::records::{
        UnlockingList, schedule_unlock, take_due, take_for_slash, unlocking_total,
    };
    use ac_primitives::market::traits::{
        AccountKeys, GatewayLookup, PriceSource, ProviderAudit, ProviderPenalty,
    };
    use ac_primitives::market::usd::Rounding;
    use ac_primitives::market::{PriceError, ReceiptError};
    use alloc::vec::Vec;
    use frame_support::PalletId;
    use frame_support::pallet_prelude::{
        BuildGenesisConfig, ConstU32, DispatchResult, Hooks, IsType, OptionQuery, StorageDoubleMap,
        StorageMap, StorageValue, StorageVersion, ValueQuery, Weight, ensure,
    };
    use frame_support::traits::fungible::{
        BalancedHold, Credit, Inspect, InspectHold, Mutate, MutateHold,
    };
    use frame_support::traits::tokens::{Precision, Preservation};
    use frame_support::traits::{EnsureOrigin, Get, Imbalance, OnUnbalanced};
    use frame_support::{BoundedVec, Identity, Twox64Concat};
    use frame_system::pallet_prelude::{BlockNumberFor, OriginFor};
    use parity_scale_codec::Encode;
    use sp_core::H256;
    use sp_runtime::traits::{AccountIdConversion, Zero};
    use sp_runtime::{AccountId32, SaturatedConversion, Saturating};

    /// Balance of the native currency (smallest ATC units).
    pub type Balance = u128;
    /// An auditor record of this runtime.
    pub type AuditorOf<T> = AuditorRecord<BlockNumberFor<T>>;
    /// A dispute record of this runtime.
    pub type DisputeOf<T> = DisputeRecord<AccountId32, BlockNumberFor<T>>;
    /// A round's roster.
    pub type Roster = BoundedVec<AccountId32, ConstU32<MAX_AUDITORS>>;
    /// A provider's statistical state and the parameter version it was built with.
    pub type VersionedSprt = (u16, SprtState<AccountId32>);
    /// Where [`clear_stale_step`](Pallet::clear_stale_step) resumes: a raw storage key.
    pub type ClearCursor = BoundedVec<u8, ConstU32<128>>;
    /// The verdicts on one provider in one round.
    pub type VerdictList = BoundedVec<VerdictRecord<AccountId32>, ConstU32<{ MAX_ASSIGN as u32 }>>;

    /// The audit pot's identifier: its account is derived from it.
    pub const POT_ID: PalletId = PalletId(*b"ac/audit");

    /// Storage version 1 adds the statistics to verdicts and the kind to disputes
    /// (m6-audit-sprt, [`crate::migrations::v1`]).
    pub const STORAGE_VERSION: StorageVersion = StorageVersion::new(1);

    #[pallet::pallet]
    #[pallet::storage_version(STORAGE_VERSION)]
    pub struct Pallet<T>(_);

    #[pallet::config]
    pub trait Config: frame_system::Config<AccountId = AccountId32, Hash = H256> {
        /// The overarching event type.
        #[allow(deprecated)] // Kept explicit for compatibility with the SDK's test tooling.
        type RuntimeEvent: From<Event<Self>> + IsType<<Self as frame_system::Config>::RuntimeEvent>;
        /// The overarching hold reason.
        type RuntimeHoldReason: From<HoldReason>;
        /// The native currency; stake is held with [`HoldReason::Stake`].
        type Currency: Inspect<AccountId32, Balance = Balance>
            + Mutate<AccountId32>
            + InspectHold<AccountId32, Reason = Self::RuntimeHoldReason>
            + MutateHold<AccountId32, Reason = Self::RuntimeHoldReason>
            + BalancedHold<AccountId32, Reason = Self::RuntimeHoldReason>;
        /// Providers: registration, prices, and the slash and jail interface (only this pallet
        /// calls it).
        type Providers: ProviderAudit<AccountId32> + ProviderPenalty<AccountId32, Balance>;
        /// Gateways.
        type Gateways: GatewayLookup<AccountId32>;
        /// Account keys, to check receipt signatures against.
        type Keys: AccountKeys<AccountId32>;
        /// The reference rate.
        type Price: PriceSource;
        /// The chain's randomness.
        type Randomness: AuditRandomness;
        /// Where slashed auditor stake goes; the runtime burns it through `Emission`.
        type Slash: OnUnbalanced<Credit<AccountId32, Self::Currency>>;
        /// Who may adjust the stake threshold, the payment and the thresholds version.
        type AdminOrigin: EnsureOrigin<Self::RuntimeOrigin>;
        /// Rounds verdicts and used request IDs are kept for.
        #[pallet::constant]
        type RetentionRounds: Get<u32>;
        /// Most verdicts accepted per round.
        #[pallet::constant]
        type MaxVerdictsPerRound: Get<u32>;
        /// Most storage entries removed per block when pruning old rounds.
        #[pallet::constant]
        type PruneLimit: Get<u32>;
        /// Weights.
        type WeightInfo: WeightInfo;
        /// Sets up rates, keys, providers and randomness for the benchmarks.
        #[cfg(feature = "runtime-benchmarks")]
        type BenchmarkHelper: super::BenchmarkHelper;
    }

    /// Why this pallet holds funds.
    #[pallet::composite_enum]
    pub enum HoldReason {
        /// Bonded or unbonding auditor stake.
        #[codec(index = 0)]
        Stake,
    }

    /// Parameters fixed at genesis.
    #[pallet::storage]
    pub type Params<T: Config> = StorageValue<_, AuditParams, OptionQuery>;

    /// Parameters the administration adjusts within guardrails.
    #[pallet::storage]
    pub type Adjustable<T: Config> = StorageValue<_, AdjustableParams, OptionQuery>;

    /// Registered auditors.
    #[pallet::storage]
    pub type Auditors<T: Config> = StorageMap<_, Identity, AccountId32, AuditorOf<T>, OptionQuery>;

    /// Number of registered auditors.
    #[pallet::storage]
    pub type AuditorCount<T: Config> = StorageValue<_, u32, ValueQuery>;

    /// The last round whose first block ran.
    #[pallet::storage]
    pub type LastRound<T: Config> = StorageValue<_, RoundIndex, OptionQuery>;

    /// Rosters of the current and the previous round.
    #[pallet::storage]
    pub type Rosters<T: Config> = StorageMap<_, Twox64Concat, RoundIndex, Roster, OptionQuery>;

    /// Seeds of the current and the previous round (absent without randomness).
    #[pallet::storage]
    pub type Seeds<T: Config> = StorageMap<_, Twox64Concat, RoundIndex, H256, OptionQuery>;

    /// Verdicts by round and provider.
    #[pallet::storage]
    pub type Verdicts<T: Config> = StorageDoubleMap<
        _,
        Twox64Concat,
        RoundIndex,
        Identity,
        AccountId32,
        VerdictList,
        ValueQuery,
    >;

    /// Request IDs used by a verdict, with the round of the verdict.
    #[pallet::storage]
    pub type UsedRequests<T: Config> = StorageMap<_, Identity, [u8; 32], RoundIndex, OptionQuery>;

    /// Request IDs used in each round, to prune [`UsedRequests`].
    #[pallet::storage]
    pub type RoundRequests<T: Config> =
        StorageDoubleMap<_, Twox64Concat, RoundIndex, Identity, [u8; 32], (), OptionQuery>;

    /// Verdicts accepted in each round (bounded by [`Config::MaxVerdictsPerRound`]).
    #[pallet::storage]
    pub type RoundVerdicts<T: Config> = StorageMap<_, Twox64Concat, RoundIndex, u32, ValueQuery>;

    /// The oldest round not yet pruned.
    #[pallet::storage]
    pub type PruneNext<T: Config> = StorageValue<_, RoundIndex, ValueQuery>;

    /// The open dispute of each provider.
    #[pallet::storage]
    pub type OpenDispute<T: Config> = StorageMap<_, Identity, AccountId32, u64, OptionQuery>;

    /// Disputes, open and closed (rare; kept).
    #[pallet::storage]
    pub type Disputes<T: Config> = StorageMap<_, Twox64Concat, u64, DisputeOf<T>, OptionQuery>;

    /// Identifier of the next dispute.
    #[pallet::storage]
    pub type NextDispute<T: Config> = StorageValue<_, u64, ValueQuery>;

    /// Verdict counts of providers.
    #[pallet::storage]
    pub type ProviderStats<T: Config> =
        StorageMap<_, Identity, AccountId32, ProviderAuditStats, ValueQuery>;

    /// Activity counts of auditors.
    #[pallet::storage]
    pub type Activity<T: Config> = StorageMap<_, Identity, AccountId32, AuditorStats, ValueQuery>;

    /// Where auditors serve the evidence of their failing verdicts, and the X-Wing key reviewers
    /// seal their requests to (m6-auditor-agent design D3).
    #[pallet::storage]
    pub type AuditorEndpoints<T: Config> =
        StorageMap<_, Identity, AccountId32, AuditorEndpoint, OptionQuery>;

    /// The statistical judgment's parameter version and switch (m6-audit-sprt design D10).
    #[pallet::storage]
    pub type StatsSettings<T: Config> = StorageValue<_, StatsConfig, OptionQuery>;

    /// Each provider's statistical state, with the parameter version it was built with; a state
    /// of another version than the current one counts as empty (design D6).
    #[pallet::storage]
    pub type SprtStates<T: Config> =
        StorageMap<_, Identity, AccountId32, VersionedSprt, OptionQuery>;

    /// Verdict lists past their retention that a statistical state or an open dispute still
    /// refers to, by round and provider; removed once nothing does.
    #[pallet::storage]
    pub type RetainedVerdicts<T: Config> = StorageDoubleMap<
        _,
        Twox64Concat,
        RoundIndex,
        Identity,
        AccountId32,
        VerdictList,
        OptionQuery,
    >;

    /// While set, states of an older parameter version are being removed, from this key on
    /// (empty: from the start).
    #[pallet::storage]
    pub type StaleClearing<T: Config> = StorageValue<_, ClearCursor, OptionQuery>;

    #[pallet::genesis_config]
    pub struct GenesisConfig<T: Config> {
        /// Parameters.
        pub params: AuditGenesis,
        /// Unused type marker.
        #[serde(skip)]
        pub _marker: core::marker::PhantomData<T>,
    }

    impl<T: Config> Default for GenesisConfig<T> {
        fn default() -> Self {
            Self {
                params: AuditGenesis::LIVE,
                _marker: core::marker::PhantomData,
            }
        }
    }

    #[pallet::genesis_build]
    impl<T: Config> BuildGenesisConfig for GenesisConfig<T> {
        fn build(&self) {
            let (fixed, adjustable) = self.params.split();
            let percents_ok = self.params.provider_slash_percent <= 100
                && self.params.auditor_slash_percent <= 100;
            let stats_ok = stats_params(self.params.stats_version).is_some();
            if fixed.check().is_err() || !percents_ok || !adjustable.within_bounds() || !stats_ok {
                // Genesis is built once, off chain: a bad parameter must stop the chain from
                // being created at all (spec "参数与护栏", "创世参数不满足护栏").
                #[allow(clippy::panic)]
                {
                    panic!("invalid audit genesis parameters: {:?}", self.params);
                }
            }
            Params::<T>::put(fixed);
            Adjustable::<T>::put(adjustable);
            StatsSettings::<T>::put(self.params.stats());
        }
    }

    #[pallet::event]
    #[pallet::generate_deposit(pub(super) fn deposit_event)]
    pub enum Event<T: Config> {
        /// An auditor registered.
        Registered {
            /// The auditor.
            who: AccountId32,
            /// Initial stake.
            stake: Balance,
        },
        /// Stake was added.
        Bonded {
            /// The auditor.
            who: AccountId32,
            /// Amount added.
            amount: Balance,
        },
        /// Stake started unbonding.
        Unbonding {
            /// The auditor.
            who: AccountId32,
            /// Amount.
            amount: Balance,
            /// Block it unlocks at.
            unlock_at: BlockNumberFor<T>,
        },
        /// An auditor is leaving.
        Exiting {
            /// The auditor.
            who: AccountId32,
        },
        /// Unlocked stake was released.
        Withdrawn {
            /// The auditor.
            who: AccountId32,
            /// Amount released.
            amount: Balance,
        },
        /// An exited auditor's record was removed.
        Removed {
            /// The former auditor.
            who: AccountId32,
        },
        /// A round started.
        RoundStarted {
            /// The round.
            round: RoundIndex,
            /// Auditors on its roster.
            auditors: u32,
            /// Whether it has a seed (and so assignments).
            seeded: bool,
        },
        /// A verdict was accepted.
        VerdictAccepted {
            /// Its round.
            round: RoundIndex,
            /// The audited provider.
            provider: AccountId32,
            /// The auditor.
            auditor: AccountId32,
            /// The outcome.
            outcome: VerdictOutcome,
        },
        /// A verdict or vote was paid from the pot.
        Paid {
            /// The payee.
            who: AccountId32,
            /// Amount.
            amount: Balance,
        },
        /// A payment was skipped: the pot cannot cover it or no rate is set.
        PaymentSkipped {
            /// Who would have been paid.
            who: AccountId32,
        },
        /// A dispute was opened.
        DisputeOpened {
            /// Its identifier.
            id: u64,
            /// The accused provider.
            provider: AccountId32,
            /// What opened it.
            kind: DisputeKind,
        },
        /// Two auditors failed a provider, or its statistical state reached the bound, but too
        /// few auditors are eligible to review.
        DisputeNotOpened {
            /// The provider.
            provider: AccountId32,
        },
        /// A reviewer voted.
        Voted {
            /// The dispute.
            id: u64,
            /// The reviewer.
            reviewer: AccountId32,
            /// The vote.
            vote: Vote,
        },
        /// A dispute was decided or closed.
        DisputeClosed {
            /// The dispute.
            id: u64,
            /// The provider.
            provider: AccountId32,
            /// How it ended.
            outcome: DisputeOutcome,
        },
        /// An auditor's stake was slashed and burned.
        Slashed {
            /// The auditor.
            who: AccountId32,
            /// Amount burned.
            amount: Balance,
        },
        /// Jailing a confirmed provider failed (it is no longer registered).
        JailFailed {
            /// The provider.
            provider: AccountId32,
        },
        /// The administration adjusted parameters.
        ParamsSet {
            /// The new values.
            params: AdjustableParams,
        },
        /// An auditor set or cleared its evidence endpoint.
        EndpointSet {
            /// The auditor.
            who: AccountId32,
            /// Whether an endpoint is now set.
            set: bool,
        },
        /// The administration set the statistical judgment's version and switch.
        StatsConfigSet {
            /// The new configuration.
            config: StatsConfig,
        },
    }

    #[pallet::error]
    pub enum Error<T> {
        /// The account is already an auditor.
        AlreadyRegistered,
        /// The account is not an auditor.
        NotAuditor,
        /// Providers and gateways cannot be auditors.
        MarketParticipant,
        /// The auditor limit is reached.
        TooManyAuditors,
        /// No reference rate: the dollar threshold cannot be converted.
        RateNotSet,
        /// The threshold overflows when converted.
        PriceOverflow,
        /// The stake would be below the threshold.
        BelowThreshold,
        /// The account cannot cover the stake.
        InsufficientBalance,
        /// An exiting auditor cannot change.
        Exiting,
        /// A zero amount.
        ZeroAmount,
        /// More than the bonded stake.
        AmountTooLarge,
        /// No unbonding stake is due.
        NothingToWithdraw,
        /// Genesis parameters are missing.
        NotConfigured,
        /// Verdicts are accepted only in the current round.
        WrongRound,
        /// The round has no seed and so no assignments.
        NoAssignments,
        /// The caller is not assigned to this provider in this round.
        NotAssigned,
        /// The caller already submitted a verdict on this provider in this round.
        AlreadySubmitted,
        /// The thresholds version is not the accepted one.
        WrongThresholdsVersion,
        /// A failure needs an evidence commitment and other outcomes must not have one.
        BadEvidence,
        /// The receipt is for another provider.
        ReceiptProvider,
        /// An auditor cannot audit a receipt it is a party to.
        SelfAudit,
        /// The receipt is for another chain.
        ReceiptGenesis,
        /// The receipt's job kind is not supported.
        ReceiptKind,
        /// The provider does not list the receipt's model.
        ReceiptModel,
        /// The receipt's fee does not match the provider's price.
        ReceiptFee,
        /// A receipt key is not the account's registered key.
        ReceiptKey,
        /// A receipt signature does not verify.
        ReceiptSignature,
        /// The request ID was already used by a verdict.
        RequestUsed,
        /// The round's verdict limit is reached.
        RoundFull,
        /// No such open dispute for this provider.
        NoDispute,
        /// The caller is not a reviewer of the dispute, or already voted.
        NotReviewer,
        /// The voting deadline has passed.
        DeadlinePassed,
        /// The voting deadline has not passed.
        DeadlineNotPassed,
        /// Adjusted parameters break the guardrails, or the thresholds version decreases.
        OutOfBounds,
        /// The endpoint is empty or not UTF-8.
        InvalidEndpoint,
        /// Only X-Wing encryption keys are accepted.
        UnsupportedKem,
        /// A verdict judged by the thresholds needs valid statistics and others must not have
        /// any.
        BadStats,
        /// The statistics' parameter version is not the accepted one.
        WrongStatsVersion,
        /// The runtime does not know this parameter version, or it decreases.
        UnknownStatsVersion,
    }

    impl<T> From<PriceError> for Error<T> {
        fn from(e: PriceError) -> Self {
            match e {
                PriceError::NotSet => Self::RateNotSet,
                _ => Self::PriceOverflow,
            }
        }
    }

    impl<T> From<ReceiptError> for Error<T> {
        fn from(e: ReceiptError) -> Self {
            match e {
                ReceiptError::WrongGenesis => Self::ReceiptGenesis,
                ReceiptError::UnsupportedJobKind => Self::ReceiptKind,
                ReceiptError::ModelNotOffered => Self::ReceiptModel,
                ReceiptError::FeeMismatch { .. } | ReceiptError::Overflow => Self::ReceiptFee,
                ReceiptError::WrongProviderKey | ReceiptError::WrongGatewayKey => Self::ReceiptKey,
                _ => Self::ReceiptSignature,
            }
        }
    }

    #[pallet::hooks]
    impl<T: Config> Hooks<BlockNumberFor<T>> for Pallet<T> {
        fn on_initialize(now: BlockNumberFor<T>) -> Weight {
            let Some(params) = Params::<T>::get() else {
                return T::DbWeight::get().reads(1);
            };
            let round = round_of(now.saturated_into(), params.round_blocks);
            let mut weight = T::DbWeight::get().reads(2);
            if LastRound::<T>::get() != Some(round) {
                let auditors = Self::start_round(round);
                weight = weight.saturating_add(T::WeightInfo::start_round(auditors));
            }
            let pruned = Self::prune_step(round);
            weight = weight.saturating_add(T::WeightInfo::prune(pruned));
            if StaleClearing::<T>::exists() {
                let cleared = Self::clear_stale_step();
                weight = weight.saturating_add(T::WeightInfo::clear_stale(cleared));
            }
            weight
        }
    }

    #[pallet::call]
    impl<T: Config> Pallet<T> {
        /// Registers the caller as an auditor and bonds `stake`.
        #[pallet::call_index(0)]
        #[pallet::weight(T::WeightInfo::register())]
        pub fn register(origin: OriginFor<T>, stake: Balance) -> DispatchResult {
            let who = frame_system::ensure_signed(origin)?;
            ensure!(
                !Auditors::<T>::contains_key(&who),
                Error::<T>::AlreadyRegistered
            );
            ensure!(
                !T::Providers::is_registered(&who) && !T::Gateways::is_registered(&who),
                Error::<T>::MarketParticipant
            );
            let count = AuditorCount::<T>::get();
            ensure!(count < MAX_AUDITORS, Error::<T>::TooManyAuditors);
            ensure!(
                stake >= Self::threshold().map_err(Error::<T>::from)?,
                Error::<T>::BelowThreshold
            );
            T::Currency::hold(&HoldReason::Stake.into(), &who, stake)
                .map_err(|_| Error::<T>::InsufficientBalance)?;
            Auditors::<T>::insert(
                &who,
                AuditorOf::<T> {
                    status: AuditorStatus::Active,
                    stake,
                    unlocking: UnlockingList::default(),
                },
            );
            AuditorCount::<T>::put(count.saturating_add(1));
            Self::deposit_event(Event::Registered { who, stake });
            Ok(())
        }

        /// Adds `amount` to the stake.
        #[pallet::call_index(1)]
        #[pallet::weight(T::WeightInfo::bond_extra())]
        pub fn bond_extra(origin: OriginFor<T>, amount: Balance) -> DispatchResult {
            let who = frame_system::ensure_signed(origin)?;
            ensure!(amount > 0, Error::<T>::ZeroAmount);
            let mut a = Auditors::<T>::get(&who).ok_or(Error::<T>::NotAuditor)?;
            ensure!(a.status == AuditorStatus::Active, Error::<T>::Exiting);
            T::Currency::hold(&HoldReason::Stake.into(), &who, amount)
                .map_err(|_| Error::<T>::InsufficientBalance)?;
            a.stake = a.stake.saturating_add(amount);
            Auditors::<T>::insert(&who, a);
            Self::deposit_event(Event::Bonded { who, amount });
            Ok(())
        }

        /// Starts unbonding `amount`; the rest must stay at or above the threshold.
        #[pallet::call_index(2)]
        #[pallet::weight(T::WeightInfo::unbond())]
        pub fn unbond(origin: OriginFor<T>, amount: Balance) -> DispatchResult {
            let who = frame_system::ensure_signed(origin)?;
            ensure!(amount > 0, Error::<T>::ZeroAmount);
            let mut a = Auditors::<T>::get(&who).ok_or(Error::<T>::NotAuditor)?;
            ensure!(a.status == AuditorStatus::Active, Error::<T>::Exiting);
            ensure!(amount <= a.stake, Error::<T>::AmountTooLarge);
            let rest = a.stake.saturating_sub(amount);
            ensure!(
                rest >= Self::threshold().map_err(Error::<T>::from)?,
                Error::<T>::BelowThreshold
            );
            a.stake = rest;
            let unlock_at = Self::unlock_block()?;
            schedule_unlock(&mut a.unlocking, amount, unlock_at);
            Auditors::<T>::insert(&who, a);
            Self::deposit_event(Event::Unbonding {
                who,
                amount,
                unlock_at,
            });
            Ok(())
        }

        /// Leaves: the auditor drops off future rosters and its whole stake starts unbonding.
        #[pallet::call_index(3)]
        #[pallet::weight(T::WeightInfo::exit())]
        pub fn exit(origin: OriginFor<T>) -> DispatchResult {
            let who = frame_system::ensure_signed(origin)?;
            let mut a = Auditors::<T>::get(&who).ok_or(Error::<T>::NotAuditor)?;
            ensure!(a.status == AuditorStatus::Active, Error::<T>::Exiting);
            Self::start_exit(&who, &mut a)?;
            Auditors::<T>::insert(&who, a);
            Ok(())
        }

        /// Releases unbonding stake that is due; removes an exited auditor with nothing left.
        #[pallet::call_index(4)]
        #[pallet::weight(T::WeightInfo::withdraw_unbonded())]
        pub fn withdraw_unbonded(origin: OriginFor<T>) -> DispatchResult {
            let who = frame_system::ensure_signed(origin)?;
            let mut a = Auditors::<T>::get(&who).ok_or(Error::<T>::NotAuditor)?;
            let due = take_due(&mut a.unlocking, frame_system::Pallet::<T>::block_number());
            ensure!(due > 0, Error::<T>::NothingToWithdraw);
            T::Currency::release(&HoldReason::Stake.into(), &who, due, Precision::BestEffort)?;
            Self::deposit_event(Event::Withdrawn {
                who: who.clone(),
                amount: due,
            });
            Self::store_or_remove(&who, a);
            Ok(())
        }

        /// Submits a verdict on a provider the caller is assigned to in the current round.
        #[pallet::call_index(5)]
        #[pallet::weight(T::WeightInfo::submit_verdict())]
        pub fn submit_verdict(
            origin: OriginFor<T>,
            verdict: alloc::boxed::Box<VerdictSubmission>,
        ) -> DispatchResult {
            let who = frame_system::ensure_signed(origin)?;
            let params = Params::<T>::get().ok_or(Error::<T>::NotConfigured)?;
            let adjustable = Adjustable::<T>::get().ok_or(Error::<T>::NotConfigured)?;
            let VerdictSubmission {
                provider,
                round,
                outcome,
                thresholds_version,
                evidence,
                receipt,
                stats,
            } = *verdict;
            ensure!(
                round == Self::current_round(&params),
                Error::<T>::WrongRound
            );
            ensure!(
                Self::assignment(round, &provider).contains(&who),
                Error::<T>::NotAssigned
            );
            let mut list = Verdicts::<T>::get(round, &provider);
            ensure!(
                !list.iter().any(|v| v.auditor == who),
                Error::<T>::AlreadySubmitted
            );
            ensure!(
                thresholds_version == adjustable.thresholds_version,
                Error::<T>::WrongThresholdsVersion
            );
            let judged = outcome.judged_by_thresholds();
            ensure!(
                (judged || matches!(outcome, VerdictOutcome::Fail(_))) == evidence.is_some(),
                Error::<T>::BadEvidence
            );
            let settings = StatsSettings::<T>::get().ok_or(Error::<T>::NotConfigured)?;
            match stats {
                Some(s) => {
                    ensure!(judged && s.stats.is_valid(), Error::<T>::BadStats);
                    ensure!(s.version == settings.version, Error::<T>::WrongStatsVersion);
                }
                None => ensure!(!judged, Error::<T>::BadStats),
            }
            let body = &receipt.body;
            ensure!(body.provider == provider, Error::<T>::ReceiptProvider);
            ensure!(
                who != body.provider && who != body.gateway,
                Error::<T>::SelfAudit
            );
            Self::check(&receipt)?;
            let request_id = body.request_id;
            ensure!(
                !UsedRequests::<T>::contains_key(request_id),
                Error::<T>::RequestUsed
            );
            let count = RoundVerdicts::<T>::get(round);
            ensure!(count < T::MaxVerdictsPerRound::get(), Error::<T>::RoundFull);
            RoundVerdicts::<T>::insert(round, count.saturating_add(1));
            RoundRequests::<T>::insert(round, request_id, ());
            UsedRequests::<T>::insert(request_id, round);
            let gateway = body.gateway.clone();
            let receipt_hash = H256(ac_crypto::hash::blake3_256(&receipt.encode()));
            list.try_push(VerdictRecord {
                auditor: who.clone(),
                outcome,
                thresholds_version,
                evidence,
                receipt_hash,
                request_id,
                gateway,
                stats,
            })
            .map_err(|_| Error::<T>::AlreadySubmitted)?;
            Verdicts::<T>::insert(round, &provider, list);
            ProviderStats::<T>::mutate(&provider, |s| match outcome {
                VerdictOutcome::Pass => s.pass = s.pass.saturating_add(1),
                VerdictOutcome::Fail(_) => s.fail = s.fail.saturating_add(1),
                VerdictOutcome::Inconclusive(_) => {
                    s.inconclusive = s.inconclusive.saturating_add(1);
                }
            });
            Activity::<T>::mutate(&who, |s| s.verdicts = s.verdicts.saturating_add(1));
            Self::deposit_event(Event::VerdictAccepted {
                round,
                provider: provider.clone(),
                auditor: who.clone(),
                outcome,
            });
            Self::pay(&who, adjustable.payment_usd);
            if matches!(outcome, VerdictOutcome::Fail(_)) {
                Self::maybe_open_dispute(&params, round, &provider);
            }
            if let Some(s) = stats {
                Self::record_stats(&params, settings, (round, &provider), &who, s);
            }
            Ok(())
        }

        /// Votes in `provider`'s open dispute `id`; `Q` votes on one side decide it at once.
        #[pallet::call_index(6)]
        #[pallet::weight(T::WeightInfo::vote())]
        pub fn vote(
            origin: OriginFor<T>,
            provider: AccountId32,
            id: u64,
            vote: Vote,
        ) -> DispatchResult {
            let who = frame_system::ensure_signed(origin)?;
            let params = Params::<T>::get().ok_or(Error::<T>::NotConfigured)?;
            ensure!(
                OpenDispute::<T>::get(&provider) == Some(id),
                Error::<T>::NoDispute
            );
            let mut d = Disputes::<T>::get(id).ok_or(Error::<T>::NoDispute)?;
            ensure!(
                frame_system::Pallet::<T>::block_number() <= d.deadline,
                Error::<T>::DeadlinePassed
            );
            let slot = d
                .reviewers
                .iter_mut()
                .find(|(r, v)| *r == who && v.is_none())
                .ok_or(Error::<T>::NotReviewer)?;
            slot.1 = Some(vote);
            Activity::<T>::mutate(&who, |s| s.votes = s.votes.saturating_add(1));
            Self::deposit_event(Event::Voted {
                id,
                reviewer: who,
                vote,
            });
            let count = |side: Vote| d.reviewers.iter().filter(|(_, v)| *v == Some(side)).count();
            let quorum = usize::from(params.quorum);
            let decided = if count(Vote::Confirm) >= quorum {
                Some(DisputeOutcome::Confirmed)
            } else if count(Vote::Reject) >= quorum {
                Some(DisputeOutcome::Rejected)
            } else {
                None
            };
            match decided {
                Some(outcome) => Self::close(&params, id, d, outcome),
                None => Disputes::<T>::insert(id, d),
            }
            Ok(())
        }

        /// Closes `provider`'s dispute `id` as undecided once its deadline has passed.
        #[pallet::call_index(7)]
        #[pallet::weight(T::WeightInfo::close_dispute())]
        pub fn close_dispute(
            origin: OriginFor<T>,
            provider: AccountId32,
            id: u64,
        ) -> DispatchResult {
            frame_system::ensure_signed(origin)?;
            let params = Params::<T>::get().ok_or(Error::<T>::NotConfigured)?;
            ensure!(
                OpenDispute::<T>::get(&provider) == Some(id),
                Error::<T>::NoDispute
            );
            let d = Disputes::<T>::get(id).ok_or(Error::<T>::NoDispute)?;
            ensure!(
                frame_system::Pallet::<T>::block_number() > d.deadline,
                Error::<T>::DeadlineNotPassed
            );
            Self::close(&params, id, d, DisputeOutcome::Undecided);
            Ok(())
        }

        /// Adjusts the stake threshold, the payment and the accepted thresholds version within
        /// their guardrails; the version never decreases.
        #[pallet::call_index(8)]
        #[pallet::weight(T::WeightInfo::set_params())]
        pub fn set_params(origin: OriginFor<T>, params: AdjustableParams) -> DispatchResult {
            T::AdminOrigin::ensure_origin(origin)?;
            let old = Adjustable::<T>::get().ok_or(Error::<T>::NotConfigured)?;
            ensure!(
                params.within_bounds() && params.thresholds_version >= old.thresholds_version,
                Error::<T>::OutOfBounds
            );
            Adjustable::<T>::put(params);
            Self::deposit_event(Event::ParamsSet { params });
            Ok(())
        }

        /// Sets or clears the caller's evidence endpoint and encryption key (spec
        /// `market/audit` "审计员证据地址"). Exiting auditors may still set it: they can be the
        /// accusers of a dispute that is still open.
        #[pallet::call_index(9)]
        #[pallet::weight(T::WeightInfo::set_endpoint())]
        pub fn set_endpoint(
            origin: OriginFor<T>,
            endpoint: Option<AuditorEndpoint>,
        ) -> DispatchResult {
            let who = frame_system::ensure_signed(origin)?;
            ensure!(Auditors::<T>::contains_key(&who), Error::<T>::NotAuditor);
            let set = endpoint.is_some();
            match endpoint {
                Some((url, kem)) => {
                    ensure!(
                        !url.is_empty() && core::str::from_utf8(&url).is_ok(),
                        Error::<T>::InvalidEndpoint
                    );
                    // Other KEM AlgIds do not decode today; the rule stays explicit for when
                    // `ac-crypto` enables more (the providers' rule).
                    ensure!(kem.alg() == KemAlg::XWing, Error::<T>::UnsupportedKem);
                    AuditorEndpoints::<T>::insert(&who, (url, kem));
                }
                None => AuditorEndpoints::<T>::remove(&who),
            }
            Self::deposit_event(Event::EndpointSet { who, set });
            Ok(())
        }

        /// Sets the statistical judgment's parameter version and switch (spec `market/audit`
        /// "参数与护栏"). The version never decreases and must be built into the runtime;
        /// changing it empties every provider's state (removed over the next blocks).
        #[pallet::call_index(10)]
        #[pallet::weight(T::WeightInfo::set_stats_config())]
        pub fn set_stats_config(origin: OriginFor<T>, config: StatsConfig) -> DispatchResult {
            T::AdminOrigin::ensure_origin(origin)?;
            let old = StatsSettings::<T>::get().ok_or(Error::<T>::NotConfigured)?;
            ensure!(
                config.version >= old.version && stats_params(config.version).is_some(),
                Error::<T>::UnknownStatsVersion
            );
            if config.version != old.version {
                StaleClearing::<T>::put(ClearCursor::new());
            }
            StatsSettings::<T>::put(config);
            Self::deposit_event(Event::StatsConfigSet { config });
            Ok(())
        }
    }

    impl<T: Config> Pallet<T> {
        /// The audit pot's account.
        #[must_use]
        pub fn pot() -> AccountId32 {
            POT_ID.into_account_truncating()
        }

        /// The genesis hash receipts must carry.
        #[must_use]
        pub fn genesis() -> H256 {
            frame_system::Pallet::<T>::block_hash(BlockNumberFor::<T>::zero())
        }

        /// The round of the current block.
        #[must_use]
        pub fn current_round(params: &AuditParams) -> RoundIndex {
            round_of(
                frame_system::Pallet::<T>::block_number().saturated_into(),
                params.round_blocks,
            )
        }

        /// Stake an auditor needs now, rounded up.
        ///
        /// # Errors
        ///
        /// [`PriceError::NotSet`] without a rate or parameters; [`PriceError::Overflow`].
        pub fn threshold() -> Result<Balance, PriceError> {
            let usd = Adjustable::<T>::get().ok_or(PriceError::NotSet)?.stake_usd;
            T::Price::to_atc(usd, Rounding::Threshold)
        }

        /// The auditors assigned to `provider` in `round` (empty without a seed).
        #[must_use]
        pub fn assignment(round: RoundIndex, provider: &AccountId32) -> Vec<AccountId32> {
            let (Some(params), Some(seed), Some(roster)) = (
                Params::<T>::get(),
                Seeds::<T>::get(round),
                Rosters::<T>::get(round),
            ) else {
                return Vec::new();
            };
            sample(
                Pool {
                    roster: &roster,
                    excluded: &[],
                },
                &seed,
                provider,
                Draw::Assign,
                usize::from(params.assign),
            )
        }

        fn check(receipt: &ac_primitives::market::SignedReceipt) -> DispatchResult {
            let body = &receipt.body;
            let provider_key =
                T::Keys::key_fingerprint(&body.provider).ok_or(Error::<T>::ReceiptKey)?;
            let gateway_key =
                T::Keys::key_fingerprint(&body.gateway).ok_or(Error::<T>::ReceiptKey)?;
            let price = T::Providers::price(&body.provider, &body.model);
            let genesis = Self::genesis();
            check_receipt(
                receipt,
                &ReceiptContext {
                    genesis: &genesis,
                    provider_key: &provider_key,
                    gateway_key: &gateway_key,
                    price: price.as_ref(),
                },
            )
            .map_err(Error::<T>::from)?;
            Ok(())
        }

        fn unlock_block() -> Result<BlockNumberFor<T>, Error<T>> {
            let params = Params::<T>::get().ok_or(Error::<T>::NotConfigured)?;
            Ok(frame_system::Pallet::<T>::block_number()
                .saturating_add(params.unbond_blocks.saturated_into()))
        }

        fn start_exit(who: &AccountId32, a: &mut AuditorOf<T>) -> DispatchResult {
            let amount = core::mem::take(&mut a.stake);
            let unlock_at = Self::unlock_block()?;
            if amount > 0 {
                schedule_unlock(&mut a.unlocking, amount, unlock_at);
            }
            a.status = AuditorStatus::Exiting;
            Self::deposit_event(Event::Exiting { who: who.clone() });
            Self::deposit_event(Event::Unbonding {
                who: who.clone(),
                amount,
                unlock_at,
            });
            Ok(())
        }

        fn store_or_remove(who: &AccountId32, a: AuditorOf<T>) {
            if a.status == AuditorStatus::Exiting && a.stake == 0 && a.unlocking.is_empty() {
                Auditors::<T>::remove(who);
                AuditorEndpoints::<T>::remove(who);
                AuditorCount::<T>::mutate(|c| *c = c.saturating_sub(1));
                Self::deposit_event(Event::Removed { who: who.clone() });
            } else {
                Auditors::<T>::insert(who, a);
            }
        }

        /// Writes `round`'s roster and seed and drops those of `round − 2`. Returns the number
        /// of auditor records read.
        pub(crate) fn start_round(round: RoundIndex) -> u32 {
            let threshold = Self::threshold().ok();
            let mut roster: Vec<AccountId32> = Vec::new();
            let mut read = 0u32;
            for (who, a) in Auditors::<T>::iter() {
                read = read.saturating_add(1);
                if a.status == AuditorStatus::Active && threshold.is_some_and(|t| a.stake >= t) {
                    roster.push(who);
                }
            }
            roster.sort();
            let auditors = u32::try_from(roster.len()).unwrap_or(u32::MAX);
            // At most MAX_AUDITORS records exist, so the roster always fits.
            let roster = Roster::truncate_from(roster);
            let mut subject = ROUND_SEED_SUBJECT.to_vec();
            subject.extend_from_slice(&round.to_le_bytes());
            let seed = T::Randomness::random(&subject);
            Rosters::<T>::insert(round, roster);
            match seed {
                Some(s) => Seeds::<T>::insert(round, s),
                None => Seeds::<T>::remove(round),
            }
            if let Some(old) = round.checked_sub(2) {
                Rosters::<T>::remove(old);
                Seeds::<T>::remove(old);
            }
            LastRound::<T>::put(round);
            Self::deposit_event(Event::RoundStarted {
                round,
                auditors,
                seeded: seed.is_some(),
            });
            read
        }

        /// Removes at most [`Config::PruneLimit`] entries of the oldest round past retention:
        /// first its used request IDs, then its verdicts. A verdict list a statistical state or
        /// an open dispute still refers to moves to [`RetainedVerdicts`] instead (spec "计数与
        /// 查询"). Returns the number of entries handled.
        pub(crate) fn prune_step(current: RoundIndex) -> u32 {
            let next = PruneNext::<T>::get();
            if next.saturating_add(T::RetentionRounds::get()) >= current {
                return 0;
            }
            let limit = usize::try_from(T::PruneLimit::get()).unwrap_or(usize::MAX);
            let ids: Vec<[u8; 32]> = RoundRequests::<T>::iter_key_prefix(next)
                .take(limit)
                .collect();
            if !ids.is_empty() {
                for id in &ids {
                    UsedRequests::<T>::remove(id);
                    RoundRequests::<T>::remove(next, id);
                }
                return u32::try_from(ids.len()).unwrap_or(u32::MAX);
            }
            let mut handled = 0u32;
            for (provider, list) in Verdicts::<T>::drain_prefix(next).take(limit.max(1)) {
                handled = handled.saturating_add(1);
                if Self::referenced(next, &provider) {
                    RetainedVerdicts::<T>::insert(next, &provider, list);
                }
            }
            if Verdicts::<T>::iter_key_prefix(next).next().is_none() {
                RoundVerdicts::<T>::remove(next);
                PruneNext::<T>::put(next.saturating_add(1));
            }
            handled
        }

        /// Whether `provider`'s statistical state (of the current version) or its open dispute
        /// refers to a verdict of `round`.
        fn referenced(round: RoundIndex, provider: &AccountId32) -> bool {
            let current = StatsSettings::<T>::get().map(|c| c.version);
            let in_state = SprtStates::<T>::get(provider).is_some_and(|(v, s)| {
                Some(v) == current && s.entries.iter().any(|e| e.round == round)
            });
            in_state
                || OpenDispute::<T>::get(provider)
                    .and_then(Disputes::<T>::get)
                    .is_some_and(|d| d.accusers.iter().any(|a| a.round == round))
        }

        /// Removes the retained verdict lists of `provider` in `rounds` that nothing refers to
        /// any more.
        fn release(provider: &AccountId32, rounds: impl IntoIterator<Item = RoundIndex>) {
            let mut seen: Vec<RoundIndex> = Vec::new();
            for r in rounds {
                if seen.contains(&r) {
                    continue;
                }
                seen.push(r);
                if RetainedVerdicts::<T>::contains_key(r, provider)
                    && !Self::referenced(r, provider)
                {
                    RetainedVerdicts::<T>::remove(r, provider);
                }
            }
        }

        /// Removes at most [`STALE_CLEAR_LIMIT`] states of an older parameter version after a
        /// version change (design D10), releasing the verdicts they kept. Returns the number of
        /// states read.
        pub(crate) fn clear_stale_step() -> u32 {
            let Some(cursor) = StaleClearing::<T>::get() else {
                return 0;
            };
            let current = StatsSettings::<T>::get().map(|c| c.version);
            let limit = usize::try_from(STALE_CLEAR_LIMIT).unwrap_or(1);
            let mut iter = if cursor.is_empty() {
                SprtStates::<T>::iter()
            } else {
                SprtStates::<T>::iter_from(cursor.into_inner())
            };
            let seen: Vec<(AccountId32, VersionedSprt)> = iter.by_ref().take(limit).collect();
            let last = iter.last_raw_key().to_vec();
            for (provider, (version, state)) in &seen {
                if Some(*version) != current {
                    SprtStates::<T>::remove(provider);
                    Self::release(provider, state.entries.iter().map(|e| e.round));
                }
            }
            if seen.len() < limit {
                StaleClearing::<T>::kill();
            } else {
                // A map key (prefix and account) is 64 bytes, within the cursor's bound.
                StaleClearing::<T>::put(ClearCursor::truncate_from(last));
            }
            u32::try_from(seen.len()).unwrap_or(u32::MAX)
        }

        /// Adds a verdict's statistics to `provider`'s state (spec "统计判定") and opens a
        /// statistical dispute when the state reaches the bound, the judgment is enabled and
        /// no dispute is open.
        fn record_stats(
            params: &AuditParams,
            settings: StatsConfig,
            (round, provider): (RoundIndex, &AccountId32),
            auditor: &AccountId32,
            stats: VerdictStats,
        ) {
            let Some(sp) = stats_params(stats.version) else {
                return;
            };
            let (mut state, mut released) = match SprtStates::<T>::take(provider) {
                Some((v, s)) if v == stats.version => (s, Vec::new()),
                // A state of an older version counts as empty.
                Some((_, s)) => (
                    SprtState::default(),
                    s.entries.iter().map(|e| e.round).collect(),
                ),
                None => (SprtState::default(), Vec::new()),
            };
            released.extend(state.entries.iter().map(|e| e.round));
            state.record(
                sp,
                SprtEntry {
                    auditor: auditor.clone(),
                    round,
                    contribution: sp.contribution(&stats.stats),
                },
            );
            if state.crossed(sp)
                && settings.enabled
                && !OpenDispute::<T>::contains_key(provider)
                && Self::open_statistical_dispute(params, round, provider, &state, stats.version)
            {
                state.reset();
            }
            if state.cumulative > 0 {
                SprtStates::<T>::insert(provider, (stats.version, state));
            }
            Self::release(provider, released);
        }

        /// Opens a statistical dispute on `state`'s verdicts (design D7); `false` if too few
        /// reviewers are eligible or the round has no seed.
        fn open_statistical_dispute(
            params: &AuditParams,
            round: RoundIndex,
            provider: &AccountId32,
            state: &SprtState<AccountId32>,
            stats_version: u16,
        ) -> bool {
            let mut accusers: Vec<Accuser<AccountId32>> = Vec::new();
            let mut gateways: Vec<AccountId32> = Vec::new();
            for e in state.entries.iter() {
                accusers.push(Accuser {
                    auditor: e.auditor.clone(),
                    round: e.round,
                });
                let gateway = Self::verdict_list(e.round, provider)
                    .into_iter()
                    .find(|v| v.auditor == e.auditor)
                    .map(|v| v.gateway);
                if let Some(g) = gateway
                    && !gateways.contains(&g)
                {
                    gateways.push(g);
                }
            }
            Self::open_dispute(
                params,
                round,
                provider,
                (accusers, gateways),
                DisputeKind::Statistical { stats_version },
            )
        }

        /// Pays `usd` from the pot to `who`, or skips the payment.
        fn pay(who: &AccountId32, usd: ac_primitives::market::MicroUsd) {
            if usd.0 == 0 {
                return;
            }
            let paid = T::Price::to_atc(usd, Rounding::Payment)
                .ok()
                .filter(|amount| *amount > 0)
                .and_then(|amount| {
                    T::Currency::transfer(&Self::pot(), who, amount, Preservation::Preserve)
                        .ok()
                        .map(|_| amount)
                });
            match paid {
                Some(amount) => Self::deposit_event(Event::Paid {
                    who: who.clone(),
                    amount,
                }),
                None => Self::deposit_event(Event::PaymentSkipped { who: who.clone() }),
            }
        }

        fn maybe_open_dispute(params: &AuditParams, round: RoundIndex, provider: &AccountId32) {
            if OpenDispute::<T>::contains_key(provider) {
                return;
            }
            let mut accusers: Vec<Accuser<AccountId32>> = Vec::new();
            let mut gateways: Vec<AccountId32> = Vec::new();
            let rounds = [round.checked_sub(1), Some(round)];
            for r in rounds.into_iter().flatten() {
                for v in Verdicts::<T>::get(r, provider) {
                    if matches!(v.outcome, VerdictOutcome::Fail(_))
                        && !accusers.iter().any(|a| a.auditor == v.auditor)
                    {
                        accusers.push(Accuser {
                            auditor: v.auditor,
                            round: r,
                        });
                        if !gateways.contains(&v.gateway) {
                            gateways.push(v.gateway);
                        }
                    }
                }
            }
            if accusers.len() < 2 {
                return;
            }
            Self::open_dispute(
                params,
                round,
                provider,
                (accusers, gateways),
                DisputeKind::Fail,
            );
        }

        /// Draws the reviewers and opens a dispute on `provider` with these accusers, excluding
        /// them, the provider and their receipts' gateways from the draw; `false` if the round
        /// has no seed or too few reviewers are eligible.
        fn open_dispute(
            params: &AuditParams,
            round: RoundIndex,
            provider: &AccountId32,
            (accusers, gateways): (Vec<Accuser<AccountId32>>, Vec<AccountId32>),
            kind: DisputeKind,
        ) -> bool {
            let (Some(seed), Some(roster)) = (Seeds::<T>::get(round), Rosters::<T>::get(round))
            else {
                return false;
            };
            let mut excluded: Vec<AccountId32> =
                accusers.iter().map(|a| a.auditor.clone()).collect();
            excluded.push(provider.clone());
            excluded.extend(gateways);
            let id = NextDispute::<T>::get();
            let reviewers = sample(
                Pool {
                    roster: &roster,
                    excluded: &excluded,
                },
                &seed,
                provider,
                Draw::Review(id),
                usize::from(params.reviewers),
            );
            if reviewers.len() < usize::from(params.reviewers) {
                Self::deposit_event(Event::DisputeNotOpened {
                    provider: provider.clone(),
                });
                return false;
            }
            // Both fit: accusers come from at most two rounds of at most MAX_ASSIGN verdicts, or
            // from a statistical state of at most MAX_SPRT_ENTRIES; reviewers are at most
            // MAX_REVIEWERS by the genesis guardrail.
            let accusers = BoundedVec::<_, ConstU32<MAX_DISPUTE_ACCUSERS>>::truncate_from(accusers);
            let reviewers = BoundedVec::<_, ConstU32<{ MAX_REVIEWERS as u32 }>>::truncate_from(
                reviewers.into_iter().map(|r| (r, None)).collect(),
            );
            let deadline = frame_system::Pallet::<T>::block_number()
                .saturating_add(params.vote_blocks.saturated_into());
            Disputes::<T>::insert(
                id,
                DisputeOf::<T> {
                    provider: provider.clone(),
                    round,
                    accusers,
                    reviewers,
                    deadline,
                    outcome: None,
                    closed_at: None,
                    kind,
                },
            );
            OpenDispute::<T>::insert(provider, id);
            NextDispute::<T>::put(id.saturating_add(1));
            Self::deposit_event(Event::DisputeOpened {
                id,
                provider: provider.clone(),
                kind,
            });
            true
        }

        fn close(params: &AuditParams, id: u64, mut d: DisputeOf<T>, outcome: DisputeOutcome) {
            let winners = match outcome {
                DisputeOutcome::Confirmed => Some(Vote::Confirm),
                DisputeOutcome::Rejected => Some(Vote::Reject),
                DisputeOutcome::Undecided => None,
            };
            let payment = Adjustable::<T>::get()
                .map_or(ac_primitives::market::MicroUsd(0), |a| a.payment_usd);
            for (reviewer, vote) in &d.reviewers {
                match vote {
                    None => Activity::<T>::mutate(reviewer, |s| {
                        s.missed_votes = s.missed_votes.saturating_add(1);
                    }),
                    Some(v) if Some(*v) == winners => Self::pay(reviewer, payment),
                    Some(_) => {}
                }
            }
            match outcome {
                DisputeOutcome::Confirmed => {
                    T::Providers::slash(&d.provider, params.provider_slash);
                    if T::Providers::jail(&d.provider).is_err() {
                        Self::deposit_event(Event::JailFailed {
                            provider: d.provider.clone(),
                        });
                    }
                    ProviderStats::<T>::mutate(&d.provider, |s| {
                        s.confirmed = s.confirmed.saturating_add(1);
                    });
                }
                // A statistical dispute is the chain's own conclusion, not an auditor's
                // accusation: rejecting it slashes nobody (design D8).
                DisputeOutcome::Rejected if d.kind == DisputeKind::Fail => {
                    for a in &d.accusers {
                        Self::punish_auditor(&a.auditor, params);
                    }
                }
                DisputeOutcome::Rejected | DisputeOutcome::Undecided => {}
            }
            OpenDispute::<T>::remove(&d.provider);
            let rounds: Vec<RoundIndex> = d.accusers.iter().map(|a| a.round).collect();
            Self::release(&d.provider, rounds);
            d.outcome = Some(outcome);
            d.closed_at = Some(frame_system::Pallet::<T>::block_number());
            let provider = d.provider.clone();
            Disputes::<T>::insert(id, d);
            Self::deposit_event(Event::DisputeClosed {
                id,
                provider,
                outcome,
            });
        }

        /// Slashes an accuser of a rejected dispute and makes it exit.
        fn punish_auditor(who: &AccountId32, params: &AuditParams) {
            let Some(mut a) = Auditors::<T>::get(who) else {
                return;
            };
            let total = a.stake.saturating_add(unlocking_total(&a.unlocking));
            let target = params.auditor_slash.mul_floor(total);
            let taken = take_for_slash(&mut a.stake, &mut a.unlocking, target);
            let (credit, _) = T::Currency::slash(&HoldReason::Stake.into(), who, taken);
            let burned = credit.peek();
            T::Slash::on_unbalanced(credit);
            Self::deposit_event(Event::Slashed {
                who: who.clone(),
                amount: burned,
            });
            if a.status == AuditorStatus::Active {
                // `start_exit` only fails without parameters, which were just read.
                let _ = Self::start_exit(who, &mut a);
            }
            Self::store_or_remove(who, a);
        }

        /// An auditor's record.
        #[must_use]
        pub fn auditor(who: &AccountId32) -> Option<AuditorOf<T>> {
            Auditors::<T>::get(who)
        }

        /// Providers `auditor` is assigned to in `round`, each with whether it submitted a
        /// verdict on it. For the runtime API only: it draws for every candidate provider.
        #[must_use]
        pub fn assigned_to(
            round: RoundIndex,
            auditor: &AccountId32,
            candidates: &[AccountId32],
        ) -> Vec<(AccountId32, bool)> {
            candidates
                .iter()
                .filter(|p| Self::assignment(round, p).contains(auditor))
                .map(|p| {
                    let done = Verdicts::<T>::get(round, p)
                        .iter()
                        .any(|v| v.auditor == *auditor);
                    (p.clone(), done)
                })
                .collect()
        }

        /// An auditor's evidence endpoint and encryption key.
        #[must_use]
        pub fn endpoint(who: &AccountId32) -> Option<AuditorEndpoint> {
            AuditorEndpoints::<T>::get(who)
        }

        /// Open disputes (provider, dispute) in provider order, after `after` if given, at most
        /// `limit` (capped at [`MAX_OPEN_DISPUTES_PAGE`]). For the runtime API only.
        #[must_use]
        pub fn open_disputes(after: Option<&AccountId32>, limit: u32) -> Vec<(AccountId32, u64)> {
            let limit = usize::try_from(limit.min(MAX_OPEN_DISPUTES_PAGE)).unwrap_or(0);
            let mut all: Vec<(AccountId32, u64)> = OpenDispute::<T>::iter()
                .filter(|(p, _)| after.is_none_or(|a| p > a))
                .collect();
            all.sort_by(|x, y| x.0.cmp(&y.0));
            all.truncate(limit);
            all
        }

        /// The verdicts on `provider` in `round`, including a list retained past its retention.
        #[must_use]
        pub fn verdict_list(round: RoundIndex, provider: &AccountId32) -> VerdictList {
            let list = Verdicts::<T>::get(round, provider);
            if list.is_empty() {
                RetainedVerdicts::<T>::get(round, provider).unwrap_or_default()
            } else {
                list
            }
        }

        /// `provider`'s statistical state under the current parameter version (empty for a
        /// state of an older one).
        #[must_use]
        pub fn sprt_state(provider: &AccountId32) -> SprtState<AccountId32> {
            let current = StatsSettings::<T>::get().map(|c| c.version);
            match SprtStates::<T>::get(provider) {
                Some((v, s)) if Some(v) == current => s,
                _ => SprtState::default(),
            }
        }

        /// The first block of the current round and of the next one.
        #[must_use]
        pub fn round_bounds() -> Option<(RoundIndex, u32, u32)> {
            let params = Params::<T>::get()?;
            let r = Self::current_round(&params);
            Some((
                r,
                round_start(r, params.round_blocks),
                round_start(r.saturating_add(1), params.round_blocks),
            ))
        }
    }
}
