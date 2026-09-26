//! Behavior of the X-Wing hybrid KEM (crypto/hybrid-kem).
#![cfg(all(feature = "kem", feature = "rand"))]
// Integration-test crate: AGENT.md §5.3 permits unwrap/expect/panics in test code.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable,
    clippy::indexing_slicing
)]

mod common;

use ac_crypto::kem::{KemSecretKey, encapsulate};
use ac_crypto::sig::SecretSeed;
use ac_crypto::{Error, KemAlg, KemCiphertext};
use common::TestRng;
use subtle::ConstantTimeEq;

fn key(seed: u8) -> KemSecretKey {
    KemSecretKey::from_seed(KemAlg::XWing, &SecretSeed::new([seed; 32])).unwrap()
}

// Scenario "封装后解封装得到相同共享秘密".
#[test]
fn encapsulate_then_decapsulate() {
    let mut rng = TestRng::new("kem");
    let sk = key(1);
    let (ct, ss) = encapsulate(&sk.public_key().unwrap(), &mut rng).unwrap();
    assert_eq!(ct.as_bytes().len(), 1120);
    let ss2 = sk.decapsulate(&ct).unwrap();
    assert!(bool::from(ss.ct_eq(&ss2)));
}

// Scenario "相同种子得到相同密钥".
#[test]
fn same_seed_same_public_key() {
    assert_eq!(key(7).public_key().unwrap(), key(7).public_key().unwrap());
    assert_ne!(key(7).public_key().unwrap(), key(8).public_key().unwrap());
}

// Requirement "错误密文不泄露信息" / Scenario "篡改密文".
#[test]
fn tampered_ciphertext_implicitly_rejected() {
    let mut rng = TestRng::new("tamper");
    let sk = key(2);
    let (ct, ss) = encapsulate(&sk.public_key().unwrap(), &mut rng).unwrap();
    for i in [0usize, 500, 1087, 1100] {
        let mut raw = ct.as_bytes().to_vec();
        raw[i] ^= 0x01;
        let bad = KemCiphertext::new(KemAlg::XWing, &raw).unwrap();
        let got = sk
            .decapsulate(&bad)
            .expect("implicit rejection must not error");
        assert!(!bool::from(got.ct_eq(&ss)), "byte {i}");
    }
    // A ciphertext for another key also decapsulates without error, to a different secret.
    let (ct_other, _) = encapsulate(&key(3).public_key().unwrap(), &mut rng).unwrap();
    assert!(!bool::from(sk.decapsulate(&ct_other).unwrap().ct_eq(&ss)));
}

// Requirement "带标签的 KEM 对象" / Scenario "公钥带标签".
#[test]
fn public_key_is_tagged() {
    let enc = key(4).public_key().unwrap().to_canonical();
    assert_eq!(&enc[..2], &[0x01, 0x11]);
    assert_eq!(enc.len(), 2 + 1216);
}

#[test]
fn reserved_kem_not_implemented() {
    assert_eq!(
        KemSecretKey::from_seed(KemAlg::MlKem1024, &SecretSeed::new([0; 32])).unwrap_err(),
        Error::NotImplemented(0x1102)
    );
}

#[test]
fn secrets_are_redacted() {
    let mut rng = TestRng::new("redact");
    let sk = key(0xAB);
    let (_, ss) = encapsulate(&sk.public_key().unwrap(), &mut rng).unwrap();
    let dbg = format!("{sk:?} {ss:?}");
    assert!(dbg.contains("redacted") && dbg.contains("XWing"), "{dbg}");
    assert!(!dbg.contains("171"));
}
