//! The publisher's canaries (spec `market/public-worker` "金丝雀工具", design D11).
//!
//! For each chosen unit the publisher computes the expected summary with the workers' own
//! executors, draws a 32-byte salt per leaf from the operating system, and builds the Merkle
//! tree whose root goes into the job. The file of leaves and proofs stays with the publisher
//! until each unit passes, when `collect` (or the wallet) reveals it.

pub use ac_wallet::public::{Canary, CanaryFile};

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)] // Test code.

    use ac_primitives::market::MicroUsd;
    use ac_primitives::market::public::{
        JobRecord, JobSpec, PublicParams, RULES_V1, Summary, UnitIndex, UnitRecord, UnitState, Url,
        canary_leaf, verify_canary,
    };
    use ac_primitives::market::work::JobKind;
    use ac_runtime::{Runtime, RuntimeCall, RuntimeOrigin};
    use anyhow::Result;
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
