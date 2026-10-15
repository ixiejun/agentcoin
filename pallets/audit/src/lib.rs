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
//!   [`AuditStats`]; each adds its log-likelihood ratio to the CUSUM ([`SprtState`]) of its
//!   provider on its model. When enabled, a state at the bound opens a statistical dispute,
//!   decided like the others; a rejected one slashes no auditor for its statistics. Verdicts a
//!   state, an open dispute or an open spot check still refers to outlive their retention until
//!   it lets them go.
//! - **Audit profiles** (m6-audit-hardening): each model is audited under its own thresholds and
//!   statistical parameter versions, its profile, or the chain's default for bf16 models. A
//!   model without one is uncalibrated: verdicts on it are refused and it earns no market work.
//! - **Spot checks**: a share of the verdicts judged by the thresholds, drawn with the next
//!   round's seed, is re-checked by reviewers who compare the outcome. An overturned verdict's
//!   auditor is slashed and made to exit. Reviewers say whether they obtained the evidence, in
//!   disputes too; an auditor whose evidence was unavailable takes a light slash instead, the
//!   full one from its third such finding within the retention.
//! - **Locks**: an auditor whose verdict an open dispute or spot check cites, and a provider with
//!   an open dispute, cannot withdraw unbonding stake. Genesis checks that both unbonding periods
//!   and the retention cover the time a verdict can be cited.
//!
//! No call slashes, jails or decides a dispute or spot check directly, and the administration
//! only adjusts the stake threshold, the payment, the spot-check rate, the models' audit
//! profiles and the statistical judgment's switch, within guardrails (D6).
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
mod tests_hardening;
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

/// Most verdict lists of the previous round drawn for spot checks per block (m6-audit-hardening
/// design D5): a round's lists are drawn within its first blocks.
pub const SPOT_CHECK_BATCH: u32 = 16;

/// Most open spot checks one `open_reviews` query returns.
pub const MAX_OPEN_REVIEWS_PAGE: u32 = 256;

use ac_primitives::market::audit::{
    AdjustableParams, AuditParams, AuditProfile, PenaltyParams, VerdictStats,
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
    /// Whether the statistical judgment opens disputes (live chains: off until calibrated
    /// hardware covers them).
    #[serde(default)]
    pub stats_enabled: bool,
    /// The audit profile of bf16 models without their own (live chains: none, every model's
    /// profile is set once its calibration report is in; m6-audit-hardening design D1).
    #[serde(default)]
    pub default_profile: Option<ProfileGenesis>,
    /// Share of the verdicts judged by the thresholds that are spot-checked, in basis points
    /// (0 to 5,000).
    pub spot_check_bps: u16,
    /// Percent of the stake an auditor whose evidence was unavailable loses (at most the
    /// auditor slash).
    pub light_slash_percent: u8,
    /// Findings of unavailable evidence within the retention at which the full penalty applies
    /// (1 to 10).
    pub unavailable_limit: u8,
}

/// An audit profile as written in a chain specification.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileGenesis {
    /// Re-check thresholds version.
    pub thresholds_version: u16,
    /// Statistical judgment parameter version.
    pub stats_version: u16,
}

