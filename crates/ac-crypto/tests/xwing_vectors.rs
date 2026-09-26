//! Requirement "规范与向量一致性" / Scenario "X-Wing 规范向量" (crypto/hybrid-kem).
#![cfg(all(feature = "kem", feature = "deterministic"))]
// Integration-test crate: AGENT.md §5.3 permits unwrap/expect/panics in test code.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable,
    clippy::indexing_slicing
)]

mod common;

use ac_crypto::kem::{KemSecretKey, encapsulate_derandomized};
use ac_crypto::sig::SecretSeed;
use ac_crypto::{KemAlg, KemCiphertext};
use common::{load, unhex};

#[test]
fn xwing_draft06_vectors() {
    let cases = load("xwing_draft06.json");
    let cases = cases.as_array().unwrap();
    assert!(!cases.is_empty());
    for c in cases {
        let h = |k: &str| unhex(c[k].as_str().unwrap());
        let seed: [u8; 32] = h("seed").try_into().unwrap();
        let sk = KemSecretKey::from_seed(KemAlg::XWing, &SecretSeed::new(seed)).unwrap();
        let pk = sk.public_key().unwrap();
        assert_eq!(pk.as_bytes(), h("pk"));
        let eseed: [u8; 64] = h("eseed").try_into().unwrap();
        let (ct, ss) = encapsulate_derandomized(&pk, &eseed).unwrap();
        assert_eq!(ct.as_bytes(), h("ct"));
        assert_eq!(ss.expose().as_slice(), h("ss"));
        let ct = KemCiphertext::new(KemAlg::XWing, &h("ct")).unwrap();
        assert_eq!(sk.decapsulate(&ct).unwrap().expose().as_slice(), h("ss"));
    }
}
