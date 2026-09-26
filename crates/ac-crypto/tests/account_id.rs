//! Requirement "账户 ID 派生" / Scenario "固定向量" (crypto/hashing).
// Integration-test crate: AGENT.md §5.3 permits unwrap/expect/panics in test code.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable,
    clippy::indexing_slicing
)]

mod common;

use ac_crypto::sig::{SecretSeed, SigningKey};
use ac_crypto::{ACCOUNT_ID_CONTEXT, PqPublicKey, SigAlg, account_id};
use common::{load, unhex};

#[test]
fn context_is_frozen() {
    assert_eq!(ACCOUNT_ID_CONTEXT, "agentcoin 2026-09 account-id v1");
}

#[test]
fn fixed_vectors() {
    let cases = load("account_id.json");
    let cases = cases.as_array().unwrap();
    let ids: Vec<u64> = cases
        .iter()
        .map(|c| c["alg_id"].as_u64().unwrap())
        .collect();
    assert_eq!(ids, [0x0101, 0x0102]);
    for c in cases {
        let alg = SigAlg::from_id(u16::try_from(c["alg_id"].as_u64().unwrap()).unwrap()).unwrap();
        let seed: [u8; 32] = unhex(c["seed"].as_str().unwrap()).try_into().unwrap();
        let pk = SigningKey::from_seed(alg, &SecretSeed::new(seed))
            .unwrap()
            .public_key()
            .unwrap();
        assert_eq!(pk.as_bytes(), unhex(c["public_key"].as_str().unwrap()));
        let from_file = PqPublicKey::new(alg, &unhex(c["public_key"].as_str().unwrap())).unwrap();
        let expected = unhex(c["account_id"].as_str().unwrap());
        assert_eq!(account_id(&pk).as_bytes().as_slice(), expected);
        assert_eq!(account_id(&from_file).as_bytes().as_slice(), expected);
    }
}

#[test]
fn same_key_same_account() {
    let pk = SigningKey::from_seed(SigAlg::MlDsa44, &SecretSeed::new([5; 32]))
        .unwrap()
        .public_key()
        .unwrap();
    assert_eq!(account_id(&pk), account_id(&pk.clone()));
}
