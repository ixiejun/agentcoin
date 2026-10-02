//! `ac-worker collect`: the publisher's result collector (spec `market/public-worker`
//! "结果收集服务", design D11).
//!
//! `PUT /<job>/<unit>` with the uploading worker in the `X-AgentCoin-Worker` header is saved to
//! `<dir>/<job>/<unit>` only when the unit passed, the uploader is in its majority, the body's
//! BLAKE3 hash is the result hash it revealed, and no result is saved yet. Otherwise the answer
//! is 403 or 409 with no explanation. With canary files configured, the collector reveals each
//! canary once its unit passed.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use ac_primitives::market::public::{CanaryReveal, JobId, UnitIndex, UnitState};
use ac_wallet::http::{self, Request, Response};
use ac_wallet::public::Unit;
use anyhow::{Context, Result};
use async_trait::async_trait;
use sp_runtime::AccountId32;
use tokio::sync::Mutex;

use crate::agent::live::WORKER_HEADER;
use crate::canary::CanaryFile;
use crate::logging::TARGET;
use crate::manifest::blake3;

/// Largest result accepted.
pub const MAX_RESULT: usize = 64 << 20;

/// What the collector needs from the chain.
#[async_trait]
pub trait CollectChain: Send + Sync {
    /// A unit's current attempt.
    async fn unit(&self, job: JobId, unit: UnitIndex) -> Result<Option<Unit>>;
    /// Sends `reveal_canary`; `Ok(false)` when it was included but failed.
    async fn reveal_canary(
        &self,
        job: JobId,
        unit: UnitIndex,
        canary: CanaryReveal,
    ) -> Result<bool>;
}

/// How an upload was answered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Saved (201).
    Saved,
    /// Not the majority's result (403).
    Refused,
    /// The unit has not passed, or its result is already saved (409).
    Conflict,
}

impl Outcome {
    fn status(self) -> u16 {
        match self {
            Self::Saved => 201,
            Self::Refused => 403,
            Self::Conflict => 409,
        }
    }
}

/// The collector.
pub struct Collector {
    chain: Arc<dyn CollectChain>,
    dir: PathBuf,
    canaries: Vec<CanaryFile>,
    revealed: Mutex<BTreeSet<(JobId, UnitIndex)>>,
    /// Serializes the "not saved yet" check and the write.
    saving: Mutex<()>,
}

impl Collector {
    /// A collector saving under `dir`, revealing `canaries` when their units pass.
    #[must_use]
    pub fn new(chain: Arc<dyn CollectChain>, dir: &Path, canaries: Vec<CanaryFile>) -> Self {
        Self {
            chain,
            dir: dir.to_path_buf(),
            canaries,
            revealed: Mutex::new(BTreeSet::new()),
            saving: Mutex::new(()),
        }
    }

    /// Where a unit's result is saved.
    #[must_use]
    pub fn path(&self, job: JobId, unit: UnitIndex) -> PathBuf {
        self.dir.join(job.to_string()).join(unit.to_string())
    }

    /// Checks an upload against the chain and saves it.
    ///
    /// # Errors
    ///
    /// RPC or write failures.
    pub async fn accept(
        &self,
        job: JobId,
        unit: UnitIndex,
        worker: &AccountId32,
        body: &[u8],
    ) -> Result<Outcome> {
        let Some(u) = self.chain.unit(job, unit).await? else {
            return Ok(Outcome::Conflict);
        };
        let UnitState::Accepted { majority, .. } = u.state else {
            return Ok(Outcome::Conflict);
        };
        let Some(slot) = u.assigned.iter().position(|w| w == worker) else {
            return Ok(Outcome::Refused);
        };
        let revealed_hash = u.reveals.get(slot).cloned().flatten().map(|(_, h)| h);
        if majority.get(slot) != Some(&true) || revealed_hash != Some(blake3(body)) {
            return Ok(Outcome::Refused);
        }
        let _saving = self.saving.lock().await;
        let path = self.path(job, unit);
        if path.exists() {
            return Ok(Outcome::Conflict);
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, body)?;
        std::fs::rename(&tmp, &path)?;
        log::info!(target: TARGET, "saved the result of job {job} unit {unit}");
        Ok(Outcome::Saved)
    }

    /// Reveals the canaries whose units passed.
    ///
    /// # Errors
    ///
    /// RPC failures.
    pub async fn reveal_canaries(&self) -> Result<()> {
        for file in &self.canaries {
            for c in &file.canaries {
                let key = (file.job, c.unit);
                if self.revealed.lock().await.contains(&key) {
                    continue;
                }
                let Some(u) = self.chain.unit(file.job, c.unit).await? else {
                    continue;
                };
                if u.canary_revealed {
                    self.revealed.lock().await.insert(key);
                    continue;
                }
                if !matches!(u.state, UnitState::Accepted { .. }) {
                    continue;
                }
                let ok = self
                    .chain
                    .reveal_canary(file.job, c.unit, c.reveal()?)
                    .await?;
                log::info!(
                    target: TARGET,
                    "revealed the canary of job {} unit {} ({})",
                    file.job,
                    c.unit,
                    if ok { "included" } else { "failed" }
                );
                self.revealed.lock().await.insert(key);
            }
        }
        Ok(())
    }
}

fn status(code: u16) -> Response {
    http::full(code, "text/plain", "")
}

