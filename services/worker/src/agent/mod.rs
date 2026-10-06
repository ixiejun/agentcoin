//! `ac-worker run`: the worker service (spec `market/public-worker` "工作者服务", design D10).
//!
//! Every tick (one block) the agent:
//!
//! 1. declares itself ready once per round;
//! 2. for each unit assigned to it: fetches the job's manifest and the unit's shard, checks both
//!    against their hashes, executes the shard in the background, saves the output with a fresh
//!    salt and commits before the commit deadline; once the deadline has passed it reveals;
//! 3. for saved units no longer assigned: uploads the full result when the unit passed with this
//!    worker in the majority, retrying until the chain prunes the unit, and forgets the others;
//! 4. claims the settled epochs of its pending work and withdraws rewards whose lock has ended.
//!
//! A unit whose data does not match its hashes, or whose execution fails (an engine that is
//! down), is not committed: a miss is better than a summary that was not computed. Logs carry
//! jobs, units, counts and durations, never data or results.

pub mod config;
pub mod live;
pub mod ports;
pub mod store;
#[cfg(test)]
mod tests;

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::Instant;

use ac_primitives::emission::EpochIndex;
use ac_primitives::market::public::{
    JobId, JobSpec, Reveal, Summary, UnitState, WorkerModels, WorkerReveal, commitment,
};
use anyhow::{Context, Result};
use pallet_public_jobs::MAX_CLAIM_EPOCHS;
use sp_runtime::AccountId32;
use tokio::task::JoinHandle;

use crate::exec::Output;
use crate::logging::TARGET;
use crate::manifest::{Manifest, check_shard, parse_manifest};
use ports::{Execute, Net, WorkerCall, WorkerChain};
use store::{Key, Store};

/// Largest manifest downloaded.
pub const MAX_MANIFEST: usize = 4 << 20;
/// Largest shard downloaded.
pub const MAX_SHARD: usize = 64 << 20;
/// Blocks a sent transaction is given to take effect before it is sent again.
pub const RESEND_BLOCKS: u32 = 4;

/// Why a unit was not executed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Failure {
    /// The manifest or the shard is missing or does not match its hash.
    Data,
    /// The engine failed or the data cannot be executed.
    Execution,
}

/// What the agent did, for tests and the log.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Counters {
    /// Units executed.
    pub executed: u64,
    /// Units not committed for a data error.
    pub data_errors: u64,
    /// Units not committed for an execution error.
    pub execution_errors: u64,
    /// Commitments sent.
    pub commits: u64,
    /// Reveals sent.
    pub reveals: u64,
    /// Results uploaded.
    pub uploads: u64,
    /// Claims sent.
    pub claims: u64,
    /// Withdrawals sent.
    pub withdrawals: u64,
}

type Running = JoinHandle<std::result::Result<Output, Failure>>;

/// The worker's state.
pub struct Agent {
    chain: Arc<dyn WorkerChain>,
    net: Arc<dyn Net>,
    exec: Arc<dyn Execute>,
    me: AccountId32,
    store: Store,
    salt: Box<dyn FnMut() -> Result<[u8; 32]> + Send>,
    jobs: BTreeMap<JobId, Arc<(JobSpec, Manifest)>>,
    running: BTreeMap<Key, (Running, Instant)>,
    failed: BTreeSet<Key>,
    unclaimable: BTreeSet<EpochIndex>,
    /// Block each claimed epoch was last claimed in.
    claimed: BTreeMap<EpochIndex, u32>,
    /// Transactions in flight, by purpose, and the block they were sent in.
    sent: BTreeMap<String, u32>,
    now: u32,
    /// What it did so far.
    pub counters: Counters,
}

/// The agent's collaborators.
pub struct Ports {
    /// The chain.
    pub chain: Arc<dyn WorkerChain>,
    /// Data and uploads.
    pub net: Arc<dyn Net>,
    /// The executor.
    pub exec: Arc<dyn Execute>,
}

