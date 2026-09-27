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
        let wallet = ac_runtime::genesis_config_presets::dev_wallet_account().unwrap();
        assert_eq!(common::free(&wallet), DEV_ENDOWMENT);
        // The development wallet is the account of the repository's fixed test mnemonic.
        let expected =
            hex::decode("5c1a1a670210dad33c958655ff5297016b8092e726b55a884bce4acffad3cdf5")
                .unwrap();
        assert_eq!(
            <sp_runtime::AccountId32 as AsRef<[u8]>>::as_ref(&wallet),
            &expected[..]
        );
        assert_eq!(issuance(), DEV_ENDOWMENT * 5);
    });
}

// consensus/aura-pq Scenario "查询授权节点", consensus/validator-set Scenario "查询集合" and
// node/chain-spec Scenario "本地链有四个授权节点" (runtime part).
#[test]
fn local_testnet_has_four_ordered_authorities() {
    preset_ext(sp_genesis_builder::LOCAL_TESTNET_RUNTIME_PRESET).execute_with(|| {
        let expected: Vec<_> = ["alice", "bob", "charlie", "dave"]
            .iter()
            .map(|n| dev_public_key(n, SigAlg::MlDsa65).unwrap())
            .collect();
        assert_eq!(ac_runtime::AuraPq::authorities(), expected);
        let (set_id, set) = ac_runtime::ValidatorSet::authority_set();
        assert_eq!(set_id, 0);
        assert_eq!(
            set,
            expected
                .into_iter()
                .map(ac_primitives::ac_bft::Authority::poa)
                .collect::<Vec<_>>()
        );
        assert_eq!(ac_runtime::ValidatorSet::epoch_length(), 20);
    });
    preset_ext(sp_genesis_builder::DEV_RUNTIME_PRESET).execute_with(|| {
        assert_eq!(ac_runtime::AuraPq::authorities().len(), 1);
        assert_eq!(ac_runtime::ValidatorSet::epoch_length(), 10);
    });
}

// Unknown presets do not exist.
#[test]
fn unknown_preset_is_none() {
    assert!(ac_runtime::genesis_config_presets::get_preset(&"mainnet".into()).is_none());
}

// node/chain-spec Scenario "开发链的经济参数" (runtime part): emission epochs of at most 20
// blocks dividing the four-year period, the PoA administration (dev: alice / 1; local: alice,
// bob, charlie / 2) and the 40% community share.
#[test]
fn development_economics() {
    let check = |preset: &str, length: u64, admins: &[&str], threshold: u32| {
        preset_ext(preset).execute_with(|| {
            let l = pallet_emission::EpochLength::<ac_runtime::Runtime>::get().unwrap();
            assert_eq!(l, length);
            assert!(l <= 20 && ac_primitives::emission::BLOCKS_PER_PERIOD.is_multiple_of(l));
            let mut expected: Vec<_> = admins.iter().map(|n| dev_account(n).unwrap()).collect();
            expected.sort();
            assert_eq!(
                pallet_collective::Members::<ac_runtime::Runtime, pallet_collective::Instance1>::get(),
                expected
            );
            assert_eq!(
                pallet_poa_admin::Threshold::<ac_runtime::Runtime>::get(),
                threshold
            );
            assert_eq!(
                pallet_treasury_dual::CommunityShare::<ac_runtime::Runtime>::get(),
                4_000
            );
            assert_eq!(ac_runtime::Emission::total_burned(), 0);
        });
    };
    check(sp_genesis_builder::DEV_RUNTIME_PRESET, 10, &["alice"], 1);
    check(
        sp_genesis_builder::LOCAL_TESTNET_RUNTIME_PRESET,
        20,
        &["alice", "bob", "charlie"],
        2,
    );
}
