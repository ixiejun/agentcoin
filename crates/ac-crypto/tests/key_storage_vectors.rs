//! Requirement "官方测试向量" (crypto/key-storage): Argon2id (RFC 9106 §5.3),
//! XChaCha20-Poly1305 (draft-irtf-cfrg-xchacha-03 A.3.1) and BIP-39 (English, 256-bit entropy).
#![cfg(all(feature = "keystore", feature = "mnemonic"))]
// Integration-test crate: AGENT.md §5.3 permits unwrap/expect/panics in test code.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

mod common;

use ac_crypto::mnemonic::{from_mnemonic, to_mnemonic};
use ac_crypto::{Error, MnemonicError, WalletEntropy};
use argon2::{Algorithm, Argon2, AssociatedData, ParamsBuilder, Version};
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{Key, XChaCha20Poly1305, XNonce};
use common::{load, unhex};

#[test]
fn argon2id_rfc9106() {
    let v = load("argon2id_rfc9106.json");
    let ad = unhex(v["associated_data"].as_str().unwrap());
    let params = ParamsBuilder::new()
        .m_cost(u32::try_from(v["m_kib"].as_u64().unwrap()).unwrap())
        .t_cost(u32::try_from(v["t"].as_u64().unwrap()).unwrap())
        .p_cost(u32::try_from(v["p"].as_u64().unwrap()).unwrap())
        .data(AssociatedData::new(&ad).unwrap())
        .output_len(32)
        .build()
        .unwrap();
    let secret = unhex(v["secret"].as_str().unwrap());
    let argon =
        Argon2::new_with_secret(&secret, Algorithm::Argon2id, Version::V0x13, params).unwrap();
    let mut out = [0u8; 32];
    argon
        .hash_password_into(
            &unhex(v["password"].as_str().unwrap()),
            &unhex(v["salt"].as_str().unwrap()),
            &mut out,
        )
        .unwrap();
    assert_eq!(out.to_vec(), unhex(v["tag"].as_str().unwrap()));
}

#[test]
fn xchacha20poly1305_draft03() {
    let v = load("xchacha20poly1305_draft03.json");
    let key: [u8; 32] = unhex(v["key"].as_str().unwrap()).try_into().unwrap();
    let nonce: [u8; 24] = unhex(v["iv"].as_str().unwrap()).try_into().unwrap();
    let aad = unhex(v["aad"].as_str().unwrap());
    let plaintext = unhex(v["plaintext"].as_str().unwrap());
    let cipher = XChaCha20Poly1305::new(&Key::from(key));
    let sealed = cipher
        .encrypt(
            &XNonce::from(nonce),
            Payload {
                msg: &plaintext,
                aad: &aad,
            },
        )
        .unwrap();
    let mut expected = unhex(v["ciphertext"].as_str().unwrap());
    expected.extend(unhex(v["tag"].as_str().unwrap()));
    assert_eq!(sealed, expected);
    let opened = cipher
        .decrypt(
            &XNonce::from(nonce),
            Payload {
                msg: &sealed,
                aad: &aad,
            },
        )
        .unwrap();
    assert_eq!(opened, plaintext);
}

#[test]
fn bip39_english_256_bit() {
    let cases = load("bip39_english_256.json");
    let cases = cases.as_array().unwrap();
    assert_eq!(cases.len(), 8);
    for c in cases {
        let entropy: [u8; 32] = unhex(c["entropy"].as_str().unwrap()).try_into().unwrap();
        let phrase = c["mnemonic"].as_str().unwrap();
        let encoded = to_mnemonic(&WalletEntropy::new(entropy)).unwrap();
        assert_eq!(encoded.as_str(), phrase);
        assert_eq!(from_mnemonic(phrase).unwrap().expose(), &entropy);
    }
}

// Scenario "助记词往返".
#[test]
fn mnemonic_round_trip() {
    let mut rng = common::TestRng::new("mnemonic round trip");
    for _ in 0..16 {
        let entropy = WalletEntropy::generate(&mut rng);
        let phrase = to_mnemonic(&entropy).unwrap();
        assert_eq!(phrase.split(' ').count(), 24);
        assert_eq!(from_mnemonic(&phrase).unwrap().expose(), entropy.expose());
    }
}

// Scenario "校验和错误".
#[test]
fn mnemonic_checksum_and_word_errors() {
    let cases = load("bip39_english_256.json");
    let phrase = cases[1]["mnemonic"].as_str().unwrap();
    let mut words: Vec<&str> = phrase.split(' ').collect();
    // Replacing the first word keeps the word count and the word list but breaks the checksum
    // for this fixed vector (checked below).
    let original = words[0];
    words[0] = if original == "abandon" {
        "ability"
    } else {
        "abandon"
    };
    assert_eq!(
        from_mnemonic(&words.join(" ")).unwrap_err(),
        Error::InvalidMnemonic(MnemonicError::Checksum)
    );
    words[0] = "notaword";
    assert_eq!(
        from_mnemonic(&words.join(" ")).unwrap_err(),
        Error::InvalidMnemonic(MnemonicError::UnknownWord)
    );
    assert_eq!(
        from_mnemonic(&words[..12].join(" ")).unwrap_err(),
        Error::InvalidMnemonic(MnemonicError::WordCount)
    );
}
