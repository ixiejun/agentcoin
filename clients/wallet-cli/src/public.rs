//! Public jobs (m6-public-jobs; spec `clients/wallet-cli` "公共任务子命令"): reads through
//! `PublicJobsApi`, shared by the wallet's `public` commands and `ac-worker`.

use ac_primitives::emission::EpochIndex;
use ac_primitives::market::audit::RoundIndex;
use ac_primitives::market::public::{
    Assignment, EpochPublic, JobId, JobRecord, PublicParams, UnitIndex, UnitRecord, WorkerRecord,
};
use anyhow::Result;
use parity_scale_codec::{Decode, Encode};
use sp_core::H256;
use sp_runtime::AccountId32;

use crate::client::NodeClient;

/// A job as the chain stores it.
pub type Job = JobRecord<u32>;
/// A unit's current attempt as the chain stores it.
pub type Unit = UnitRecord<AccountId32, u32>;
/// A worker as the chain stores it.
pub type Worker = WorkerRecord<u32>;

impl NodeClient {
    async fn public_api<T: Decode>(&self, method: &str, args: &impl Encode) -> Result<T> {
        let raw = self
            .call_api(&format!("PublicJobsApi_{method}"), args)
            .await?;
        Ok(T::decode(&mut &raw[..])?)
    }

    /// The current round and its first and last block.
    ///
    /// # Errors
    ///
    /// RPC failures.
    pub async fn public_round(&self) -> Result<Option<(RoundIndex, u32, u32)>> {
        self.public_api("round", &()).await
    }

    /// The current round's roster and seed.
    ///
    /// # Errors
    ///
    /// RPC failures.
    pub async fn public_roster(&self) -> Result<(Vec<AccountId32>, Option<H256>)> {
        self.public_api("roster", &()).await
    }

    /// A worker's record.
    ///
    /// # Errors
    ///
    /// RPC failures.
    pub async fn public_worker(&self, who: &AccountId32) -> Result<Option<Worker>> {
        self.public_api("worker", who).await
    }

    /// A job.
    ///
    /// # Errors
    ///
    /// RPC failures.
    pub async fn public_job(&self, job: JobId) -> Result<Option<Job>> {
        self.public_api("job", &job).await
    }

    /// Jobs in progress, in publication order.
    ///
    /// # Errors
    ///
    /// RPC failures.
    pub async fn public_jobs(&self) -> Result<Vec<JobId>> {
        self.public_api("jobs", &()).await
    }

    /// A unit's current attempt.
    ///
    /// # Errors
    ///
    /// RPC failures.
    pub async fn public_unit(&self, job: JobId, unit: UnitIndex) -> Result<Option<Unit>> {
        self.public_api("unit", &(job, unit)).await
    }

    /// A worker's unsettled units.
    ///
    /// # Errors
    ///
    /// RPC failures.
    pub async fn public_assigned(&self, who: &AccountId32) -> Result<Vec<Assignment<u32>>> {
        self.public_api("assigned", who).await
    }

    /// An epoch's public work and emission.
    ///
    /// # Errors
    ///
    /// RPC failures.
    pub async fn public_epoch(&self, epoch: EpochIndex) -> Result<EpochPublic<u128>> {
        self.public_api("epoch", &epoch).await
    }

    /// A worker's unclaimed public work by maturity epoch.
    ///
    /// # Errors
    ///
    /// RPC failures.
    pub async fn public_pending(&self, who: &AccountId32) -> Result<Vec<(EpochIndex, u128)>> {
        self.public_api("pending", who).await
    }

    /// A worker's locked rewards as `(unlock block, amount)`.
    ///
    /// # Errors
    ///
    /// RPC failures.
    pub async fn public_locked(&self, who: &AccountId32) -> Result<Vec<(u32, u128)>> {
        self.public_api("locked", who).await
    }

    /// Balance of the public payout account.
    ///
    /// # Errors
    ///
    /// RPC failures.
    pub async fn public_pot(&self) -> Result<u128> {
        self.public_api("pot_balance", &()).await
    }

    /// Current parameters.
    ///
    /// # Errors
    ///
    /// RPC failures.
    pub async fn public_params(&self) -> Result<Option<PublicParams>> {
        self.public_api("params", &()).await
    }
}
