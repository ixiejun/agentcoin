//! Requirement "Poseidon2 哈希" (crypto/hashing, m4-evm design D7).
#![cfg(feature = "poseidon2")]
// Integration-test crate: AGENT.md §5.3 permits unwrap/expect/panics in test code.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

mod common;

use ac_crypto::poseidon2::{self, RATE, WIDTH};
use common::{load, unhex};
use proptest::prelude::{any, prop_assert_ne, proptest};

fn element(value: &serde_json::Value) -> u64 {
    u64::from_str_radix(value.as_str().unwrap(), 16).unwrap()
}

// Scenario "上游置换向量": Plonky3's published known-answer vector for the instance.
#[test]
fn permutation_matches_the_upstream_vector() {
    let v = load("poseidon2_goldilocks_12.json");
    let mut state = [0u64; WIDTH];
    for (slot, x) in state.iter_mut().zip(v["input"].as_array().unwrap()) {
        *slot = element(x);
    }
    poseidon2::permute(&mut state);
    let expected: Vec<u64> = v["output"]
        .as_array()
        .unwrap()
        .iter()
        .map(element)
        .collect();
    assert_eq!(state.to_vec(), expected);
}

fn input(len: usize) -> Vec<u8> {
    (0..len).map(|i| u8::try_from(i % 251).unwrap()).collect()
}

// Scenario "哈希回归向量": empty, 31, 32 and 1,000 bytes.
#[test]
fn hashes_match_the_regression_vectors() {
    let v = load("poseidon2_hash.json");
    let cases = v["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 4);
    for case in cases {
        let len = usize::try_from(case["input_len"].as_u64().unwrap()).unwrap();
        assert_eq!(
            poseidon2::hash(&input(len)).unwrap().to_vec(),
            unhex(case["hash"].as_str().unwrap()),
            "{len} bytes"
        );
    }
}

// The library sponge equals the design's construction built from the vector-checked
// permutation: overwrite the rate with each block of 8 elements, permute, output 4 elements.
#[test]
fn sponge_is_the_designed_construction() {
    for len in [0usize, 6, 7, 55, 56, 57, 200] {
        let mut state = [0u64; WIDTH];
        for block in poseidon2::encode(&input(len)).unwrap().chunks(RATE) {
            state[..RATE].copy_from_slice(block);
            poseidon2::permute(&mut state);
        }
        let expected: Vec<u8> = state[..4].iter().flat_map(|e| e.to_le_bytes()).collect();
        assert_eq!(
            poseidon2::hash(&input(len)).unwrap().to_vec(),
            expected,
            "{len}"
        );
    }
}

// Scenario "编码单射": one more trailing zero byte changes the hash; encodings of different
// inputs differ.
#[test]
fn trailing_zero_changes_the_hash() {
    for len in [0usize, 1, 6, 7, 13, 14, 55, 56] {
        let a = input(len);
        let mut b = a.clone();
        b.push(0);
        assert_ne!(
            poseidon2::encode(&a).unwrap(),
            poseidon2::encode(&b).unwrap()
        );
        assert_ne!(poseidon2::hash(&a).unwrap(), poseidon2::hash(&b).unwrap());
    }
}

proptest! {
    #[test]
    fn distinct_inputs_have_distinct_encodings(
        a in proptest::collection::vec(any::<u8>(), 0..120),
        b in proptest::collection::vec(any::<u8>(), 0..120),
    ) {
        if a != b {
            prop_assert_ne!(poseidon2::encode(&a).unwrap(), poseidon2::encode(&b).unwrap());
        }
    }
}
