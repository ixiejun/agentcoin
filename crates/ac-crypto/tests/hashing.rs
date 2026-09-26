//! Requirement "256 位哈希" / Scenario "官方向量" (crypto/hashing).
// Integration-test crate: AGENT.md §5.3 permits unwrap/expect/panics in test code.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable,
    clippy::indexing_slicing
)]

mod common;

use ac_crypto::hash::{blake3_256, sha3_256};
use common::unhex;

#[test]
fn blake3_official_vectors() {
    // From the BLAKE3 reference `test_vectors.json` (input byte i = i mod 251), key-less hash.
    let input = |n: usize| {
        (0..n)
            .map(|i| u8::try_from(i % 251).unwrap())
            .collect::<Vec<u8>>()
    };
    assert_eq!(
        blake3_256(&input(0)).to_vec(),
        unhex("af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262")
    );
    assert_eq!(
        blake3_256(&input(1)).to_vec(),
        unhex("2d3adedff11b61f14c886e35afa036736dcd87a74d27b5c1510225d0f592e213")
    );
    assert_eq!(
        blake3_256(&input(1024)).to_vec(),
        unhex("42214739f095a406f3fc83deb889744ac00df831c10daa55189b5d121c855af7")
    );
}

#[test]
fn sha3_256_nist_examples() {
    // FIPS 202 examples (NIST CSRC "SHA3-256" example values).
    assert_eq!(
        sha3_256(b"").to_vec(),
        unhex("a7ffc6f8bf1ed76651c14756a061d662f580ff4de43b49fa82d80a4b80f8434a")
    );
    assert_eq!(
        sha3_256(b"abc").to_vec(),
        unhex("3a985da74fe225b2045c172d6bd390bd855f086e3e9d525b46bfe24511431532")
    );
}

#[test]
fn outputs_are_32_bytes() {
    assert_eq!(blake3_256(b"x").len(), 32);
    assert_eq!(sha3_256(b"x").len(), 32);
}
