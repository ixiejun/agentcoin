//! Requirement "NIST 签名与验证向量一致性" and "从种子确定性生成密钥" (crypto/pq-signature).
#![cfg(feature = "deterministic")]
// Integration-test crate: AGENT.md §5.3 permits unwrap/expect/panics in test code.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable,
    clippy::indexing_slicing
)]

mod common;

use ac_crypto::sig::{SecretSeed, SigningKey, verify};
use ac_crypto::{Error, PqPublicKey, PqSignature, SigAlg};
use common::{load, unhex};
use serde_json::Value;

fn alg(v: &Value) -> SigAlg {
    match v["parameterSet"].as_str().unwrap() {
        "ML-DSA-44" => SigAlg::MlDsa44,
        "ML-DSA-65" => SigAlg::MlDsa65,
        "ML-DSA-87" => SigAlg::MlDsa87,
        other => panic!("unexpected parameter set {other}"),
    }
}

fn s(v: &Value, k: &str) -> Vec<u8> {
    unhex(v[k].as_str().unwrap())
}

fn assert_per_set(cases: &[Value], min: usize) {
    for a in [SigAlg::MlDsa44, SigAlg::MlDsa65, SigAlg::MlDsa87] {
        let n = cases.iter().filter(|c| alg(c) == a).count();
        assert!(n >= min, "{a:?} has only {n} cases");
    }
}

// Scenario "与 NIST keyGen 向量一致" (public key and expanded private key).
#[test]
fn acvp_keygen() {
    let cases = load("ml_dsa_keygen.json");
    let cases = cases.as_array().unwrap();
    assert_per_set(cases, 10);
    for c in cases {
        let seed: [u8; 32] = s(c, "seed").try_into().unwrap();
        let sk = SigningKey::from_seed(alg(c), &SecretSeed::new(seed)).unwrap();
        assert_eq!(
            sk.public_key().unwrap().as_bytes(),
            s(c, "pk"),
            "tcId {}",
            c["tcId"]
        );
        // The expanded private-key encoding is checked against the backend directly: this
        // crate only exposes seed-based keys.
        let expected_sk = s(c, "sk");
        let got_sk = expanded_sk(alg(c), &seed);
        assert_eq!(got_sk, expected_sk, "tcId {}", c["tcId"]);
    }
}

#[allow(deprecated)] // `to_expanded` is deprecated upstream but is what ACVP publishes.
fn expanded_sk(alg: SigAlg, seed: &[u8; 32]) -> Vec<u8> {
    use ml_dsa::{B32, MlDsa44, MlDsa65, MlDsa87, SigningKey as K};
    let xi = B32::from(*seed);
    match alg {
        SigAlg::MlDsa44 => K::<MlDsa44>::from_seed(&xi)
            .expanded_key()
            .to_expanded()
            .to_vec(),
        SigAlg::MlDsa65 => K::<MlDsa65>::from_seed(&xi)
            .expanded_key()
            .to_expanded()
            .to_vec(),
        SigAlg::MlDsa87 => K::<MlDsa87>::from_seed(&xi)
            .expanded_key()
            .to_expanded()
            .to_vec(),
        _ => unreachable!(),
    }
}

// Scenario "sigGen 向量": deterministic signing with the published expanded key, then
// verification through this crate's API.
#[test]
#[allow(deprecated)] // ACVP sigGen publishes expanded keys only.
fn acvp_siggen() {
    use ml_dsa::{ExpandedSigningKey as E, MlDsa44, MlDsa65, MlDsa87};
    let cases = load("ml_dsa_siggen.json");
    let cases = cases.as_array().unwrap();
    assert_per_set(cases, 10);
    for c in cases {
        let (sk, msg, ctx, expected) = (
            s(c, "sk"),
            s(c, "message"),
            s(c, "context"),
            s(c, "signature"),
        );
        let sig = match alg(c) {
            SigAlg::MlDsa44 => E::<MlDsa44>::from_expanded(sk.as_slice().try_into().unwrap())
                .sign_deterministic(&msg, &ctx)
                .unwrap()
                .encode()
                .to_vec(),
            SigAlg::MlDsa65 => E::<MlDsa65>::from_expanded(sk.as_slice().try_into().unwrap())
                .sign_deterministic(&msg, &ctx)
                .unwrap()
                .encode()
                .to_vec(),
            SigAlg::MlDsa87 => E::<MlDsa87>::from_expanded(sk.as_slice().try_into().unwrap())
                .sign_deterministic(&msg, &ctx)
                .unwrap()
                .encode()
                .to_vec(),
            _ => unreachable!(),
        };
        assert_eq!(sig, expected, "tcId {}", c["tcId"]);
        let pk = PqPublicKey::new(alg(c), &derive_pk(alg(c), &sk)).unwrap();
        let sig = PqSignature::new(alg(c), &expected).unwrap();
        assert_eq!(verify(&pk, &msg, &ctx, &sig), Ok(()), "tcId {}", c["tcId"]);
    }
}

#[allow(deprecated)]
fn derive_pk(alg: SigAlg, sk: &[u8]) -> Vec<u8> {
    use ml_dsa::{ExpandedSigningKey as E, MlDsa44, MlDsa65, MlDsa87};
    match alg {
        SigAlg::MlDsa44 => E::<MlDsa44>::from_expanded(sk.try_into().unwrap())
            .verifying_key()
            .encode()
            .to_vec(),
        SigAlg::MlDsa65 => E::<MlDsa65>::from_expanded(sk.try_into().unwrap())
            .verifying_key()
            .encode()
            .to_vec(),
        SigAlg::MlDsa87 => E::<MlDsa87>::from_expanded(sk.try_into().unwrap())
            .verifying_key()
            .encode()
            .to_vec(),
        _ => unreachable!(),
    }
}

// Scenario "sigVer 向量": this crate's verify agrees with every expected result.
#[test]
fn acvp_sigver() {
    let cases = load("ml_dsa_sigver.json");
    let cases = cases.as_array().unwrap();
    assert_per_set(cases, 10);
    let passed = cases
        .iter()
        .filter(|c| c["testPassed"].as_bool().unwrap())
        .count();
    assert!(passed > 0 && passed < cases.len(), "need both outcomes");
    for c in cases {
        let a = alg(c);
        let pk = PqPublicKey::new(a, &s(c, "pk")).unwrap();
        let expected = c["testPassed"].as_bool().unwrap();
        let result = match PqSignature::new(a, &s(c, "signature")) {
            Ok(sig) => verify(&pk, &s(c, "message"), &s(c, "context"), &sig),
            Err(e) => Err(e),
        };
        assert_eq!(
            result.is_ok(),
            expected,
            "tcId {} ({})",
            c["tcId"],
            c["reason"]
        );
        if !expected {
            assert!(matches!(
                result,
                Err(Error::InvalidSignature | Error::InvalidLength { .. })
            ));
        }
    }
}
