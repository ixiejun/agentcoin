//! Public jobs (m6-public-jobs; spec `clients/wallet-cli` "公共任务子命令"): reads through
//! `PublicJobsApi`, shared by the wallet's `public` commands and `ac-worker`.

use ac_primitives::emission::EpochIndex;
use ac_primitives::market::audit::RoundIndex;
use ac_primitives::market::public::{
    Assignment, CanaryProof, CanaryReveal, CanarySiblings, EpochPublic, JobId, JobRecord, JobSpec,
    PublicParams, RULES_V1, Summary, UnitIndex, UnitRecord, Url, WorkerRecord, canary_leaf,
    canary_proof, canary_root, check_spec,
};
use ac_primitives::market::work::JobKind;
use ac_primitives::market::{MicroUsd, ModelId};
use anyhow::{Context, Result, bail, ensure};
use parity_scale_codec::{Decode, Encode};
use serde::{Deserialize, Serialize};
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

/// Parses a public job kind: `eval`, `embed` or `clean`.
///
/// # Errors
///
/// Anything else.
pub fn parse_kind(text: &str) -> Result<JobKind> {
    match text {
        "eval" => Ok(JobKind::Eval),
        "embed" => Ok(JobKind::Embed),
        "clean" => Ok(JobKind::DataClean),
        other => bail!("a public job kind is `eval`, `embed` or `clean`, not `{other}`"),
    }
}

/// What a publisher asks for, before the local checks.
#[derive(Clone, Debug)]
pub struct Publish {
    /// The kind.
    pub kind: JobKind,
    /// The model (evaluation and embedding).
    pub model: Option<ModelId>,
    /// The data manifest's bytes, as served at `manifest_url`.
    pub manifest: Vec<u8>,
    /// Where the manifest is served.
    pub manifest_url: String,
    /// Where workers upload results.
    pub results_url: String,
    /// Number of units.
    pub units: u32,
    /// Price per unit.
    pub price: MicroUsd,
    /// Canary Merkle root.
    pub canary_root: Option<[u8; 32]>,
}

/// The job spec of `p`, checked locally (spec `clients/wallet-cli` "公共任务子命令"): the
/// manifest lists exactly `units` shards, and the spec passes the chain's checks with
/// `price_cap`.
///
/// # Errors
///
/// A malformed manifest, a unit count that differs from it, or a spec the chain would refuse.
pub fn job_spec(p: Publish, price_cap: MicroUsd) -> Result<JobSpec> {
    let manifest: serde_json::Value =
        serde_json::from_slice(&p.manifest).context("the manifest is not JSON")?;
    let listed = manifest
        .get("units")
        .and_then(serde_json::Value::as_array)
        .context("the manifest has no unit list")?
        .len();
    if u32::try_from(listed).ok() != Some(p.units) {
        bail!(
            "the manifest lists {listed} units, not {} as given",
            p.units
        );
    }
    let url =
        |s: String| Url::try_from(s.into_bytes()).map_err(|_| anyhow::anyhow!("a URL is too long"));
    let spec = JobSpec {
        kind: p.kind,
        model: p.model,
        rules: RULES_V1,
        manifest_hash: ac_crypto::hash::blake3_256(&p.manifest),
        manifest_url: url(p.manifest_url)?,
        results_url: url(p.results_url)?,
        units: p.units,
        price: p.price,
        canary_root: p.canary_root,
    };
    check_spec(&spec, price_cap).map_err(|e| anyhow::anyhow!("the job would be refused: {e}"))?;
    Ok(spec)
}

/// One canary unit.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Canary {
    /// The unit.
    pub unit: UnitIndex,
    /// The expected summary, hexadecimal.
    pub summary: String,
    /// The leaf's salt, hexadecimal.
    pub salt: String,
    /// The leaf's position.
    pub index: u32,
    /// The tree's number of leaves.
    pub leaves: u32,
    /// The proof's siblings, leaf level first, hexadecimal.
    pub siblings: Vec<String>,
}

/// A job's canaries.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CanaryFile {
    /// The job.
    pub job: JobId,
    /// The Merkle root the job is published with, hexadecimal.
    pub root: String,
    /// The canaries, in unit order.
    pub canaries: Vec<Canary>,
}

fn hex32(s: &str) -> Result<[u8; 32]> {
    let v = hex::decode(s.trim_start_matches("0x"))?;
    <[u8; 32]>::try_from(v.as_slice()).context("not 32 bytes")
}

