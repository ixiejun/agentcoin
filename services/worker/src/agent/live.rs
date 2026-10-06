//! The worker wired to the real world: the node (reads through `PublicJobsApi`, transactions
//! signed by the worker account), HTTP for data and uploads, the configured engines, and the
//! loop of `ac-worker run`.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use ac_primitives::emission::EpochIndex;
use ac_primitives::encode_address;
use ac_primitives::market::ModelId;
use ac_primitives::market::audit::RoundIndex;
use ac_primitives::market::public::{
    Assignment, EpochPublic, JobId, JobSpec, MAX_ITEMS, PublicParams, UnitIndex, WorkerModels,
};
use ac_primitives::market::work::JobKind;
use ac_runtime::RuntimeCall;
use ac_wallet::http::{Client, read_body};
use ac_wallet::market::parse_model_id;
use ac_wallet::ops::Signer;
use ac_wallet::public::{Job, Unit, Worker};
use ac_wallet::wallet::read_password_file;
use ac_wallet::{NodeClient, Wallet};
use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use frame_support::BoundedVec;
use rand_core::TryRng;
use sp_runtime::AccountId32;
use tokio::sync::Mutex;

use super::config::Config;
use super::ports::{Execute, Net, WorkerCall, WorkerChain};
use super::store::Store;
use super::{Agent, Ports};
use crate::engine::EngineClient;
use crate::exec::{self, Output};
use crate::logging::TARGET;
use crate::manifest::{MAX_DOCUMENTS, parse_eval, parse_texts};

/// Request header naming the uploading worker (design D10).
pub const WORKER_HEADER: &str = "X-AgentCoin-Worker";

/// The node and the worker's signer.
pub struct NodeChain {
    /// The node.
    pub node: NodeClient,
    /// The worker account's signer.
    pub signer: Signer,
}

#[async_trait]
impl WorkerChain for NodeChain {
    async fn best_block(&self) -> Result<u32> {
        Ok(u32::try_from(self.node.best_block().await?).unwrap_or(u32::MAX))
    }

    async fn params(&self) -> Result<Option<PublicParams>> {
        self.node.public_params().await
    }

    async fn round(&self) -> Result<Option<(RoundIndex, u32, u32)>> {
        self.node.public_round().await
    }

    async fn worker(&self, who: &AccountId32) -> Result<Option<Worker>> {
        self.node.public_worker(who).await
    }

    async fn assigned(&self, who: &AccountId32) -> Result<Vec<Assignment<u32>>> {
        self.node.public_assigned(who).await
    }

    async fn job(&self, job: JobId) -> Result<Option<Job>> {
        self.node.public_job(job).await
    }

    async fn unit(&self, job: JobId, unit: UnitIndex) -> Result<Option<Unit>> {
        self.node.public_unit(job, unit).await
    }

    async fn pending(&self, who: &AccountId32) -> Result<Vec<(EpochIndex, u128)>> {
        self.node.public_pending(who).await
    }

    async fn epoch(&self, epoch: EpochIndex) -> Result<EpochPublic<u128>> {
        self.node.public_epoch(epoch).await
    }

    async fn locked(&self, who: &AccountId32) -> Result<Vec<(u32, u128)>> {
        self.node.public_locked(who).await
    }

    async fn submit(&self, call: WorkerCall) -> Result<bool> {
        let call = match call {
            WorkerCall::SetModels(models) => pallet_public_jobs::Call::set_models { models },
            WorkerCall::Ready => pallet_public_jobs::Call::ready {},
            WorkerCall::Commit { job, unit, hash } => {
                pallet_public_jobs::Call::commit { job, unit, hash }
            }
            WorkerCall::Reveal { job, unit, reveal } => pallet_public_jobs::Call::reveal {
                job,
                unit,
                revealed: reveal,
            },
            WorkerCall::Claim(epochs) => pallet_public_jobs::Call::claim {
                epochs: BoundedVec::truncate_from(epochs),
            },
            WorkerCall::Withdraw => pallet_public_jobs::Call::withdraw {},
        };
        // Sent without waiting for inclusion: a worker has many commitments and reveals with
        // deadlines a few blocks apart, and the agent reads their effect back from the chain.
        // The nonce counts the worker's transactions still in the pool.
        let nonce = self.node.next_nonce(&self.signer.account).await?;
        let xt = self
            .signer
            .sign_call_at(&self.node, RuntimeCall::PublicJobs(call), nonce)
            .await?;
        self.node.submit(&xt).await?;
        Ok(true)
    }
}

/// HTTP downloads and uploads.
pub struct HttpNet {
    /// The client.
    pub client: Client,
}

#[async_trait]
impl Net for HttpNet {
    async fn get(&self, url: &str, limit: usize) -> Result<Vec<u8>> {
        Ok(self.client.get_bytes(url, limit).await?.to_vec())
    }