/// Answers one request.
pub async fn handle(req: Request, collector: Arc<Collector>) -> Response {
    if req.method() != "PUT" {
        return status(405);
    }
    let mut parts = req.uri().path().trim_matches('/').split('/');
    let (Some(Ok(job)), Some(Ok(unit)), None) = (
        parts.next().map(str::parse::<JobId>),
        parts.next().map(str::parse::<UnitIndex>),
        parts.next(),
    ) else {
        return status(404);
    };
    let worker = req
        .headers()
        .get(WORKER_HEADER)
        .and_then(|v| v.to_str().ok())
        .and_then(|a| ac_wallet::parse_address(a).ok());
    let Some(worker) = worker else {
        return status(403);
    };
    let Ok(body) = http::read_body(req.into_body(), MAX_RESULT).await else {
        return status(413);
    };
    match collector.accept(job, unit, &worker, &body).await {
        Ok(outcome) => status(outcome.status()),
        Err(e) => {
            log::warn!(target: TARGET, "upload of job {job} unit {unit} not checked: {e:#}");
            status(503)
        }
    }
}

/// The collector's configuration (JSON):
///
/// ```json
/// {"node": "http://127.0.0.1:9944", "listen": "0.0.0.0:8600", "dir": "results",
///  "wallet": "publisher.json", "password_file": "publisher.pass",
///  "canaries": ["job-0-canaries.json"]}
/// ```
///
/// The wallet signs canary reveals and is needed only with canary files.
#[derive(Clone, Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CollectConfig {
    /// Node RPC URL.
    pub node: String,
    /// Where the service listens.
    pub listen: String,
    /// Where results are saved.
    pub dir: PathBuf,
    /// The account that reveals canaries.
    #[serde(default)]
    pub wallet: Option<PathBuf>,
    /// Its password file.
    #[serde(default)]
    pub password_file: Option<PathBuf>,
    /// Canary files of `ac-worker canary`.
    #[serde(default)]
    pub canaries: Vec<PathBuf>,
    /// Milliseconds between canary checks.
    #[serde(default = "default_poll")]
    pub poll_ms: u64,
}

const fn default_poll() -> u64 {
    2_000
}

impl CollectConfig {
    /// Reads a configuration file; relative paths are taken from the file's directory.
    ///
    /// # Errors
    ///
    /// An unreadable or malformed file.
    pub fn load(path: &Path) -> Result<Self> {
        let text =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        let mut c: Self = serde_json::from_str(&text)
            .with_context(|| format!("parsing the configuration {}", path.display()))?;
        let base = path.parent().unwrap_or_else(|| Path::new("."));
        let fix = |p: &mut PathBuf| {
            if p.is_relative() {
                *p = base.join(&*p);
            }
        };
        fix(&mut c.dir);
        if let Some(p) = c.wallet.as_mut() {
            fix(p);
        }
        if let Some(p) = c.password_file.as_mut() {
            fix(p);
        }
        c.canaries.iter_mut().for_each(fix);
        Ok(c)
    }
}

/// The node, and the signer of canary reveals.
pub struct NodeCollect {
    /// The node.
    pub node: ac_wallet::NodeClient,
    /// The publisher account's signer, if canaries are revealed.
    pub signer: Option<ac_wallet::ops::Signer>,
}

#[async_trait]
impl CollectChain for NodeCollect {
    async fn unit(&self, job: JobId, unit: UnitIndex) -> Result<Option<Unit>> {
        self.node.public_unit(job, unit).await
    }

    async fn reveal_canary(
        &self,
        job: JobId,
        unit: UnitIndex,
        canary: CanaryReveal,
    ) -> Result<bool> {
        let signer = self
            .signer
            .as_ref()
            .context("canary reveals need a wallet")?;
        let call = ac_runtime::RuntimeCall::PublicJobs(pallet_public_jobs::Call::reveal_canary {
            job,
            unit,
            canary: Box::new(canary),
        });
        Ok(signer.submit(&self.node, call).await?.success)
    }
}

/// `ac-worker collect`: serves uploads and reveals canaries until stopped.
///
/// # Errors
///
/// A bad configuration, a wrong password or an address that cannot be bound.
pub async fn run(cfg: CollectConfig) -> Result<()> {
    let canaries = cfg
        .canaries
        .iter()
        .map(|p| -> Result<CanaryFile> {
            let bytes = std::fs::read(p).with_context(|| format!("reading {}", p.display()))?;
            Ok(serde_json::from_slice(&bytes)?)
        })
        .collect::<Result<Vec<_>>>()?;
    let signer = match (&cfg.wallet, &cfg.password_file) {
        (Some(w), Some(p)) => {
            let password = ac_wallet::wallet::read_password_file(p)?;
            Some(ac_wallet::ops::Signer::from_wallet(
                &ac_wallet::Wallet::load(w)?,
                &password,
            )?)
        }
        _ => None,
    };
    anyhow::ensure!(
        canaries.is_empty() || signer.is_some(),
        "canary files need a wallet and its password file"
    );
    let chain = Arc::new(NodeCollect {
        node: ac_wallet::NodeClient::new(&cfg.node)?,
        signer,
    });
    std::fs::create_dir_all(&cfg.dir)?;
    let collector = Arc::new(Collector::new(chain, &cfg.dir, canaries));
    let listener = tokio::net::TcpListener::bind(&cfg.listen).await?;
    println!("listening on {}", listener.local_addr()?);
    let served = Arc::clone(&collector);
    tokio::spawn(http::serve(listener, move |req| {
        let c = Arc::clone(&served);
        async move { handle(req, c).await }
    }));
    loop {
        if let Err(e) = collector.reveal_canaries().await {
            log::warn!(target: TARGET, "canary check failed: {e:#}");
        }
        tokio::time::sleep(std::time::Duration::from_millis(cfg.poll_ms)).await;
    }
}

#[cfg(test)]
mod tests;
