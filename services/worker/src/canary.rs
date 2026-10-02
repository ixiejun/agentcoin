//! The publisher's canaries (spec `market/public-worker` "金丝雀工具", design D11).
//!
//! For each chosen unit the publisher computes the expected summary with the workers' own
//! executors, draws a 32-byte salt per leaf from the operating system, and builds the Merkle
//! tree whose root goes into the job. The file of leaves and proofs stays with the publisher
//! until each unit passes, when `collect` (or the wallet) reveals it.

use ac_primitives::market::public::{
    CanaryProof, CanaryReveal, CanarySiblings, JobId, Summary, UnitIndex, canary_leaf,
    canary_proof, canary_root,
};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};

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
    #![allow(clippy::unwrap_used, clippy::expect_used)] // Test code.

    use ac_primitives::market::MicroUsd;
    use ac_primitives::market::public::{
        JobRecord, JobSpec, PublicParams, RULES_V1, UnitRecord, UnitState, Url, verify_canary,
    };
    use ac_primitives::market::work::JobKind;
    use ac_runtime::{Runtime, RuntimeCall, RuntimeOrigin};
    use sp_runtime::traits::Dispatchable;
    use sp_runtime::{AccountId32, BuildStorage};

    use super::*;

    fn summaries() -> Vec<(UnitIndex, Vec<u8>)> {
        [4u32, 0, 9, 2, 7]
            .iter()
            .map(|u| (*u, ac_crypto::hash::blake3_256(&u.to_le_bytes()).to_vec()))
            .collect()
    }

    fn salts() -> impl FnMut() -> Result<[u8; 32]> {
        let mut n = 0u8;
        move || {
            n = n.wrapping_add(1);
            Ok([n; 32])
        }
    }

    // Spec "根与证明一致": every leaf's proof leads to the root.
    #[test]
    fn every_proof_leads_to_the_root() {
        let file = CanaryFile::build(3, &summaries(), salts()).unwrap();
        assert_eq!(
            file.canaries.iter().map(|c| c.unit).collect::<Vec<_>>(),
            vec![0, 2, 4, 7, 9]
        );
        let root = file.root().unwrap();
        for c in &file.canaries {
            let r = c.reveal().unwrap();
            let leaf = canary_leaf(3, c.unit, &r.summary, &r.salt);
            assert!(verify_canary(&root, &leaf, &r.proof));
            // Another unit's leaf does not verify with this proof.
            let other = canary_leaf(3, c.unit + 100, &r.summary, &r.salt);
            assert!(!verify_canary(&root, &other, &r.proof));
        }
        let json = serde_json::to_string(&file).unwrap();
        assert_eq!(serde_json::from_str::<CanaryFile>(&json).unwrap(), file);
        assert!(CanaryFile::build(3, &[], salts()).is_err());
        assert!(CanaryFile::build(3, &[(1, vec![0; 32]), (1, vec![1; 32])], salts()).is_err());
    }

    // Spec "根与证明一致": the chain accepts the reveal the tool's output makes, and the unit's
    // canary passes when the majority revealed the expected summary.
    #[test]
    fn the_chain_accepts_the_tools_reveal() {
        let units = summaries();
        let file = CanaryFile::build(0, &units, salts()).unwrap();
        let mut storage = frame_system::GenesisConfig::<Runtime>::default()
            .build_storage()
            .unwrap();
        pallet_public_jobs::GenesisConfig::<Runtime> {
            params: PublicParams::DEV,
            _marker: Default::default(),
        }
        .assimilate_storage(&mut storage)
        .unwrap();
        sp_io::TestExternalities::new(storage).execute_with(|| {
            let spec = JobSpec {
                kind: JobKind::DataClean,
                model: None,
                rules: RULES_V1,
                manifest_hash: [1; 32],
                manifest_url: Url::truncate_from(b"https://d/m".to_vec()),
                results_url: Url::truncate_from(b"https://r/".to_vec()),
                units: 10,
                price: MicroUsd(10_000),
                canary_root: Some(file.root().unwrap()),
            };
            pallet_public_jobs::Jobs::<Runtime>::insert(
                0,
                JobRecord {
                    spec,
                    published_at: 1,
                    opened: 10,
                    accepted: 1,
                    failed: 0,
                    cancelled: false,
                },
            );
            let canary = file.get(7).unwrap();
            let expected = Summary::truncate_from(hex::decode(&canary.summary).unwrap());
            let w = |b: u8| AccountId32::new([b; 32]);
            pallet_public_jobs::Units::<Runtime>::insert(
                0,
                7,
                UnitRecord {
                    attempt: 1,
                    opened_at: 1,
                    commit_by: 20,
                    reveal_by: 30,
                    assigned: [w(1), w(2), w(3)],
                    tried: Default::default(),
                    commits: [None; 3],
                    reveals: [
                        Some((expected.clone(), [0; 32])),
                        Some((expected.clone(), [0; 32])),
                        Some((expected, [0; 32])),
                    ],
                    state: UnitState::Accepted {
                        reference: 0,
                        majority: [true; 3],
                    },
                    settled_at: Some(30),
                    canary_revealed: false,
                },
            );
            RuntimeCall::PublicJobs(pallet_public_jobs::Call::reveal_canary {
                job: 0,
                unit: 7,
                canary: Box::new(canary.reveal().unwrap()),
            })
            .dispatch(RuntimeOrigin::signed(w(9)))
            .unwrap();
            let u = pallet_public_jobs::Units::<Runtime>::get(0, 7).unwrap();
            assert!(u.canary_revealed);
            assert!(matches!(u.state, UnitState::Accepted { .. }));
        });
    }
}
