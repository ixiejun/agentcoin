//! Behavior of ML-DSA signing and verification (crypto/pq-signature).
#![cfg(all(feature = "rand", feature = "deterministic"))]
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
use common::TestRng;

const TX: &[u8] = b"agentcoin/tx/v1";
const VOTE: &[u8] = b"agentcoin/bft-vote/v1";

fn key(alg: SigAlg) -> SigningKey {
    SigningKey::from_seed(alg, &SecretSeed::new([42u8; 32])).unwrap()
}

// Requirement "从种子确定性生成密钥" / Scenario "相同种子得到相同密钥".
#[test]
fn same_seed_same_key() {
    for alg in [SigAlg::MlDsa44, SigAlg::MlDsa65, SigAlg::MlDsa87] {
        assert_eq!(
            key(alg).public_key().unwrap(),
            key(alg).public_key().unwrap()
        );
        let other = SigningKey::from_seed(alg, &SecretSeed::new([43u8; 32])).unwrap();
        assert_ne!(key(alg).public_key().unwrap(), other.public_key().unwrap());
    }
}

#[test]
fn generate_from_rng() {
    let mut rng = TestRng::new("generate");
    let a = SigningKey::generate(SigAlg::MlDsa44, &mut rng).unwrap();
    let b = SigningKey::generate(SigAlg::MlDsa44, &mut rng).unwrap();
    assert_ne!(a.public_key().unwrap(), b.public_key().unwrap());
}

// Requirement "带上下文的签名与验证".
#[test]
fn context_binding() {
    let mut rng = TestRng::new("context");
    for alg in [SigAlg::MlDsa44, SigAlg::MlDsa65, SigAlg::MlDsa87] {
        let sk = key(alg);
        let pk = sk.public_key().unwrap();
        let sig = sk.sign(b"hello", TX, &mut rng).unwrap();
        // Scenario "同一上下文验证通过".
        assert_eq!(verify(&pk, b"hello", TX, &sig), Ok(()));
        // Scenario "不同上下文验证失败".
        assert_eq!(
            verify(&pk, b"hello", VOTE, &sig),
            Err(Error::InvalidSignature)
        );
    }
}

// Scenario "上下文过长".
#[test]
fn context_too_long() {
    let mut rng = TestRng::new("long");
    let sk = key(SigAlg::MlDsa44);
    let pk = sk.public_key().unwrap();
    let long = [b'a'; 256];
    assert_eq!(
        sk.sign(b"m", &long, &mut rng).unwrap_err(),
        Error::ContextTooLong
    );
    assert_eq!(
        sk.sign_deterministic(b"m", &long).unwrap_err(),
        Error::ContextTooLong
    );
    let sig = sk.sign(b"m", &long[..255], &mut rng).unwrap();
    assert_eq!(verify(&pk, b"m", &long[..255], &sig), Ok(()));
    assert_eq!(verify(&pk, b"m", &long, &sig), Err(Error::ContextTooLong));
}

// Scenario "篡改消息或签名".
#[test]
fn tampering_fails() {
    let sk = key(SigAlg::MlDsa65);
    let pk = sk.public_key().unwrap();
    let msg = b"transfer 10 ATC".to_vec();
    let sig = sk.sign_deterministic(&msg, TX).unwrap();
    for i in [0usize, 7, msg.len() - 1] {
        let mut m = msg.clone();
        m[i] ^= 0x01;
        assert_eq!(verify(&pk, &m, TX, &sig), Err(Error::InvalidSignature));
    }
    for i in [0usize, 100, 3308] {
        let mut raw = sig.as_bytes().to_vec();
        raw[i] ^= 0x80;
        let bad = PqSignature::new(SigAlg::MlDsa65, &raw).unwrap();
        assert_eq!(verify(&pk, &msg, TX, &bad), Err(Error::InvalidSignature));
    }
}

// Requirement "签名模式".
#[test]
fn signing_modes() {
    let mut rng = TestRng::new("modes");
    let sk = key(SigAlg::MlDsa44);
    let pk = sk.public_key().unwrap();
    // Scenario "hedged 签名每次不同".
    let a = sk.sign(b"m", TX, &mut rng).unwrap();
    let b = sk.sign(b"m", TX, &mut rng).unwrap();
    assert_ne!(a, b);
    assert!(verify(&pk, b"m", TX, &a).is_ok() && verify(&pk, b"m", TX, &b).is_ok());
    // Scenario "确定性签名可复现".
    let c = sk.sign_deterministic(b"m", TX).unwrap();
    assert_eq!(c, sk.sign_deterministic(b"m", TX).unwrap());
    assert!(verify(&pk, b"m", TX, &c).is_ok());
}

// Requirement "跨算法混用被拒绝" / Scenario "ML-DSA-44 公钥配 ML-DSA-65 签名".
#[test]
fn algorithm_mismatch() {
    let pk44 = key(SigAlg::MlDsa44).public_key().unwrap();
    let sig65 = key(SigAlg::MlDsa65).sign_deterministic(b"m", TX).unwrap();
    assert_eq!(
        verify(&pk44, b"m", TX, &sig65),
        Err(Error::AlgorithmMismatch)
    );
}

// Requirement "未知与预留算法的安全处理" / Scenario "预留算法".
#[test]
fn reserved_algorithm() {
    assert_eq!(
        SigningKey::from_seed(SigAlg::SlhDsaSha2_128s, &SecretSeed::new([0; 32])).unwrap_err(),
        Error::NotImplemented(0x10)
    );
    let mut enc = vec![0x10u8];
    enc.extend([0u8; 32]);
    assert_eq!(
        PqPublicKey::from_canonical(&enc),
        Err(Error::NotImplemented(0x10))
    );
}

// Requirement "私钥材料的保护" / Scenario "调试输出脱敏".
#[test]
fn debug_output_is_redacted() {
    let seed_bytes = [0xABu8; 32];
    let seed = SecretSeed::new(seed_bytes);
    let sk = SigningKey::from_seed(SigAlg::MlDsa44, &seed).unwrap();
    let dbg = format!("{sk:?} {seed:?}");
    assert!(!dbg.to_lowercase().contains("abab"), "{dbg}");
    assert!(!dbg.contains("171"), "{dbg}");
    assert!(dbg.contains("MlDsa44"));
}
