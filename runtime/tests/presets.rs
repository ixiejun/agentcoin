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

// m3-pos 8.2, node/chain-spec Requirement "质押与切换参数": the development and local presets use
// small values so that a switch completes within minutes; `K` fits half an epoch; the default
// (live) genesis values are the constitution's.
#[test]
fn staking_and_switch_parameters() {
    use ac_primitives::staking::TransitionParams;
    use ac_runtime::Runtime;
    use ac_runtime::genesis_config_presets::{
        DEV_STAKING, DEV_TRANSITION, LOCAL_STAKING, LOCAL_TRANSITION,
    };

    for (id, transition, staking, k, epoch) in [
        (
            sp_genesis_builder::DEV_RUNTIME_PRESET,
            DEV_TRANSITION,
            DEV_STAKING,
            5u32,
            10u64,
        ),
        (
            sp_genesis_builder::LOCAL_TESTNET_RUNTIME_PRESET,
            LOCAL_TRANSITION,
            LOCAL_STAKING,
            10,
            20,
        ),
    ] {
        preset_ext(id).execute_with(|| {
            assert_eq!(
                pallet_validator_set::TransitionParams::<Runtime>::get(),
                transition
            );
            assert_eq!(pallet_validator_set::ValidatorCount::<Runtime>::get(), k);
            assert_eq!(pallet_validator_set::EpochLength::<Runtime>::get(), epoch);
            assert!(u64::from(k) * 2 <= epoch, "K fits half an epoch");
            assert_eq!(pallet_staking_pos::Params::<Runtime>::get(), staking);
            // Minutes, not days: at most a hundred one-second blocks each.
            assert!(transition.min_height + transition.sustain_blocks <= 100);
            assert!(staking.self_unbond_blocks <= 100 && staking.commission_delay <= 100);
            assert!(transition.min_candidates <= 3);
        });
    }
    // Scenario "导出正式链参数": the defaults a live chain spec starts from.
    let live = pallet_validator_set::GenesisConfig::<Runtime>::default();
    assert_eq!(live.transition, TransitionParams::CONSTITUTION);
    assert_eq!(live.transition.stake_bps, 1_000);
    assert_eq!(live.transition.min_candidates, 21);
    assert_eq!(live.transition.min_height, 63_115_200);
    assert_eq!(live.transition.sustain_blocks, 604_800);
    assert_eq!(live.validator_count, 100);
    let staking = pallet_staking_pos::StakingParams::default();
    assert_eq!(staking, pallet_staking_pos::StakingParams::LIVE);
    assert_eq!((staking.max_candidates, staking.max_nominators), (500, 750));
    assert_eq!(staking.self_unbond_blocks, 2_419_200);
    assert_eq!(
        (staking.nomination_unbond_min, staking.nomination_unbond_max),
        (172_800, 2_419_200)
    );
    assert_eq!(staking.commission_delay, 604_800);
}

// m6-audit-chain, spec node/chain-spec "审计参数": test presets use short rounds and three
// reviewers deciding by two; a live chain spec starts from the draft live values.
#[test]
fn audit_parameters() {
    use ac_primitives::market::audit::AuditParams;
    use ac_runtime::Runtime;
    use sp_runtime::Perbill;

    for id in [
        sp_genesis_builder::DEV_RUNTIME_PRESET,
        sp_genesis_builder::LOCAL_TESTNET_RUNTIME_PRESET,
    ] {
        preset_ext(id).execute_with(|| {
            let p = pallet_audit::Params::<Runtime>::get().unwrap();
            assert!(p.round_blocks <= 50 && p.vote_blocks <= 50 && p.unbond_blocks <= 100);
            assert_eq!((p.reviewers, p.quorum, p.assign), (3, 2, 2));
            assert_eq!(p.check(), Ok(()));
        });
    }
    // Scenario "导出正式链审计参数".
    let live = pallet_audit::GenesisConfig::<Runtime>::default().params;
    let (fixed, adjustable) = live.split();
    assert_eq!(fixed, AuditParams::LIVE);
    assert_eq!(fixed.round_blocks, 1_800);
    assert_eq!((fixed.reviewers, fixed.quorum), (5, 3));
    assert_eq!(fixed.provider_slash, Perbill::from_percent(10));
    assert_eq!(fixed.auditor_slash, Perbill::from_percent(10));
    assert_eq!(adjustable.stake_usd.0, 1_000_000_000);
    assert_eq!(adjustable.payment_usd.0, 50_000);
    // The accepted thresholds version is the one the auditors' software publishes.
    assert_eq!(
        adjustable.thresholds_version,
        ac_market_proto::AUDIT_THRESHOLDS.version
    );
}
