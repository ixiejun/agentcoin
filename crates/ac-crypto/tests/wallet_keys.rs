//! Requirement "助记词" / Scenario "派生规则固定" (crypto/key-storage), plus the fixed
//! development accounts (node/chain-spec, Scenario "开发密钥可复现").
#![cfg(feature = "mnemonic")]
// Integration-test crate: AGENT.md §5.3 permits unwrap/expect/panics in test code.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

mod common;

use ac_crypto::sig::SigningKey;
use ac_crypto::{
    DEV_SEED_CONTEXT, SigAlg, WALLET_KEY_CONTEXT, account_id, dev_seed, mnemonic, wallet_key_seed,
};
use common::{load, unhex};

#[test]
fn contexts_are_frozen() {
    assert_eq!(WALLET_KEY_CONTEXT, "agentcoin 2026-09 wallet-key v1");
    assert_eq!(DEV_SEED_CONTEXT, "agentcoin 2026-09 dev-seed v1");
}

#[test]
fn wallet_derivation_vectors() {
    let v = load("wallet_keys.json");
    let entropy = mnemonic::from_mnemonic(v["mnemonic"].as_str().unwrap()).unwrap();
    let cases = v["wallet"].as_array().unwrap();
    assert_eq!(cases.len(), 3);
    for c in cases {
        let alg = SigAlg::from_id(u8::try_from(c["alg_id"].as_u64().unwrap()).unwrap()).unwrap();
        let index = u32::try_from(c["index"].as_u64().unwrap()).unwrap();
        let key =
            SigningKey::from_seed(alg, &wallet_key_seed(&entropy, alg, index).unwrap()).unwrap();
        assert_eq!(
            account_id(&key.public_key().unwrap()).as_bytes().to_vec(),
            unhex(c["account_id"].as_str().unwrap())
        );
    }
}

#[test]
fn dev_account_vectors() {
    let v = load("wallet_keys.json");
    let cases = v["dev"].as_array().unwrap();
    assert_eq!(cases.len(), 8);
    for c in cases {
        let alg = SigAlg::from_id(u8::try_from(c["alg_id"].as_u64().unwrap()).unwrap()).unwrap();
        let seed = dev_seed(c["name"].as_str().unwrap()).unwrap();
        let key = SigningKey::from_seed(alg, &seed).unwrap();
        assert_eq!(
            account_id(&key.public_key().unwrap()).as_bytes().to_vec(),
            unhex(c["account_id"].as_str().unwrap())
        );
    }
}