    async fn put(&self, url: &str, worker: &AccountId32, body: Vec<u8>) -> Result<()> {
        let address = encode_address(worker.as_ref());
        let resp = self
            .client
            .put_with(
                url,
                "application/octet-stream",
                &[(WORKER_HEADER, &address)],
                body,
            )
            .await?;
        let status = resp.status();
        let _ = read_body(resp.into_body(), 4096).await;
        // 409: the unit's result is already collected (another majority worker uploaded it).
        if !status.is_success() && status.as_u16() != 409 {
            bail!("the collector answered {status}");
        }
        Ok(())
    }
}

/// One engine; units of its model run one at a time.
pub struct Engine {
    /// The client.
    pub client: EngineClient,
    /// The model's name on the engine.
    pub model: String,
    /// Serializes use of the engine.
    pub busy: Mutex<()>,
}

/// The configured engines.
pub struct Engines {
    /// Engines by on-chain model.
    pub engines: BTreeMap<ModelId, Engine>,
}

#[async_trait]
impl Execute for Engines {
    fn can_run(&self, spec: &JobSpec) -> bool {
        match (spec.kind, spec.model) {
            (JobKind::DataClean, _) => true,
            (JobKind::Eval | JobKind::Embed, Some(m)) => self.engines.contains_key(&m),
            _ => false,
        }
    }

    async fn run(&self, job: JobId, spec: &JobSpec, shard: &[u8]) -> Result<Output> {
        if spec.kind == JobKind::DataClean {
            let docs = parse_texts(shard, MAX_DOCUMENTS)?;
            return tokio::task::spawn_blocking(move || exec::clean_unit(&docs)).await?;
        }
        let engine = spec
            .model
            .and_then(|m| self.engines.get(&m))
            .context("no engine for the job's model")?;
        let _busy = engine.busy.lock().await;
        match spec.kind {
            JobKind::Eval => {
                exec::eval_unit(&engine.client, &engine.model, &parse_eval(shard)?).await
            }
            JobKind::Embed => {
                let texts = parse_texts(shard, MAX_ITEMS)?;
                exec::embed_unit(&engine.client, &engine.model, job, &texts).await
            }
            _ => bail!("not a public job kind"),
        }
    }
}

/// Builds the engines of a configuration and the models they make the worker register for.
///
/// # Errors
///
/// A malformed model ID, a duplicate model or a bad engine URL.
pub fn engines(cfg: &Config) -> Result<(Engines, WorkerModels)> {
    let mut engines = BTreeMap::new();
    for e in &cfg.engines {
        let model = parse_model_id(&e.model)?;
        let engine = Engine {
            client: EngineClient::new(&e.engine)?,
            model: e.engine_model.clone(),
            busy: Mutex::new(()),
        };
        if engines.insert(model, engine).is_some() {
            bail!("model {} is configured twice", e.model);
        }
    }
    let models = WorkerModels::try_from(engines.keys().copied().collect::<Vec<_>>())
        .map_err(|_| anyhow::anyhow!("too many models"))?;
    Ok((Engines { engines }, models))
}

/// Salts from the operating system's CSPRNG (AGENT.md §6.6).
///
/// # Errors
///
/// The operating system's random source fails.
pub fn os_salts() -> Result<Box<dyn FnMut() -> Result<[u8; 32]> + Send>> {
    let mut rng = ac_crypto::OsRng::new()?;
    Ok(Box::new(move || {
        let mut salt = [0u8; 32];
        rng.try_fill_bytes(&mut salt)
            .map_err(|_| anyhow::anyhow!("the random source failed"))?;
        Ok(salt)
    }))
}

/// `ac-worker run`: start-up checks, then a tick every `poll_ms`.
///
/// # Errors
///
/// A bad configuration, a wrong password or failed start-up checks.
pub async fn run(cfg: Config) -> Result<()> {
    let password = read_password_file(&cfg.password_file)?;
    let wallet = Wallet::load(&cfg.wallet)?;
    let signer = Signer::from_wallet(&wallet, &password)?;
    let me = signer.account.clone();
    let node = NodeClient::new(&cfg.node)?;
    let (engines, models) = engines(&cfg)?;
    let ports = Ports {
        chain: Arc::new(NodeChain { node, signer }),
        net: Arc::new(HttpNet {
            client: Client::new()?,
        }),
        exec: Arc::new(engines),
    };
    let mut agent = Agent::new(ports, me, Store::open(&cfg.data_dir)?, os_salts()?);
    agent.preflight(&models).await?;
    log::info!(target: TARGET, "worker running with {} models", models.len());
    loop {
        if let Err(e) = agent.tick().await {
            // RPC and transaction errors carry no data; the next tick retries.
            log::warn!(target: TARGET, "tick failed: {e:#}");
        }
        tokio::time::sleep(Duration::from_millis(cfg.poll_ms)).await;
    }
}
