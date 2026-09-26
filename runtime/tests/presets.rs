//! Genesis presets (node/chain-spec, consensus/aura-pq), task 5.5.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::too_many_arguments
)]

mod common;

use ac_crypto::SigAlg;
use ac_runtime::genesis_config_presets::{
    DEV_ACCOUNTS, DEV_ENDOWMENT, dev_account, dev_public_key, preset_names,
};
use common::{issuance, preset_ext, sum_of_balances};

// Requirement "正式链创世零发行" / Scenario "内置非开发预设为零发行".
#[test]
fn only_development_presets_have_issuance() {
    let names = preset_names();
    assert!(!names.is_empty());
    for name in names {
        let id: &str = name.as_ref();
        preset_ext(id).execute_with(|| {
            let is_dev = id == sp_genesis_builder::DEV_RUNTIME_PRESET
                || id == sp_genesis_builder::LOCAL_TESTNET_RUNTIME_PRESET;
            assert_eq!(issuance() > 0, is_dev, "preset {id}");
            assert_eq!(sum_of_balances(), issuance());
        });
    }
}

#[test]
fn development_accounts_are_endowed() {
    preset_ext(sp_genesis_builder::DEV_RUNTIME_PRESET).execute_with(|| {
        for name in DEV_ACCOUNTS {
            assert_eq!(common::free(&dev_account(name).unwrap()), DEV_ENDOWMENT);
        }
        assert_eq!(issuance(), DEV_ENDOWMENT * 4);
    });
}

// consensus/aura-pq Requirement "授权节点集合来自创世" / Scenario "查询授权节点".
#[test]
fn local_testnet_has_three_ordered_authorities() {
    preset_ext(sp_genesis_builder::LOCAL_TESTNET_RUNTIME_PRESET).execute_with(|| {
        let expected: Vec<_> = ["alice", "bob", "charlie"]
            .iter()
            .map(|n| dev_public_key(n, SigAlg::MlDsa65).unwrap())
            .collect();
        assert_eq!(ac_runtime::AuraPq::authorities(), expected);
    });
    preset_ext(sp_genesis_builder::DEV_RUNTIME_PRESET).execute_with(|| {
        assert_eq!(ac_runtime::AuraPq::authorities().len(), 1);
    });
}

// Unknown presets do not exist.
#[test]
fn unknown_preset_is_none() {
    assert!(ac_runtime::genesis_config_presets::get_preset(&"mainnet".into()).is_none());
}
