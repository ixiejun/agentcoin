//! What the worker needs from the outside world, as traits, so that its decisions can be tested
//! with stand-ins: the chain, the network (data and uploads) and the unit executor.

use ac_primitives::emission::EpochIndex;
use ac_primitives::market::audit::RoundIndex;
use ac_primitives::market::public::{
    Assignment, EpochPublic, JobId, JobSpec, PublicParams, UnitIndex, WorkerModels, WorkerReveal,
};
use ac_wallet::public::{Job, Unit, Worker};
use anyhow::Result;
use async_trait::async_trait;
use sp_core::H256;
use sp_runtime::AccountId32;

use crate::exec::Output;

/// The public job transactions a worker sends, signed by its account.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WorkerCall {
    /// Replaces the registered models.
    SetModels(WorkerModels),
    /// Ready for the current round.
    Ready,
    /// A commitment.
    Commit {
        /// The job.
        job: JobId,
        /// The unit.
        unit: UnitIndex,
        /// The commitment.
        hash: H256,
    },
    /// A reveal.
    Reveal {
        /// The job.
        job: JobId,
        /// The unit.
        unit: UnitIndex,
        /// What is revealed.
        reveal: WorkerReveal,
    },
    /// Claims settled epochs.
    Claim(Vec<EpochIndex>),
    /// Releases rewards whose lock has ended.
    Withdraw,
}

/// The chain, read through `PublicJobsApi` and written by the worker's transactions.
#[async_trait]
pub trait WorkerChain: Send + Sync {
    /// The best block.
    async fn best_block(&self) -> Result<u32>;
    /// Current parameters.
    async fn params(&self) -> Result<Option<PublicParams>>;
    /// The current round and its first and last block.
    async fn round(&self) -> Result<Option<(RoundIndex, u32, u32)>>;
    /// A worker's record.
    async fn worker(&self, who: &AccountId32) -> Result<Option<Worker>>;
    /// A worker's unsettled units.
    async fn assigned(&self, who: &AccountId32) -> Result<Vec<Assignment<u32>>>;
    /// A job.
    async fn job(&self, job: JobId) -> Result<Option<Job>>;
    /// A unit's current attempt.
    async fn unit(&self, job: JobId, unit: UnitIndex) -> Result<Option<Unit>>;
    /// A worker's unclaimed public work by maturity epoch.
    async fn pending(&self, who: &AccountId32) -> Result<Vec<(EpochIndex, u128)>>;
    /// An epoch's public work and emission.
    async fn epoch(&self, epoch: EpochIndex) -> Result<EpochPublic<u128>>;
    /// A worker's locked rewards as `(unlock block, amount)`.
    async fn locked(&self, who: &AccountId32) -> Result<Vec<(u32, u128)>>;
    /// Sends a transaction; `Ok(true)` once the node accepted it (the agent reads its effect back
    /// from the chain), `Ok(false)` when it was rejected without an error.
    async fn submit(&self, call: WorkerCall) -> Result<bool>;
}

/// Data downloads and result uploads.
#[async_trait]
pub trait Net: Send + Sync {
    /// `GET url`, at most `limit` bytes.
    async fn get(&self, url: &str, limit: usize) -> Result<Vec<u8>>;
    /// Uploads a unit's full result to `url` as `worker`.
    async fn put(&self, url: &str, worker: &AccountId32, body: Vec<u8>) -> Result<()>;
}

/// Runs a unit's shard with the rules of its kind.
#[async_trait]
pub trait Execute: Send + Sync {
    /// Whether this worker can run units of `spec` (its model is configured).
    fn can_run(&self, spec: &JobSpec) -> bool;
    /// Executes a shard whose bytes were checked against the manifest.
    async fn run(&self, job: JobId, spec: &JobSpec, shard: &[u8]) -> Result<Output>;
}
