//! Requirement "规范与向量一致性" / Scenario "ML-KEM-768 NIST 向量" (crypto/hybrid-kem).
//!
//! Exercises the ML-KEM-768 component that X-Wing is built on.
// Integration-test crate: AGENT.md §5.3 permits unwrap/expect/panics in test code.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable,
    clippy::indexing_slicing
)]

mod common;

use common::{load, unhex};
use ml_kem::{Decapsulate, KeyExport, MlKem768, Seed};

#[test]
fn acvp_keygen() {
    let cases = load("ml_kem_768_keygen.json");
    let cases = cases.as_array().unwrap();
    assert!(cases.len() >= 10);
    for c in cases {
        let h = |k: &str| unhex(c[k].as_str().unwrap());
        let mut seed = h("d");
        seed.extend(h("z"));
        let dk = ml_kem::DecapsulationKey::<MlKem768>::from_seed(
            Seed::try_from(seed.as_slice()).unwrap(),
        );
        assert_eq!(
            dk.encapsulation_key().to_bytes().as_slice(),
            h("ek"),
            "tcId {}",
            c["tcId"]
        );
        assert_eq!(expanded(&dk), h("dk"), "tcId {}", c["tcId"]);
    }
}

// ACVP publishes expanded decapsulation keys; the expanded encoding is deprecated upstream.
#[allow(deprecated)]
fn expanded(dk: &ml_kem::DecapsulationKey<MlKem768>) -> Vec<u8> {
    use ml_kem::ExpandedKeyEncoding;
    dk.to_expanded_bytes().to_vec()
}

#[test]
#[allow(deprecated)] // ACVP decapsulation vectors use expanded keys.
fn acvp_encapsulation_and_decapsulation() {
    let v = load("ml_kem_768_encapdecap.json");
    let enc = v["encapsulation"].as_array().unwrap();
    let dec = v["decapsulation"].as_array().unwrap();
    assert!(enc.len() >= 10 && dec.len() >= 10);
    for c in enc {
        let h = |k: &str| unhex(c[k].as_str().unwrap());
        let ek = ml_kem::EncapsulationKey::<MlKem768>::new(h("ek").as_slice().try_into().unwrap())
            .unwrap();
        let (ct, k) = ek.encapsulate_deterministic(h("m").as_slice().try_into().unwrap());
        assert_eq!(ct.as_slice(), h("c"), "tcId {}", c["tcId"]);
        assert_eq!(k.as_slice(), h("k"), "tcId {}", c["tcId"]);
    }
    for c in dec {
        let h = |k: &str| unhex(c[k].as_str().unwrap());
        let dk = ml_kem::DecapsulationKey::<MlKem768>::from_expanded(
            h("dk").as_slice().try_into().unwrap(),
        )
        .unwrap();
        let k = dk.decapsulate(h("c").as_slice().try_into().unwrap());
        assert_eq!(k.as_slice(), h("k"), "tcId {}", c["tcId"]);
    }
}