impl Agent {
    /// An agent for the worker account `me`, saving units in `store`; `salt` draws commitment
    /// salts (the operating system's CSPRNG outside tests).
    #[must_use]
    pub fn new(
        ports: Ports,
        me: AccountId32,
        store: Store,
        salt: Box<dyn FnMut() -> Result<[u8; 32]> + Send>,
    ) -> Self {
        Self {
            chain: ports.chain,
            net: ports.net,
            exec: ports.exec,
            me,
            store,
            salt,
            jobs: BTreeMap::new(),
            running: BTreeMap::new(),
            failed: BTreeSet::new(),
            unclaimable: BTreeSet::new(),
            claimed: BTreeMap::new(),
            sent: BTreeMap::new(),
            now: 0,
            counters: Counters::default(),
        }
    }

    /// Start-up checks: the account is a worker and its registered models are `models` (they
    /// are updated otherwise).
    ///
    /// # Errors
    ///
    /// An unregistered account, RPC failures or a failed update.
    pub async fn preflight(&self, models: &WorkerModels) -> Result<()> {
        let record =
            self.chain.worker(&self.me).await?.context(
                "the account is not a registered worker (ac-wallet public worker register)",
            )?;
        let mut have = record.models.to_vec();
        let mut want = models.to_vec();
        have.sort_unstable();
        want.sort_unstable();
        if have != want {
            log::info!(target: TARGET, "updating the registered models ({} configured)", want.len());
            anyhow::ensure!(
                self.chain
                    .submit(WorkerCall::SetModels(models.clone()))
                    .await?,
                "the model update failed"
            );
        }
        Ok(())
    }

    /// One tick at the best block.
    ///
    /// # Errors
    ///
    /// RPC failures (the next tick retries).
    pub async fn tick(&mut self) -> Result<()> {
        if self.chain.params().await?.is_none() {
            return Ok(());
        }
        let now = self.chain.best_block().await?;
        self.now = now;
        self.sent
            .retain(|_, at| now < at.saturating_add(RESEND_BLOCKS.saturating_mul(25)));
        self.ready().await?;
        self.units(now).await?;
        self.settled().await?;
        self.rewards(now).await
    }

    async fn ready(&mut self) -> Result<()> {
        let (Some(record), Some((round, _, _))) = (
            self.chain.worker(&self.me).await?,
            self.chain.round().await?,
        ) else {
            return Ok(());
        };
        if record.last_ready != Some(round) {
            self.send(format!("ready {round}"), WorkerCall::Ready).await;
        }
        Ok(())
    }

    /// Sends `call` unless a transaction for the same `purpose` was sent within
    /// [`RESEND_BLOCKS`]; a failure to send is logged and retried on a later tick, without
    /// holding up the other transactions of this one.
    async fn send(&mut self, purpose: String, call: WorkerCall) -> bool {
        let now = self.now;
        if self
            .sent
            .get(&purpose)
            .is_some_and(|at| now < at.saturating_add(RESEND_BLOCKS))
        {
            return false;
        }
        let sent = match self.chain.submit(call).await {
            Ok(sent) => sent,
            Err(e) => {
                // Node and chain errors name the call and the reason, never data.
                log::warn!(target: TARGET, "{purpose} not sent: {e:#}");
                false
            }
        };
        self.sent.insert(purpose, now);
        sent
    }

    async fn units(&mut self, now: u32) -> Result<()> {
        for a in self.chain.assigned(&self.me).await? {
            let key = (a.job, a.unit, a.attempt);
            if !a.committed {
                if now >= a.commit_by {
                    // Too late to commit: an execution still running is abandoned.
                    self.running.remove(&key);
                    continue;
                }
                if let Some(saved) = self.store.get(key) {
                    self.commit(key, &saved).await?;
                    continue;
                }
                if !self.failed.contains(&key) {
                    self.execute(key).await?;
                }
            } else if !a.revealed
                && now > a.commit_by
                // Sent now, it is included in a later block, which must be within the window.
                && now < a.reveal_by
                && let Some(saved) = self.store.get(key)
            {
                let reveal = WorkerReveal {
                    summary: Summary::try_from(saved.summary()?)
                        .map_err(|_| anyhow::anyhow!("summary too long"))?,
                    result_hash: saved.result_hash()?,
                    salt: saved.salt()?,
                };
                let (job, unit, attempt) = key;
                if self
                    .send(
                        format!("reveal {job}/{unit}/{attempt}"),
                        WorkerCall::Reveal { job, unit, reveal },
                    )
                    .await
                {
                    self.counters.reveals += 1;
                    log::info!(target: TARGET, "revealed job {job} unit {unit}");
                }
            }
        }
        Ok(())
    }