impl From<ProfileGenesis> for AuditProfile {
    fn from(p: ProfileGenesis) -> Self {
        Self {
            thresholds_version: p.thresholds_version,
            stats_version: p.stats_version,
        }
    }
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
        stats_enabled: false,
        default_profile: None,
        spot_check_bps: 500,
        light_slash_percent: 1,
        unavailable_limit: 3,
    };

    /// The light penalty's parameters.
    #[must_use]
    pub fn penalty(&self) -> PenaltyParams {
        PenaltyParams {
            light_slash: Perbill::from_percent(u32::from(self.light_slash_percent)),
            unavailable_limit: self.unavailable_limit,
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
                spot_check_bps: self.spot_check_bps,
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
    /// Registers `model` as a bf16 model, so it can have an audit profile.
    fn register_bf16_model(model: ac_primitives::market::ModelId);
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
        AuditGenesis, AuditRandomness, AuditorEndpoint, MAX_OPEN_DISPUTES_PAGE, SPOT_CHECK_BATCH,
        STALE_CLEAR_LIMIT, VerdictSubmission, WeightInfo,
    };
    use ac_crypto::KemAlg;
    use ac_primitives::market::audit::{
        Accuser, AdjustableParams, AuditParams, AuditProfile, AuditorRecord, AuditorStats,
        AuditorStatus, DisputeKind, DisputeOutcome, DisputeRecord, Draw, MAX_ASSIGN, MAX_AUDITORS,
        MAX_DISPUTE_ACCUSERS, MAX_MARKED, MAX_REVIEWERS, MAX_UNAVAILABLE_LIMIT, PenaltyParams,
        Pool, ProfileError, ProfileSource, ProviderAuditStats, ROUND_SEED_SUBJECT, ReviewOutcome,
        ReviewRecord, ReviewStats, ReviewVote, RoundIndex, SprtEntry, SprtState,
        UnavailablePenalty, VerdictOutcome, VerdictRecord, VerdictStats, Vote, WindowParams,
        check_windows, recent_findings, resolve, round_of, round_start, sample, spot_checked,
        stats_params, unavailable, unavailable_penalty,
    };
    use ac_primitives::market::model::ModelId;
    use ac_primitives::market::receipt::{ReceiptContext, check_receipt};
    use ac_primitives::market::records::{
        UnlockingList, schedule_unlock, take_due, take_for_slash, unlocking_total,
    };
    use ac_primitives::market::traits::{
        AccountKeys, GatewayLookup, ModelQuantization, PriceSource, ProviderAudit, ProviderPenalty,
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
    use sp_runtime::Perbill;
    use sp_runtime::traits::{AccountIdConversion, Zero};
    use sp_runtime::{AccountId32, SaturatedConversion, Saturating};

    /// Balance of the native currency (smallest ATC units).
    pub type Balance = u128;
    /// An auditor record of this runtime.
    pub type AuditorOf<T> = AuditorRecord<BlockNumberFor<T>>;
    /// A dispute record of this runtime.
    pub type DisputeOf<T> = DisputeRecord<AccountId32, BlockNumberFor<T>>;
    /// A spot-check record of this runtime.
    pub type ReviewOf<T> = ReviewRecord<AccountId32, BlockNumberFor<T>>;
    /// Rounds of an auditor's recent findings of unavailable evidence, oldest first.
    pub type Findings = BoundedVec<RoundIndex, ConstU32<{ MAX_UNAVAILABLE_LIMIT as u32 }>>;
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
    /// (m6-audit-sprt, [`crate::migrations::v1`]); version 2 moves the thresholds and statistical
    /// versions into audit profiles, adds the spot-check rate and the light penalty and keeps
    /// statistical states per model (m6-audit-hardening, [`crate::migrations::v2`]).
    pub const STORAGE_VERSION: StorageVersion = StorageVersion::new(2);

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
        /// Registered models' quantization: only bf16 models get audit profiles.
        type Models: ModelQuantization;
        /// Blocks a provider's stake unbonds for, to check the time windows at genesis.
        type ProviderUnbondBlocks: Get<u64>;
        /// Who may adjust the stake threshold, the payment, the spot-check rate, the models'
        /// audit profiles and the statistical judgment's switch.
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

    /// Whether the statistical judgment opens disputes.
    #[pallet::storage]
    pub type StatsEnabled<T: Config> = StorageValue<_, bool, ValueQuery>;

    /// Each model's own audit profile (m6-audit-hardening design D1).
    #[pallet::storage]
    pub type AuditProfiles<T: Config> = StorageMap<_, Identity, ModelId, AuditProfile, OptionQuery>;

    /// The profile of bf16 models without their own.
    #[pallet::storage]
    pub type DefaultProfile<T: Config> = StorageValue<_, AuditProfile, OptionQuery>;

    /// Each provider's statistical state on each model, with the parameter version it was built
    /// with; a state of another version than its model's profile counts as empty (design D2).
    #[pallet::storage]
    pub type ModelSprtStates<T: Config> =
        StorageDoubleMap<_, Identity, AccountId32, Identity, ModelId, VersionedSprt, OptionQuery>;

    /// The light penalty's parameters, fixed at genesis.
    #[pallet::storage]
    pub type Penalty<T: Config> = StorageValue<_, PenaltyParams, OptionQuery>;

    /// Each dispute ballot's marks of the cited verdicts whose evidence the reviewer did not
    /// obtain (bit `i`: the dispute's accuser `i`); absent means all obtained (design D6).
    #[pallet::storage]
    pub type DisputeMarks<T: Config> =
        StorageDoubleMap<_, Twox64Concat, u64, Identity, AccountId32, u128, OptionQuery>;

    /// Spot checks, open and closed (design D5).
    #[pallet::storage]
    pub type Reviews<T: Config> = StorageMap<_, Twox64Concat, u64, ReviewOf<T>, OptionQuery>;

    /// Identifier of the next spot check.
    #[pallet::storage]
    pub type NextReview<T: Config> = StorageValue<_, u64, ValueQuery>;

    /// Open spot checks.
    #[pallet::storage]
    pub type OpenReviews<T: Config> = StorageMap<_, Twox64Concat, u64, (), OptionQuery>;

    /// Open spot checks citing a verdict of (round, provider), which keeps its list.
    #[pallet::storage]
    pub type ReviewRefs<T: Config> =
        StorageDoubleMap<_, Twox64Concat, RoundIndex, Identity, AccountId32, u32, OptionQuery>;

    /// The round whose verdicts are being drawn for spot checks, and the raw key of the last
    /// verdict list drawn (empty: none yet).
    #[pallet::storage]
    pub type SpotCheckCursor<T: Config> = StorageValue<_, (RoundIndex, ClearCursor), OptionQuery>;

    /// Open disputes and spot checks citing an auditor's verdicts: while positive, its
    /// unbonding stake stays (design D8).
    #[pallet::storage]
    pub type Locks<T: Config> = StorageMap<_, Identity, AccountId32, u32, OptionQuery>;

    /// Rounds of each auditor's recent findings of unavailable evidence (design D7).
    #[pallet::storage]
    pub type UnavailableFindings<T: Config> =
        StorageMap<_, Identity, AccountId32, Findings, OptionQuery>;

    /// Spot-check counts of auditors.
    #[pallet::storage]
    pub type ReviewActivity<T: Config> =
        StorageMap<_, Identity, AccountId32, ReviewStats, ValueQuery>;

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

    /// While set, states of another parameter version than their model's profile are being
    /// removed, from this key on (empty: from the start).
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
            let penalty = self.params.penalty();
            let percents_ok = self.params.provider_slash_percent <= 100
                && self.params.auditor_slash_percent <= 100
                && self.params.light_slash_percent <= 100;
            let profile_ok = self
                .params
                .default_profile
                .is_none_or(|p| AuditProfile::from(p).check_known().is_ok());
            let windows = check_windows(&WindowParams {
                round_blocks: u64::from(fixed.round_blocks),
                vote_blocks: u64::from(fixed.vote_blocks),
                auditor_unbond: u64::from(fixed.unbond_blocks),
                provider_unbond: T::ProviderUnbondBlocks::get(),
                retention: u64::from(T::RetentionRounds::get())
                    .saturating_mul(u64::from(fixed.round_blocks)),
            });
            if fixed.check().is_err()
                || !percents_ok
                || !adjustable.within_bounds()
                || !penalty.is_valid(fixed.auditor_slash)
                || !profile_ok
                || windows.is_err()
            {
                // Genesis is built once, off chain: a bad parameter must stop the chain from
                // being created at all (spec "参数与护栏", "创世参数不满足护栏", "时间窗口的耦合约束").
                #[allow(clippy::panic)]
                {
                    panic!(
                        "invalid audit genesis parameters: {:?} ({windows:?})",
                        self.params
                    );
                }
            }
            Params::<T>::put(fixed);
            Adjustable::<T>::put(adjustable);
            Penalty::<T>::put(penalty);
            StatsEnabled::<T>::put(self.params.stats_enabled);
            if let Some(p) = self.params.default_profile {
                DefaultProfile::<T>::put(AuditProfile::from(p));
            }
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
        /// The administration switched the statistical judgment on or off.
        StatsEnabledSet {
            /// Whether it opens disputes now.
            enabled: bool,
        },
        /// The administration set a model's audit profile.
        ProfileSet {
            /// The model.
            model: ModelId,
            /// Its profile.
            profile: AuditProfile,
        },
        /// A verdict was drawn for a spot check.
        SpotCheckOpened {
            /// The spot check.
            id: u64,
            /// The verdict's provider.
            provider: AccountId32,
            /// The verdict's round.
            round: RoundIndex,
            /// The verdict's auditor.
            auditor: AccountId32,
        },
        /// A verdict was drawn for a spot check but too few auditors are eligible to review.
        SpotCheckNotOpened {
            /// The verdict's provider.
            provider: AccountId32,
            /// The verdict's auditor.
            auditor: AccountId32,
        },
        /// A spot-check reviewer voted.
        ReviewVoted {
            /// The spot check.
            id: u64,
            /// The reviewer.
            reviewer: AccountId32,
            /// The vote.
            vote: ReviewVote,
            /// Whether the reviewer obtained the evidence.
            obtained: bool,
        },
        /// A spot check was decided or closed.
        SpotCheckClosed {
            /// The spot check.
            id: u64,
            /// How it ended.
            outcome: ReviewOutcome,
        },
        /// An auditor's evidence was found unavailable in a decided dispute or spot check.
        EvidenceUnavailable {
            /// The auditor.
            who: AccountId32,
            /// Whether the full penalty applied (its findings reached the limit).
            full: bool,
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
        /// The receipt's model has no audit profile (it is uncalibrated).
        Uncalibrated,
        /// Only bf16 models get audit profiles.
        NotBf16,
        /// A profile version is not built into the runtime, or it decreases.
        BadProfile,
        /// No such open spot check.
        NoReview,
        /// The marks name a verdict the dispute does not cite.
        BadMarks,
        /// An open dispute or spot check cites the auditor's verdicts: its unbonding stake stays.
        Locked,
    }

    impl<T> From<ProfileError> for Error<T> {
        fn from(e: ProfileError) -> Self {
            match e {
                ProfileError::NotBf16 => Self::NotBf16,
                _ => Self::BadProfile,
            }
        }
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
                // The previous round's verdicts are drawn for spot checks with this round's
                // seed, which did not exist when they were submitted (design D5).
                if let Some(previous) = round.checked_sub(1) {
                    SpotCheckCursor::<T>::put((previous, ClearCursor::new()));
                }
            }
            if SpotCheckCursor::<T>::exists() {
                let (lists, opened) = Self::spot_check_step(&params, round);
                weight = weight.saturating_add(T::WeightInfo::spot_check(lists, opened));
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
            // Spec "争议当事方的罚没锁": cited verdicts keep the stake slashable.
            ensure!(!Locks::<T>::contains_key(&who), Error::<T>::Locked);
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
            // Spec "裁决": the versions are those of the receipt's model's profile.
            let model = receipt.body.model;
            let (profile, _) = Self::profile(&model).ok_or(Error::<T>::Uncalibrated)?;
            ensure!(
                thresholds_version == profile.thresholds_version,
                Error::<T>::WrongThresholdsVersion
            );
            let judged = outcome.judged_by_thresholds();
            ensure!(
                (judged || matches!(outcome, VerdictOutcome::Fail(_))) == evidence.is_some(),
                Error::<T>::BadEvidence
            );
            match stats {
                Some(s) => {
                    ensure!(judged && s.stats.is_valid(), Error::<T>::BadStats);
                    ensure!(
                        s.version == profile.stats_version,
                        Error::<T>::WrongStatsVersion
                    );
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
                Self::record_stats(&params, (round, &provider, model), &who, s);
            }
            Ok(())
        }

        /// Votes in `provider`'s open dispute `id`, marking in `unavailable` the cited verdicts
        /// whose evidence the caller did not obtain (bit `i`: the dispute's accuser `i`); `Q`
        /// votes on one side decide it at once (spec "争议与升级复核").
        #[pallet::call_index(11)]
        #[pallet::weight(T::WeightInfo::vote_dispute())]
        pub fn vote_dispute(
            origin: OriginFor<T>,
            provider: AccountId32,
            id: u64,
            vote: Vote,
            unavailable: u128,
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
            // Only the cited verdicts can be marked.
            let cited = u32::try_from(d.accusers.len()).unwrap_or(u32::MAX);
            ensure!(
                cited >= 128 || unavailable.checked_shr(cited).unwrap_or(0) == 0,
                Error::<T>::BadMarks
            );
            let slot = d
                .reviewers
                .iter_mut()
                .find(|(r, v)| *r == who && v.is_none())
                .ok_or(Error::<T>::NotReviewer)?;
            slot.1 = Some(vote);
            if unavailable != 0 {
                DisputeMarks::<T>::insert(id, &who, unavailable);
            }
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

        /// Adjusts the stake threshold, the payment and the spot-check rate within their
        /// guardrails.
        #[pallet::call_index(8)]
        #[pallet::weight(T::WeightInfo::set_params())]
        pub fn set_params(origin: OriginFor<T>, params: AdjustableParams) -> DispatchResult {
            T::AdminOrigin::ensure_origin(origin)?;
            ensure!(Adjustable::<T>::exists(), Error::<T>::NotConfigured);
            ensure!(params.within_bounds(), Error::<T>::OutOfBounds);
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

        /// Votes in open spot check `id` and says whether the caller obtained the verdict's
        /// evidence; `Q` votes on one side decide it at once (spec "裁决抽查").
        #[pallet::call_index(12)]
        #[pallet::weight(T::WeightInfo::vote_review())]
        pub fn vote_review(
            origin: OriginFor<T>,
            id: u64,
            vote: ReviewVote,
            obtained: bool,
        ) -> DispatchResult {
            let who = frame_system::ensure_signed(origin)?;
            let params = Params::<T>::get().ok_or(Error::<T>::NotConfigured)?;
            ensure!(OpenReviews::<T>::contains_key(id), Error::<T>::NoReview);
            let mut r = Reviews::<T>::get(id).ok_or(Error::<T>::NoReview)?;
            ensure!(
                frame_system::Pallet::<T>::block_number() <= r.deadline,
                Error::<T>::DeadlinePassed
            );
            let slot = r
                .reviewers
                .iter_mut()
                .find(|(reviewer, b)| *reviewer == who && b.is_none())
                .ok_or(Error::<T>::NotReviewer)?;
            slot.1 = Some((vote, obtained));
            Activity::<T>::mutate(&who, |s| s.votes = s.votes.saturating_add(1));
            Self::deposit_event(Event::ReviewVoted {
                id,
                reviewer: who,
                vote,
                obtained,
            });
            match r.decided(params.quorum) {
                Some(outcome) => Self::close_review(&params, id, r, outcome),
                None => Reviews::<T>::insert(id, r),
            }
            Ok(())
        }

        /// Closes open spot check `id` as undecided once its deadline has passed.
        #[pallet::call_index(13)]
        #[pallet::weight(T::WeightInfo::close_review())]
        pub fn close_spot_check(origin: OriginFor<T>, id: u64) -> DispatchResult {
            frame_system::ensure_signed(origin)?;
            let params = Params::<T>::get().ok_or(Error::<T>::NotConfigured)?;
            ensure!(OpenReviews::<T>::contains_key(id), Error::<T>::NoReview);
            let r = Reviews::<T>::get(id).ok_or(Error::<T>::NoReview)?;
            ensure!(
                frame_system::Pallet::<T>::block_number() > r.deadline,
                Error::<T>::DeadlineNotPassed
            );
            Self::close_review(&params, id, r, ReviewOutcome::Undecided);
            Ok(())
        }

        /// Sets `model`'s audit profile (spec "按模型的审计档案"): a bf16 model, versions built
        /// into the runtime, neither lower than its profile now. Raising the statistical version
        /// empties the model's statistical states (removed over the next blocks).
        #[pallet::call_index(14)]
        #[pallet::weight(T::WeightInfo::set_audit_profile())]
        pub fn set_audit_profile(
            origin: OriginFor<T>,
            model: ModelId,
            profile: AuditProfile,
        ) -> DispatchResult {
            T::AdminOrigin::ensure_origin(origin)?;
            let quant = T::Models::quantization(&model).ok_or(Error::<T>::NotBf16)?;
            let current = Self::profile(&model).map(|(p, _)| p);
            profile
                .check_update(quant, current.as_ref())
                .map_err(Error::<T>::from)?;
            AuditProfiles::<T>::insert(model, profile);
            if current.is_some_and(|c| c.stats_version != profile.stats_version) {
                StaleClearing::<T>::put(ClearCursor::new());
            }
            Self::deposit_event(Event::ProfileSet { model, profile });
            Ok(())
        }

        /// Switches the statistical judgment on or off (spec "参数与护栏").
        #[pallet::call_index(15)]
        #[pallet::weight(T::WeightInfo::set_stats_enabled())]
        pub fn set_stats_enabled(origin: OriginFor<T>, enabled: bool) -> DispatchResult {
            T::AdminOrigin::ensure_origin(origin)?;
            StatsEnabled::<T>::put(enabled);
            Self::deposit_event(Event::StatsEnabledSet { enabled });
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

        /// `model`'s audit profile and where it comes from: its own, else the default for a
        /// bf16 model; `None` for an uncalibrated or unregistered model.
        #[must_use]
        pub fn profile(model: &ModelId) -> Option<(AuditProfile, ProfileSource)> {
            resolve(
                AuditProfiles::<T>::get(model),
                DefaultProfile::<T>::get(),
                T::Models::quantization(model)?,
            )
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

        /// Whether one of `provider`'s statistical states, its open dispute or an open spot
        /// check refers to a verdict of `round`.
        pub(crate) fn is_referenced(round: RoundIndex, provider: &AccountId32) -> bool {
            Self::referenced(round, provider)
        }

        fn referenced(round: RoundIndex, provider: &AccountId32) -> bool {
            ReviewRefs::<T>::contains_key(round, provider)
                || ModelSprtStates::<T>::iter_prefix_values(provider)
                    .any(|(_, s)| s.entries.iter().any(|e| e.round == round))
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

        /// Removes at most [`STALE_CLEAR_LIMIT`] states of another parameter version than their
        /// model's profile after a profile change (design D2), releasing the verdicts they kept.
        /// Returns the number of states read.
        pub(crate) fn clear_stale_step() -> u32 {
            let Some(cursor) = StaleClearing::<T>::get() else {
                return 0;
            };
            let limit = usize::try_from(STALE_CLEAR_LIMIT).unwrap_or(1);
            let mut iter = if cursor.is_empty() {
                ModelSprtStates::<T>::iter()
            } else {
                ModelSprtStates::<T>::iter_from(cursor.into_inner())
            };
            let seen: Vec<(AccountId32, ModelId, VersionedSprt)> =
                iter.by_ref().take(limit).collect();
            let last = iter.last_raw_key().to_vec();
            for (provider, model, (version, state)) in &seen {
                let current = Self::profile(model).map(|(p, _)| p.stats_version);
                if Some(*version) != current {
                    ModelSprtStates::<T>::remove(provider, model);
                    Self::release(provider, state.entries.iter().map(|e| e.round));
                }
            }
            if seen.len() < limit {
                StaleClearing::<T>::kill();
            } else {
                // A double-map key (prefix, account and model) is 96 bytes, within the cursor's
                // bound.
                StaleClearing::<T>::put(ClearCursor::truncate_from(last));
            }
            u32::try_from(seen.len()).unwrap_or(u32::MAX)
        }

        /// Adds a verdict's statistics to `provider`'s state on `model` (spec "统计判定") and
        /// opens a statistical dispute when the state reaches the bound, the judgment is enabled
        /// and no dispute is open.
        fn record_stats(
            params: &AuditParams,
            (round, provider, model): (RoundIndex, &AccountId32, ModelId),
            auditor: &AccountId32,
            stats: VerdictStats,
        ) {
            let Some(sp) = stats_params(stats.version) else {
                return;
            };
            let (mut state, mut released) = match ModelSprtStates::<T>::take(provider, model) {
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
                && StatsEnabled::<T>::get()
                && !OpenDispute::<T>::contains_key(provider)
                && Self::open_statistical_dispute(
                    params,
                    (round, provider, model),
                    &state,
                    stats.version,
                )
            {
                state.reset();
            }
            if state.cumulative > 0 {
                ModelSprtStates::<T>::insert(provider, model, (stats.version, state));
            }
            Self::release(provider, released);
        }

        /// Opens a statistical dispute on `state`'s verdicts (design D7); `false` if too few
        /// reviewers are eligible or the round has no seed.
        fn open_statistical_dispute(
            params: &AuditParams,
            (round, provider, model): (RoundIndex, &AccountId32, ModelId),
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
                DisputeKind::ModelStatistical {
                    stats_version,
                    model,
                },
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
            let cited = Self::distinct_auditors(&accusers);
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
            Self::lock(&cited);
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
            // Spec "争议与升级复核": a cited verdict marked by at least `Q` ballots is evidence
            // unavailable.
            let masks: Vec<u128> = d
                .reviewers
                .iter()
                .filter(|(_, v)| v.is_some())
                .filter_map(|(r, _)| DisputeMarks::<T>::get(id, r))
                .collect();
            let lost = unavailable(&masks, d.accusers.len().min(MAX_MARKED), params.quorum);
            let unavailable_of = |who: &AccountId32| {
                d.accusers
                    .iter()
                    .zip(lost.iter())
                    .any(|(a, l)| a.auditor == *who && *l)
            };
            let cited = Self::distinct_auditors(&d.accusers);
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
                    Self::penalize_unavailable(params, &cited, unavailable_of);
                }
                // An accusation the evidence refutes costs the full slash; one whose evidence
                // was unavailable only the light one (spec "处罚").
                DisputeOutcome::Rejected if d.kind == DisputeKind::Fail => {
                    for who in &cited {
                        if unavailable_of(who) {
                            Self::penalize_unavailable(params, core::slice::from_ref(who), |_| {
                                true
                            });
                        } else {
                            Self::punish_auditor(who, params);
                        }
                    }
                }
                // A statistical dispute is the chain's own conclusion, not an auditor's
                // accusation: rejecting it slashes nobody for the statistics (m6-audit-sprt
                // design D8), only for evidence that was unavailable.
                DisputeOutcome::Rejected => {
                    Self::penalize_unavailable(params, &cited, unavailable_of);
                }
                DisputeOutcome::Undecided => {}
            }
            Self::unlock(&cited);
            let _ = DisputeMarks::<T>::clear_prefix(id, u32::from(MAX_REVIEWERS), None);
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

        /// The distinct auditors of `accusers`, in order.
        fn distinct_auditors(accusers: &[Accuser<AccountId32>]) -> Vec<AccountId32> {
            let mut out: Vec<AccountId32> = Vec::new();
            for a in accusers {
                if !out.contains(&a.auditor) {
                    out.push(a.auditor.clone());
                }
            }
            out
        }

        /// One more open dispute or spot check cites each of `auditors`.
        fn lock(auditors: &[AccountId32]) {
            for who in auditors {
                Locks::<T>::mutate(who, |n| *n = Some(n.unwrap_or(0).saturating_add(1)));
            }
        }

        /// One fewer open dispute or spot check cites each of `auditors`.
        fn unlock(auditors: &[AccountId32]) {
            for who in auditors {
                Locks::<T>::mutate_exists(who, |n| {
                    *n = n.map(|v| v.saturating_sub(1)).filter(|v| *v > 0);
                });
            }
        }

        /// Penalizes each of `auditors` for which `lost` holds for unavailable evidence: the
        /// light slash, or the full one and exit once its findings within the retention reach
        /// the limit (spec "处罚", design D7).
        fn penalize_unavailable(
            params: &AuditParams,
            auditors: &[AccountId32],
            lost: impl Fn(&AccountId32) -> bool,
        ) {
            let penalty = Penalty::<T>::get().unwrap_or(PenaltyParams::LIVE);
            let now = Self::current_round(params);
            let retention = T::RetentionRounds::get();
            for who in auditors.iter().filter(|w| lost(w)) {
                let history = UnavailableFindings::<T>::get(who).unwrap_or_default();
                let full = unavailable_penalty(&history, now, retention, penalty.unavailable_limit)
                    == UnavailablePenalty::Full;
                let mut kept: Vec<RoundIndex> = recent_findings(&history, now, retention).collect();
                kept.push(now);
                // Keep the newest findings that fit; older ones no longer matter once the limit
                // is the bound.
                let skip = kept
                    .len()
                    .saturating_sub(usize::from(MAX_UNAVAILABLE_LIMIT));
                UnavailableFindings::<T>::insert(
                    who,
                    Findings::truncate_from(kept.into_iter().skip(skip).collect()),
                );
                ReviewActivity::<T>::mutate(who, |s| {
                    s.unavailable = s.unavailable.saturating_add(1)
                });
                Self::deposit_event(Event::EvidenceUnavailable {
                    who: who.clone(),
                    full,
                });
                if full {
                    Self::punish_auditor(who, params);
                } else {
                    Self::slash_auditor(who, penalty.light_slash, false);
                }
            }
        }

        /// Slashes `who` (the auditor slash) and makes it exit: an accusation refuted, or a
        /// verdict overturned, with the evidence at hand.
        fn punish_auditor(who: &AccountId32, params: &AuditParams) {
            Self::slash_auditor(who, params.auditor_slash, true);
        }

        /// Slashes `ratio` of `who`'s whole stake and burns it; with `exit`, makes it exit too.
        fn slash_auditor(who: &AccountId32, ratio: Perbill, exit: bool) {
            let Some(mut a) = Auditors::<T>::get(who) else {
                return;
            };
            let total = a.stake.saturating_add(unlocking_total(&a.unlocking));
            let target = ratio.mul_floor(total);
            let taken = take_for_slash(&mut a.stake, &mut a.unlocking, target);
            let (credit, _) = T::Currency::slash(&HoldReason::Stake.into(), who, taken);
            let burned = credit.peek();
            T::Slash::on_unbalanced(credit);
            Self::deposit_event(Event::Slashed {
                who: who.clone(),
                amount: burned,
            });
            if exit && a.status == AuditorStatus::Active {
                // `start_exit` only fails without parameters, which the caller read.
                let _ = Self::start_exit(who, &mut a);
            }
            Self::store_or_remove(who, a);
        }

        /// Draws at most [`SPOT_CHECK_BATCH`] verdict lists of the cursor's round for spot
        /// checks with the seed of `round`, the next one (spec "裁决抽查", design D5). Returns
        /// the lists read and the spot checks opened.
        pub(crate) fn spot_check_step(params: &AuditParams, round: RoundIndex) -> (u32, u32) {
            let Some((verdict_round, cursor)) = SpotCheckCursor::<T>::get() else {
                return (0, 0);
            };
            let rate = Adjustable::<T>::get().map_or(0, |a| a.spot_check_bps);
            let (Some(seed), true) = (Seeds::<T>::get(round), rate > 0) else {
                SpotCheckCursor::<T>::kill();
                return (0, 0);
            };
            let limit = usize::try_from(SPOT_CHECK_BATCH).unwrap_or(1);
            let mut iter = if cursor.is_empty() {
                Verdicts::<T>::iter_prefix(verdict_round)
            } else {
                Verdicts::<T>::iter_prefix_from(verdict_round, cursor.into_inner())
            };
            let lists: Vec<(AccountId32, VerdictList)> = iter.by_ref().take(limit).collect();
            let last = iter.last_raw_key().to_vec();
            let mut opened = 0u32;
            for (provider, list) in &lists {
                for v in list.iter().filter(|v| v.stats.is_some()) {
                    if spot_checked(&seed, &v.request_id, rate)
                        && Self::open_review(params, (round, verdict_round), provider, v)
                    {
                        opened = opened.saturating_add(1);
                    }
                }
            }
            if lists.len() < limit {
                SpotCheckCursor::<T>::kill();
            } else {
                // A double-map key suffix is at most 64 bytes, within the cursor's bound.
                SpotCheckCursor::<T>::put((verdict_round, ClearCursor::truncate_from(last)));
            }
            (u32::try_from(lists.len()).unwrap_or(u32::MAX), opened)
        }

        /// Opens a spot check of `verdict` on `provider` from `verdict_round`, with reviewers
        /// drawn from `round`'s roster and seed, excluding its auditor, the provider and the
        /// receipt's gateway; `false` if too few are eligible.
        fn open_review(
            params: &AuditParams,
            (round, verdict_round): (RoundIndex, RoundIndex),
            provider: &AccountId32,
            verdict: &VerdictRecord<AccountId32>,
        ) -> bool {
            let (Some(seed), Some(roster)) = (Seeds::<T>::get(round), Rosters::<T>::get(round))
            else {
                return false;
            };
            let id = NextReview::<T>::get();
            let excluded = [
                verdict.auditor.clone(),
                provider.clone(),
                verdict.gateway.clone(),
            ];
            let reviewers = sample(
                Pool {
                    roster: &roster,
                    excluded: &excluded,
                },
                &seed,
                provider,
                Draw::SpotCheck(id),
                usize::from(params.reviewers),
            );
            if reviewers.len() < usize::from(params.reviewers) {
                Self::deposit_event(Event::SpotCheckNotOpened {
                    provider: provider.clone(),
                    auditor: verdict.auditor.clone(),
                });
                return false;
            }
            let deadline = frame_system::Pallet::<T>::block_number()
                .saturating_add(params.vote_blocks.saturated_into());
            Reviews::<T>::insert(
                id,
                ReviewOf::<T> {
                    provider: provider.clone(),
                    round: verdict_round,
                    auditor: verdict.auditor.clone(),
                    // At most MAX_REVIEWERS by the genesis guardrail.
                    reviewers: BoundedVec::truncate_from(
                        reviewers.into_iter().map(|r| (r, None)).collect(),
                    ),
                    deadline,
                    outcome: None,
                    closed_at: None,
                },
            );
            OpenReviews::<T>::insert(id, ());
            NextReview::<T>::put(id.saturating_add(1));
            ReviewRefs::<T>::mutate(verdict_round, provider, |n| {
                *n = Some(n.unwrap_or(0).saturating_add(1));
            });
            Self::lock(core::slice::from_ref(&verdict.auditor));
            ReviewActivity::<T>::mutate(&verdict.auditor, |s| {
                s.spot_checked = s.spot_checked.saturating_add(1);
            });
            Self::deposit_event(Event::SpotCheckOpened {
                id,
                provider: provider.clone(),
                round: verdict_round,
                auditor: verdict.auditor.clone(),
            });
            true
        }

        /// Ends spot check `id` with `outcome`: pays the winning reviewers, counts the missed
        /// ballots, and slashes the auditor of an overturned verdict (spec "处罚").
        fn close_review(params: &AuditParams, id: u64, mut r: ReviewOf<T>, outcome: ReviewOutcome) {
            let winners = match outcome {
                ReviewOutcome::Upheld => Some(ReviewVote::Uphold),
                ReviewOutcome::Overturned => Some(ReviewVote::Overturn),
                ReviewOutcome::Undecided => None,
            };
            let payment = Adjustable::<T>::get()
                .map_or(ac_primitives::market::MicroUsd(0), |a| a.payment_usd);
            for (reviewer, ballot) in &r.reviewers {
                match ballot {
                    None => Activity::<T>::mutate(reviewer, |s| {
                        s.missed_votes = s.missed_votes.saturating_add(1);
                    }),
                    Some((v, _)) if Some(*v) == winners => Self::pay(reviewer, payment),
                    Some(_) => {}
                }
            }
            let lost = r.evidence_unavailable(params.quorum);
            let auditor = r.auditor.clone();
            match outcome {
                ReviewOutcome::Overturned => {
                    ReviewActivity::<T>::mutate(&auditor, |s| {
                        s.overturned = s.overturned.saturating_add(1);
                    });
                    if lost {
                        Self::penalize_unavailable(params, core::slice::from_ref(&auditor), |_| {
                            true
                        });
                    } else {
                        Self::punish_auditor(&auditor, params);
                    }
                }
                ReviewOutcome::Upheld if lost => {
                    Self::penalize_unavailable(params, core::slice::from_ref(&auditor), |_| true);
                }
                ReviewOutcome::Upheld | ReviewOutcome::Undecided => {}
            }
            Self::unlock(core::slice::from_ref(&auditor));
            OpenReviews::<T>::remove(id);
            ReviewRefs::<T>::mutate_exists(r.round, &r.provider, |n| {
                *n = n.map(|v| v.saturating_sub(1)).filter(|v| *v > 0);
            });
            Self::release(&r.provider, [r.round]);
            r.outcome = Some(outcome);
            r.closed_at = Some(frame_system::Pallet::<T>::block_number());
            Reviews::<T>::insert(id, r);
            Self::deposit_event(Event::SpotCheckClosed { id, outcome });
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

        /// `provider`'s statistical state on `model` under the model's profile (empty for a
        /// state of another version, or an uncalibrated model).
        #[must_use]
        pub fn sprt_state(provider: &AccountId32, model: &ModelId) -> SprtState<AccountId32> {
            let current = Self::profile(model).map(|(p, _)| p.stats_version);
            match ModelSprtStates::<T>::get(provider, model) {
                Some((v, s)) if Some(v) == current => s,
                _ => SprtState::default(),
            }
        }

        /// `provider`'s statistical states, by model, under each model's profile; states of
        /// another version (not yet cleared) are left out. For the runtime API only.
        #[must_use]
        pub fn sprt_states(provider: &AccountId32) -> Vec<(ModelId, SprtState<AccountId32>)> {
            ModelSprtStates::<T>::iter_prefix(provider)
                .filter(|(model, (v, _))| {
                    Self::profile(model).is_some_and(|(p, _)| p.stats_version == *v)
                })
                .map(|(model, (_, s))| (model, s))
                .collect()
        }

        /// Open spot checks in identifier order, after `after` if given, at most `limit` (capped
        /// at [`MAX_OPEN_REVIEWS_PAGE`]). For the runtime API only.
        #[must_use]
        pub fn open_reviews(after: Option<u64>, limit: u32) -> Vec<u64> {
            let limit = usize::try_from(limit.min(super::MAX_OPEN_REVIEWS_PAGE)).unwrap_or(0);
            let mut all: Vec<u64> = OpenReviews::<T>::iter_keys()
                .filter(|id| after.is_none_or(|a| *id > a))
                .collect();
            all.sort_unstable();
            all.truncate(limit);
            all
        }

        /// A spot check, open or closed.
        #[must_use]
        pub fn review(id: u64) -> Option<ReviewOf<T>> {
            Reviews::<T>::get(id)
        }

        /// Open disputes and spot checks citing `who`'s verdicts.
        #[must_use]
        pub fn locks(who: &AccountId32) -> u32 {
            Locks::<T>::get(who).unwrap_or(0)
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

    impl<T: Config> ac_primitives::market::traits::ModelCalibration for Pallet<T> {
        fn is_calibrated(model: &ModelId) -> bool {
            Self::profile(model).is_some()
        }
    }

    impl<T: Config> ac_primitives::market::traits::DisputeLock<AccountId32> for Pallet<T> {
        /// A provider with an open dispute, or an auditor whose verdicts an open dispute or spot
        /// check cites.
        fn is_locked(who: &AccountId32) -> bool {
            OpenDispute::<T>::contains_key(who) || Locks::<T>::contains_key(who)
        }
    }
}
