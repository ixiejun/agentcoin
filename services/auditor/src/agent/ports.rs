//! What the agent needs from the outside world, as traits, so that its decisions can be tested
//! with stand-ins: the chain, buying an inference, re-checking one, and fetching a dispute's
//! evidence from an accuser.

use ac_market_proto::ErrorCode;
use ac_market_proto::audit::EvidenceResponse;
use ac_primitives::market::ModelId;
use ac_primitives::market::audit::{
    AdjustableParams, AuditParams, DisputeRecord, RoundIndex, SprtState, StatsConfig,
    VerdictRecord, Vote,
};
use ac_primitives::market::model::QuantType;
use ac_wallet::proxy::Completed;
use anyhow::Result;
use async_trait::async_trait;
use pallet_audit::{AuditorEndpoint, VerdictSubmission};
use sp_runtime::AccountId32;

use crate::case::{RecheckCase, Report};

/// A dispute as the chain stores it.
pub type Dispute = DisputeRecord<AccountId32, u32>;

/// The audit transactions the agent sends, signed by the auditor account.
#[derive(Clone, Debug)]
pub enum AuditCall {
    /// A verdict.
    Verdict(Box<VerdictSubmission>),
    /// A reviewer's vote.
    Vote {
        /// The accused provider.
        provider: AccountId32,
        /// The dispute.
        id: u64,
        /// The vote.
        vote: Vote,
    },
    /// Closing an undecided dispute after its deadline.
    Close {
        /// The accused provider.
        provider: AccountId32,
        /// The dispute.
        id: u64,
    },
    /// Setting the evidence endpoint.
    SetEndpoint(Option<AuditorEndpoint>),
}

/// The chain, read through the runtime APIs and written by the auditor's transactions.
#[async_trait]
pub trait AuditChain: Send + Sync {
    /// The best block.
    async fn best_block(&self) -> Result<u32>;
    /// The current round and the first blocks of it and of the next one.
    async fn round(&self) -> Result<Option<(RoundIndex, u32, u32)>>;
    /// Fixed and adjustable audit parameters.
    async fn params(&self) -> Result<Option<(AuditParams, AdjustableParams)>>;
    /// The providers `auditor` is assigned to in `round`, with whether it submitted on each.
    async fn assigned_to(
        &self,
        round: RoundIndex,
        auditor: &AccountId32,
    ) -> Result<Vec<(AccountId32, bool)>>;
    /// The models a provider is registered for.
    async fn provider_models(&self, provider: &AccountId32) -> Result<Vec<ModelId>>;
    /// A model's registered precision; `None` for an unknown model.
    async fn model_quant(&self, model: ModelId) -> Result<Option<QuantType>>;
    /// The verdicts on `provider` in `round`.
    async fn verdicts(
        &self,
        round: RoundIndex,
        provider: &AccountId32,
    ) -> Result<Vec<VerdictRecord<AccountId32>>>;
    /// Every open dispute (provider, id).
    async fn open_disputes(&self) -> Result<Vec<(AccountId32, u64)>>;
    /// A dispute.
    async fn dispute(&self, id: u64) -> Result<Option<Dispute>>;
    /// An auditor's evidence endpoint.
    async fn endpoint(&self, who: &AccountId32) -> Result<Option<AuditorEndpoint>>;
    /// Whether `who` is a registered, not exiting auditor.
    async fn is_active_auditor(&self, who: &AccountId32) -> Result<bool>;
    /// The statistical judgment's parameter version and switch.
    async fn stats_config(&self) -> Result<Option<StatsConfig>>;
    /// `provider`'s statistical state under the current parameter version.
    async fn sprt_state(&self, provider: &AccountId32) -> Result<SprtState<AccountId32>>;
    /// Sends an audit transaction; `Ok(false)` when it was included but failed.
    async fn submit(&self, call: AuditCall) -> Result<bool>;
}

/// Buying inferences through the payment accounts' channels.
#[async_trait]
pub trait Shop: Send + Sync {
    /// Number of (payment account, gateway) routes.
    fn routes(&self) -> usize;
    /// Whether a route can pay for a request now (not halted, below its spending limit).
    async fn usable(&self, route: usize) -> bool;
    /// One non-streamed request through `route`, pinned to `provider`.
    async fn buy(
        &self,
        route: usize,
        body: &[u8],
        provider: &AccountId32,
    ) -> Result<Completed, (ErrorCode, String)>;
}

/// Re-check engines, one per model.
#[async_trait]
pub trait Recheck: Send + Sync {
    /// The engine's model name for `model`, if this agent can re-check it.
    fn engine_model(&self, model: ModelId) -> Option<String>;
    /// Re-checks `case`.
    async fn recheck(&self, case: &RecheckCase, quant: QuantType) -> Report;
    /// The prompt tokens of `messages` under `model`'s chat template, as the re-check counts
    /// them (m6-toploc-gpu-calibration design D9).
    async fn prompt_tokens(&self, model: ModelId, messages: &serde_json::Value) -> Result<u32>;
}

/// Fetching evidence from an accuser's endpoint.
#[async_trait]
pub trait FetchEvidence: Send + Sync {
    /// Asks `endpoint` for the evidence of `commitment` in dispute `dispute`.
    async fn fetch(
        &self,
        endpoint: &AuditorEndpoint,
        dispute: u64,
        commitment: [u8; 32],
    ) -> Result<EvidenceResponse>;
}