    /// Starts, or collects, the execution of an assigned unit.
    async fn execute(&mut self, key: Key) -> Result<()> {
        let (job, unit, _) = key;
        match self.running.remove(&key) {
            None => {
                let data = match self.job_data(job).await {
                    Ok(d) => d,
                    Err(Failure::Data) => {
                        self.fail(key, Failure::Data);
                        return Ok(());
                    }
                    Err(Failure::Execution) => return Ok(()),
                };
                if !self.exec.can_run(&data.0) {
                    self.fail(key, Failure::Execution);
                    return Ok(());
                }
                let (net, exec) = (Arc::clone(&self.net), Arc::clone(&self.exec));
                let handle = tokio::spawn(async move {
                    let (spec, manifest) = &*data;
                    let index = usize::try_from(unit).map_err(|_| Failure::Data)?;
                    let shard = manifest.units.get(index).ok_or(Failure::Data)?;
                    let bytes = net
                        .get(&shard.url, MAX_SHARD)
                        .await
                        .map_err(|_| Failure::Data)?;
                    check_shard(&bytes, shard).map_err(|_| Failure::Data)?;
                    exec.run(job, spec, &bytes)
                        .await
                        .map_err(|_| Failure::Execution)
                });
                self.running.insert(key, (handle, Instant::now()));
            }
            Some((handle, started)) if handle.is_finished() => match handle.await {
                Ok(Ok(output)) => {
                    let salt = (self.salt)()?;
                    let saved = self.store.save(key, &output, &salt)?;
                    self.counters.executed += 1;
                    log::info!(
                        target: TARGET,
                        "executed job {job} unit {unit} in {} ms",
                        started.elapsed().as_millis()
                    );
                    self.commit(key, &saved).await?;
                }
                Ok(Err(failure)) => self.fail(key, failure),
                Err(_) => self.fail(key, Failure::Execution),
            },
            Some(running) => {
                self.running.insert(key, running);
            }
        }
        Ok(())
    }

    fn fail(&mut self, key: Key, failure: Failure) {
        let (job, unit, _) = key;
        match failure {
            Failure::Data => {
                self.counters.data_errors += 1;
                log::warn!(target: TARGET, "data error in job {job} unit {unit}: not committed");
            }
            Failure::Execution => {
                self.counters.execution_errors += 1;
                log::warn!(target: TARGET, "execution error in job {job} unit {unit}: not committed");
            }
        }
        self.failed.insert(key);
    }

    /// The job's spec and checked manifest; `Execution` for a transient failure (retried).
    async fn job_data(
        &mut self,
        job: JobId,
    ) -> std::result::Result<Arc<(JobSpec, Manifest)>, Failure> {
        if let Some(d) = self.jobs.get(&job) {
            return Ok(Arc::clone(d));
        }
        let record = match self.chain.job(job).await {
            Ok(Some(r)) => r,
            Ok(None) => return Err(Failure::Data),
            Err(_) => return Err(Failure::Execution),
        };
        let url =
            String::from_utf8(record.spec.manifest_url.to_vec()).map_err(|_| Failure::Data)?;
        let bytes = self
            .net
            .get(&url, MAX_MANIFEST)
            .await
            .map_err(|_| Failure::Data)?;
        let manifest =
            parse_manifest(&bytes, &record.spec.manifest_hash).map_err(|_| Failure::Data)?;
        let data = Arc::new((record.spec, manifest));
        self.jobs.insert(job, Arc::clone(&data));
        Ok(data)
    }

    async fn commit(&mut self, key: Key, saved: &store::Saved) -> Result<()> {
        let (job, unit, attempt) = key;
        let hash = commitment(&Reveal {
            job,
            unit,
            attempt,
            worker: &self.me,
            summary: &saved.summary()?,
            result_hash: &saved.result_hash()?,
            salt: &saved.salt()?,
        });
        if self
            .send(
                format!("commit {job}/{unit}/{attempt}"),
                WorkerCall::Commit { job, unit, hash },
            )
            .await
        {
            self.counters.commits += 1;
            log::info!(target: TARGET, "committed job {job} unit {unit}");
        }
        Ok(())
    }

