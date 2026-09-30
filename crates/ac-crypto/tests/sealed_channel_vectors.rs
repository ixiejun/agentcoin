//! Requirement "可复现的测试向量" (crypto/sealed-channel): the RFC 8439 §2.8.2 ChaCha20-Poly1305
//! vector and the repository's sealed-channel regression vector.
#![cfg(all(feature = "sealed", feature = "deterministic"))]
// Integration-test crate: AGENT.md §5.3 permits unwrap/expect/panics in test code.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

mod common;

use ac_crypto::kem::KemSecretKey;
use ac_crypto::sealed::{
    Acceptor, ReplayCache, SignedHandshake, accept_session, open_session_derandomized, recipient_id,
};
use ac_crypto::sig::{SecretSeed, SigningKey};
use ac_crypto::{KemAlg, SigAlg, account_id};
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use common::{load, unhex};

fn arr<const N: usize>(hex: &serde_json::Value) -> [u8; N] {
    unhex(hex.as_str().unwrap()).try_into().unwrap()
}

// Scenario "AEAD 官方向量".
#[test]
fn chacha20poly1305_rfc8439() {
    let v = load("chacha20poly1305_rfc8439.json");
    let cipher = ChaCha20Poly1305::new(&Key::from(arr::<32>(&v["key"])));
    let nonce = Nonce::from(arr::<12>(&v["nonce"]));
    let (aad, plain) = (
        unhex(v["aad"].as_str().unwrap()),
        unhex(v["plaintext"].as_str().unwrap()),
    );
    let mut expected = unhex(v["ciphertext"].as_str().unwrap());
    expected.extend(unhex(v["tag"].as_str().unwrap()));
    let sealed = cipher
        .encrypt(
            &nonce,
            Payload {
                msg: &plain,
                aad: &aad,
            },
        )
        .unwrap();
    assert_eq!(sealed, expected);
    let opened = cipher
        .decrypt(
            &nonce,
            Payload {
                msg: &sealed,
                aad: &aad,
            },
        )
        .unwrap();
    assert_eq!(opened, plain);
    // A changed tag is rejected.
    let mut bad = sealed;
    *bad.last_mut().unwrap() ^= 1;
    assert!(
        cipher
            .decrypt(
                &nonce,
                Payload {
                    msg: &bad,
                    aad: &aad
                }
            )
            .is_err()
    );
}

// Scenario "通道回归向量": the handshake, both keys and every chunk are byte-for-byte fixed.
#[test]
fn sealed_channel_regression() {
    let v = load("sealed_channel.json");
    let kem =
        KemSecretKey::from_seed(KemAlg::XWing, &SecretSeed::new(arr(&v["kem_seed"]))).unwrap();
    let recipient = kem.public_key().unwrap();
    assert_eq!(
        v["signing_alg_id"].as_u64().unwrap(),
        u64::from(SigAlg::MlDsa44.id())
    );
    let signer =
        SigningKey::from_seed(SigAlg::MlDsa44, &SecretSeed::new(arr(&v["signing_seed"]))).unwrap();
    let created = v["created"].as_u64().unwrap();
    let (hs, mut tx) = open_session_derandomized(
        &recipient,
        &signer,
        (account_id(&signer.public_key().unwrap()), created),
        arr(&v["nonce"]),
        &arr(&v["kem_randomness"]),
    )
    .unwrap();
    assert_eq!(
        recipient_id(&recipient).unwrap(),
        arr::<32>(&v["recipient_id"])
    );
    let wire = unhex(v["handshake"].as_str().unwrap());
    assert_eq!(hs.encode().unwrap(), wire);
    assert_eq!(SignedHandshake::decode(&wire).unwrap(), hs);
    let (k_req, k_resp) = tx.vector_keys();
    assert_eq!(k_req, arr::<32>(&v["request_key"]));
    assert_eq!(k_resp, arr::<32>(&v["response_key"]));

    let key = signer.public_key().unwrap();
    let mut rx = accept_session(
        &hs,
        &Acceptor {
            secret: &kem,
            recipient: recipient_id(&recipient).unwrap(),
            now: created,
        },
        |_| Some(key),
        &mut ReplayCache::default(),
    )
    .unwrap();
    for c in v["request"].as_array().unwrap() {
        let (plain, last) = (
            unhex(c["plaintext"].as_str().unwrap()),
            c["last"].as_bool().unwrap(),
        );
        let chunk = tx.request.seal(&plain, last).unwrap();
        assert_eq!(chunk, unhex(c["chunk"].as_str().unwrap()));
        assert_eq!(rx.request.open(&chunk).unwrap(), (plain, last));
    }
    rx.request.finish().unwrap();
    for c in v["response"].as_array().unwrap() {
        let (plain, last) = (
            unhex(c["plaintext"].as_str().unwrap()),
            c["last"].as_bool().unwrap(),
        );
        let chunk = rx.response.seal(&plain, last).unwrap();
        assert_eq!(chunk, unhex(c["chunk"].as_str().unwrap()));
        assert_eq!(tx.response.open(&chunk).unwrap(), (plain, last));
    }
    tx.response.finish().unwrap();
}
