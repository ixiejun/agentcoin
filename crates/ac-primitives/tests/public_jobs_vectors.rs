//! Regression vectors of the public jobs' hashing (m6-public-jobs task 1.4): a worker's
//! commitment, a canary tree with its proofs, a worker draw and fingerprint direction blocks.
//! Regenerate with `AC_WRITE_VECTORS=1 cargo test -p ac-primitives --test public_jobs_vectors`;
//! the file must not change otherwise.

#![allow(
    clippy::unwrap_used,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)] // Test code.

use ac_primitives::market::public::{
    Reveal, assign_unit, canary_leaf, canary_proof, canary_root, commitment, direction_block,
};
use parity_scale_codec::Encode;
use serde_json::{Value, json};
use sp_core::H256;

const PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/vectors/public_jobs.json"
);

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn vectors() -> Value {
    let worker = [0x33u8; 32];
    let summary = [1u8, 0, 3, 2];
    let c = commitment(&Reveal {
        job: 7,
        unit: 42,
        attempt: 1,
        worker: &worker,
        summary: &summary,
        result_hash: &[0x44; 32],
        salt: &[0x55; 32],
    });
    let leaves: Vec<[u8; 32]> = (0..5u32)
        .map(|u| canary_leaf(7, u * 3, &[u as u8; 32], &[0x66; 32]))
        .collect();
    let root = canary_root(&leaves).unwrap();
    let proofs: Vec<Value> = (0..leaves.len())
        .map(|i| json!(hex(&canary_proof(&leaves, i).unwrap().encode())))
        .collect();
    let roster: Vec<[u8; 32]> = (0..10u8).map(|i| [i; 32]).collect();
    let drawn: Vec<String> = assign_unit(&roster, &H256([0x77; 32]), (7, 42, 1), |_| true)
        .iter()
        .map(|w| hex(w))
        .collect();
    json!({
        "commitment": {
            "job": 7, "unit": 42, "attempt": 1, "worker": hex(&worker),
            "summary": hex(&summary), "result_hash": hex(&[0x44; 32]), "salt": hex(&[0x55; 32]),
            "commitment": hex(c.as_bytes()),
        },
        "canary": {
            "leaves": leaves.iter().map(|l| hex(l)).collect::<Vec<_>>(),
            "root": hex(&root),
            "proofs": proofs,
        },
        "assign": {
            "roster": "ten accounts [i; 32], i = 0..10",
            "seed": hex(&[0x77; 32]),
            "unit": [7, 42, 1],
            "drawn": drawn,
        },
        "direction": {
            "job": 7,
            "blocks": [hex(&direction_block(7, 0, 0)), hex(&direction_block(7, 31, 3))],
        },
    })
}

#[test]
fn public_jobs_vectors_are_stable() {
    let text = format!("{}\n", serde_json::to_string_pretty(&vectors()).unwrap());
    if std::env::var_os("AC_WRITE_VECTORS").is_some() {
        std::fs::write(PATH, &text).unwrap();
    }
    let stored = std::fs::read_to_string(PATH).unwrap();
    assert_eq!(
        stored, text,
        "public jobs vectors changed; see the module docs"
    );
}