    /// Saved units that are no longer assigned: upload or forget.
    async fn settled(&mut self) -> Result<()> {
        let assigned: BTreeSet<Key> = self
            .chain
            .assigned(&self.me)
            .await?
            .iter()
            .map(|a| (a.job, a.unit, a.attempt))
            .collect();
        for key in self.store.keys() {
            if assigned.contains(&key) {
                continue;
            }
            let (job, unit, attempt) = key;
            let Some(u) = self.chain.unit(job, unit).await? else {
                // Pruned: nothing left to do with it.
                self.store.remove(key);
                continue;
            };
            if u.attempt != attempt {
                self.store.remove(key);
                continue;
            }
            match u.state {
                UnitState::Accepted { majority, .. } => {
                    let mine = u.assigned.iter().position(|w| *w == self.me);
                    let in_majority = mine.and_then(|i| majority.get(i)).copied() == Some(true);
                    let uploaded = self.store.get(key).is_some_and(|s| s.uploaded);
                    if in_majority && !uploaded {
                        self.upload(key).await;
                    } else if !in_majority {
                        self.store.remove(key);
                    }
                }
                UnitState::Open => {}
                _ => self.store.remove(key),
            }
        }
        Ok(())
    }

    async fn upload(&mut self, key: Key) {
        let (job, _, _) = key;
        let Some(data) = self.jobs.get(&job).cloned() else {
            // After a restart: the spec alone is enough to upload.
            if let Ok(Some(record)) = self.chain.job(job).await {
                self.upload_to(key, &record.spec).await;
            }
            return;
        };
        self.upload_to(key, &data.0).await;
    }

    async fn upload_to(&mut self, key: Key, spec: &JobSpec) {
        let (job, unit, _) = key;
        let Ok(base) = String::from_utf8(spec.results_url.to_vec()) else {
            return;
        };
        let url = ac_wallet::http::join(&base, &format!("{job}/{unit}"));
        let Ok(body) = self.store.result(key) else {
            return;
        };
        match self.net.put(&url, &self.me, body).await {
            Ok(()) => {
                if self.store.mark_uploaded(key).is_ok() {
                    self.counters.uploads += 1;
                    log::info!(target: TARGET, "uploaded the result of job {job} unit {unit}");
                }
            }
            Err(_) => {
                log::warn!(target: TARGET, "upload of job {job} unit {unit} failed; retrying");
            }
        }
    }

    async fn rewards(&mut self, now: u32) -> Result<()> {
        let mut due = Vec::new();
        for (epoch, _) in self.chain.pending(&self.me).await? {
            match self.claimed.get(&epoch) {
                // Claimed and still pending after the claim had time to land: its share rounds
                // to nothing, so it is never claimable; do not pay for claiming it again.
                Some(at) if now >= at.saturating_add(RESEND_BLOCKS) => {
                    self.unclaimable.insert(epoch);
                    continue;
                }
                Some(_) => continue,
                None => {}
            }
            if !self.unclaimable.contains(&epoch)
                && self.chain.epoch(epoch).await?.emission.is_some()
            {
                due.push(epoch);
            }
        }
        due.truncate(usize::try_from(MAX_CLAIM_EPOCHS).unwrap_or(16));
        if !due.is_empty()
            && self
                .send(format!("claim {due:?}"), WorkerCall::Claim(due.clone()))
                .await
        {
            self.counters.claims += 1;
            log::info!(target: TARGET, "claimed {} epochs", due.len());
            for e in due {
                self.claimed.insert(e, now);
            }
        }
        let locked = self.chain.locked(&self.me).await?;
        if locked.iter().any(|(at, _)| *at <= now)
            && self.send("withdraw".into(), WorkerCall::Withdraw).await
        {
            self.counters.withdrawals += 1;
            log::info!(target: TARGET, "withdrew unlocked rewards");
        }
        Ok(())
    }
}