impl Canary {
    /// The `reveal_canary` argument for this leaf.
    ///
    /// # Errors
    ///
    /// A corrupt file.
    pub fn reveal(&self) -> Result<CanaryReveal> {
        let siblings = self
            .siblings
            .iter()
            .map(|s| hex32(s))
            .collect::<Result<Vec<_>>>()?;
        Ok(CanaryReveal {
            summary: Summary::try_from(hex::decode(&self.summary)?)
                .map_err(|_| anyhow::anyhow!("summary too long"))?,
            salt: hex32(&self.salt)?,
            proof: CanaryProof {
                index: self.index,
                leaves: self.leaves,
                siblings: CanarySiblings::try_from(siblings)
                    .map_err(|_| anyhow::anyhow!("proof too deep"))?,
            },
        })
    }
}

impl CanaryFile {
    /// Builds the canaries of `job` from each unit's expected summary; `salt` draws the leaves'
    /// salts.
    ///
    /// # Errors
    ///
    /// No units, a unit given twice, too many units or a failing random source.
    pub fn build(
        job: JobId,
        units: &[(UnitIndex, Vec<u8>)],
        mut salt: impl FnMut() -> Result<[u8; 32]>,
    ) -> Result<Self> {
        ensure!(!units.is_empty(), "no canary units");
        let mut sorted = units.to_vec();
        sorted.sort_by_key(|(u, _)| *u);
        ensure!(
            sorted
                .windows(2)
                .all(|w| w.first().map(|a| a.0) != w.get(1).map(|b| b.0)),
            "a canary unit is given twice"
        );
        let salts = sorted.iter().map(|_| salt()).collect::<Result<Vec<_>>>()?;
        let leaves: Vec<[u8; 32]> = sorted
            .iter()
            .zip(&salts)
            .map(|((unit, summary), s)| canary_leaf(job, *unit, summary, s))
            .collect();
        let root = canary_root(&leaves).context("no leaves")?;
        let canaries = sorted
            .iter()
            .zip(&salts)
            .enumerate()
            .map(|(i, ((unit, summary), s))| {
                let proof = canary_proof(&leaves, i).context("too many canary units")?;
                Ok(Canary {
                    unit: *unit,
                    summary: hex::encode(summary),
                    salt: hex::encode(s),
                    index: proof.index,
                    leaves: proof.leaves,
                    siblings: proof.siblings.iter().map(hex::encode).collect(),
                })
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(Self {
            job,
            root: hex::encode(root),
            canaries,
        })
    }

    /// The root.
    ///
    /// # Errors
    ///
    /// A corrupt file.
    pub fn root(&self) -> Result<[u8; 32]> {
        hex32(&self.root)
    }

    /// The canary of `unit`, if it is one.
    #[must_use]
    pub fn get(&self, unit: UnitIndex) -> Option<&Canary> {
        self.canaries.iter().find(|c| c.unit == unit)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)] // Test code.

    use super::*;

    fn manifest(units: usize) -> Vec<u8> {
        let shards: Vec<_> = (0..units)
            .map(|i| serde_json::json!({"url": format!("https://d/{i}"), "blake3": "00", "items": 1}))
            .collect();
        serde_json::to_vec(&serde_json::json!({ "units": shards })).unwrap()
    }

    fn publish(units: u32, manifest_units: usize) -> Publish {
        Publish {
            kind: JobKind::DataClean,
            model: None,
            manifest: manifest(manifest_units),
            manifest_url: "https://d/m.json".into(),
            results_url: "https://r/".into(),
            units,
            price: MicroUsd(10_000),
            canary_root: None,
        }
    }

    // Spec "清单与单元数不符".
    #[test]
    fn a_unit_count_that_differs_from_the_manifest_is_refused() {
        let err = job_spec(publish(100, 90), MicroUsd(1_000_000)).unwrap_err();
        assert!(err.to_string().contains("90 units"), "{err}");
        let spec = job_spec(publish(90, 90), MicroUsd(1_000_000)).unwrap();
        assert_eq!(spec.units, 90);
        assert_eq!(
            spec.manifest_hash,
            ac_crypto::hash::blake3_256(&manifest(90))
        );
    }

    #[test]
    fn the_chains_checks_run_locally() {
        // Above the price cap.
        assert!(job_spec(publish(3, 3), MicroUsd(5_000)).is_err());
        // An evaluation job needs a model; data cleaning takes none.
        let mut p = publish(3, 3);
        p.kind = JobKind::Eval;
        assert!(job_spec(p.clone(), MicroUsd(1_000_000)).is_err());
        p.model = Some(ModelId([1; 32]));
        assert!(job_spec(p, MicroUsd(1_000_000)).is_ok());
        let mut p = publish(3, 3);
        p.manifest = b"not json".to_vec();
        assert!(job_spec(p, MicroUsd(1_000_000)).is_err());
    }

    #[test]
    fn kinds_parse() {
        assert_eq!(parse_kind("eval").unwrap(), JobKind::Eval);
        assert_eq!(parse_kind("embed").unwrap(), JobKind::Embed);
        assert_eq!(parse_kind("clean").unwrap(), JobKind::DataClean);
        assert!(parse_kind("train").is_err());
    }
}
