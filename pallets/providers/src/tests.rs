//! Unit tests; each names the spec `market/providers` scenario it covers.
// Test code: an overflow here fails the test loudly (debug builds panic), which is what we want.
#![allow(clippy::arithmetic_side_effects)]

use frame_support::dispatch::Pays;
use frame_support::traits::fungible::{Inspect, InspectHold};
use frame_support::{BoundedVec, assert_noop, assert_ok};
use proptest::prelude::*;
use sp_runtime::Perbill;

use ac_crypto::{KemAlg, KemPublicKey};
use ac_primitives::market::records::{AttestationRef, ProviderStatus, Tier, unlocking_total};
use ac_primitives::market::traits::ProviderPenalty;
use parity_scale_codec::{Decode, Encode};

use crate::mock::{
    ALICE, BOB, Balances, FUNDS, HEARTBEAT, MODEL_A, MODEL_B, Providers, RuntimeCall,
    RuntimeHoldReason, RuntimeOrigin, System, T1_MIN, T2_MIN, Test, UNBOND, UNKNOWN, burned, ext,
    issuance_and_burned, kem, price, register, registration, set_rate,
};
use crate::{Error, Event, HoldReason, ModelProviders};

fn held(who: u64) -> u128 {
    Balances::balance_on_hold(&RuntimeHoldReason::Providers(HoldReason::Stake), &who)
}

fn at(block: u64) {
    System::set_block_number(block);
}

fn serviceable_of(model: ac_primitives::market::ModelId) -> Vec<u64> {
    Providers::serviceable_providers(model, None, 256)
        .into_iter()
        .map(|(who, _)| who)
        .collect()
}

// Requirement "提供者登记", Scenario "正常登记".
#[test]
fn a_provider_registers() {
    ext().execute_with(|| {
        let issuance = issuance_and_burned();
        assert_ok!(register(ALICE, &[price(MODEL_A, 100, 200)], T2_MIN));
        let p = Providers::provider(&ALICE).unwrap();
        assert_eq!(p.tier, Tier::T2);
        assert_eq!(p.kem_pk, kem());
        assert_eq!(p.models.to_vec(), vec![price(MODEL_A, 100, 200)]);
        assert_eq!(p.stake, T2_MIN);
        assert_eq!(p.status, ProviderStatus::Active);
        assert_eq!(p.last_heartbeat, 1);
        assert_eq!(held(ALICE), T2_MIN);
        assert!(Providers::is_serviceable(&ALICE));
        assert_eq!(serviceable_of(MODEL_A), vec![ALICE]);
        assert_eq!(issuance_and_burned(), issuance);
        System::assert_last_event(
            Event::Registered {
                who: ALICE,
                tier: Tier::T2,
                stake: T2_MIN,
            }
            .into(),
        );
        assert_noop!(
            register(ALICE, &[price(MODEL_A, 1, 1)], T2_MIN),
            Error::<Test>::AlreadyRegistered
        );
    });
}

// Scenario "未登记的模型", plus the other model-list rules.
#[test]
fn the_model_list_is_checked() {
    ext().execute_with(|| {
        assert_noop!(
            register(ALICE, &[price(UNKNOWN, 1, 1)], T2_MIN),
            Error::<Test>::UnknownModel
        );
        assert_noop!(register(ALICE, &[], T2_MIN), Error::<Test>::NoModels);
        assert_noop!(
            register(ALICE, &[price(MODEL_A, 1, 1), price(MODEL_A, 2, 2)], T2_MIN),
            Error::<Test>::DuplicateModel
        );
        assert_noop!(
            register(ALICE, &[price(MODEL_A, 0, 1)], T2_MIN),
            Error::<Test>::ZeroPrice
        );
    });
}

// Scenario "非 X-Wing 公钥": other KEM AlgIds and wrong lengths do not even decode as a call
// argument, so such a transaction is invalid.
#[test]
fn only_x_wing_keys_are_accepted() {
    assert!(KemPublicKey::new(KemAlg::MlKem1024, &[0; 1568]).is_err());
    assert!(KemPublicKey::new(KemAlg::XWing, &[0; 1215]).is_err());
    let mut reg = registration(Tier::T2, &[price(MODEL_A, 1, 1)], T2_MIN);
    reg.endpoint = BoundedVec::truncate_from(b"x".to_vec());
    let call = RuntimeCall::Providers(crate::Call::register { registration: reg });
    let mut bytes = call.encode();
    // The KEM key follows the pallet index, call index, tier and endpoint (compact length 1 + 1).
    let alg_at = 1 + 1 + 1 + 1 + 1;
    assert_eq!(bytes[alg_at], KemAlg::XWing as u8);
    bytes[alg_at] = 0x02; // ML-KEM-1024
    assert!(RuntimeCall::decode(&mut &bytes[..]).is_err());
}

// Scenario "T0 预留", and attestations are refused.
#[test]
fn t0_and_attestations_are_reserved() {
    ext().execute_with(|| {
        let t0 = registration(Tier::T0, &[price(MODEL_A, 1, 1)], T1_MIN);
        assert_noop!(
            Providers::register(RuntimeOrigin::signed(ALICE), t0),
            Error::<Test>::TierNotEnabled
        );
        let mut attested = registration(Tier::T1, &[price(MODEL_A, 1, 1)], T1_MIN);
        attested.attestation = Some(AttestationRef([1; 32]));
        assert_noop!(
            Providers::register(RuntimeOrigin::signed(ALICE), attested),
            Error::<Test>::AttestationNotEnabled
        );
    });
}

#[test]
fn the_endpoint_must_be_utf8() {
    ext().execute_with(|| {
        let mut reg = registration(Tier::T2, &[price(MODEL_A, 1, 1)], T2_MIN);
        reg.endpoint = BoundedVec::truncate_from(vec![0xff]);
        assert_noop!(
            Providers::register(RuntimeOrigin::signed(ALICE), reg),
            Error::<Test>::InvalidEndpoint
        );
    });
}

// Requirement "修改登记信息", Scenario "更新价格".
#[test]
fn a_provider_updates_its_prices_and_models() {
    ext().execute_with(|| {
        assert_ok!(register(
            ALICE,
            &[price(MODEL_A, 1_000_000, 2_000_000)],
            T2_MIN
        ));
        let list = BoundedVec::truncate_from(vec![price(MODEL_B, 1_000_000, 1_500_000)]);
        assert_ok!(Providers::update(
            RuntimeOrigin::signed(ALICE),
            None,
            None,
            Some(list)
        ));
        let p = Providers::provider(&ALICE).unwrap();
        assert_eq!(
            p.models.to_vec(),
            vec![price(MODEL_B, 1_000_000, 1_500_000)]
        );
        assert!(serviceable_of(MODEL_A).is_empty());
        assert_eq!(serviceable_of(MODEL_B), vec![ALICE]);
        let bad = BoundedVec::truncate_from(vec![price(UNKNOWN, 1, 1)]);
        assert_noop!(
            Providers::update(RuntimeOrigin::signed(ALICE), None, None, Some(bad)),
            Error::<Test>::UnknownModel
        );
        assert_noop!(
            Providers::update(RuntimeOrigin::signed(BOB), None, None, None),
            Error::<Test>::NotProvider
        );
    });
}

// Requirement "质押门槛", Scenario "质押不足" and "未设置时换算失败" (spec economics/ref-rate).
#[test]
fn the_threshold_is_in_dollars() {
    ext().execute_with(|| {
        assert_noop!(
            register(ALICE, &[price(MODEL_A, 1, 1)], T2_MIN - 1),
            Error::<Test>::BelowThreshold
        );
        set_rate(None);
        assert_noop!(
            register(ALICE, &[price(MODEL_A, 1, 1)], T2_MIN),
            Error::<Test>::RateNotSet
        );
        set_rate(Some(3));
        // $100 at 3 units per dollar = 300 units.
        assert_ok!(register(ALICE, &[price(MODEL_A, 1, 1)], 300));
    });
}

// Scenario "汇率变化使质押不足".
#[test]
fn a_rate_change_can_make_a_provider_unserviceable() {
    ext().execute_with(|| {
        assert_ok!(register(ALICE, &[price(MODEL_A, 1, 1)], T2_MIN));
        assert!(Providers::is_serviceable(&ALICE));
        set_rate(Some(1_100)); // ATC depreciates: $100 is now 110,000 units.
        assert!(!Providers::is_serviceable(&ALICE));
        assert!(serviceable_of(MODEL_A).is_empty());
        assert_ok!(Providers::bond_extra(RuntimeOrigin::signed(ALICE), 10_000));
        assert!(Providers::is_serviceable(&ALICE));
        set_rate(None);
        assert!(!Providers::is_serviceable(&ALICE));
    });
}

// Requirement "心跳与可服务判定", Scenario "心跳超时".
#[test]
fn a_silent_provider_becomes_unserviceable() {
    ext().execute_with(|| {
        assert_ok!(register(ALICE, &[price(MODEL_A, 1, 1)], T2_MIN));
        at(1 + 2 * HEARTBEAT);
        assert_eq!(serviceable_of(MODEL_A), vec![ALICE]);
        at(2 + 2 * HEARTBEAT);
        assert!(serviceable_of(MODEL_A).is_empty());
        assert_ok!(Providers::heartbeat(RuntimeOrigin::signed(ALICE)));
        assert_eq!(serviceable_of(MODEL_A), vec![ALICE]);
    });
}

// Scenarios "按时心跳免费" and "过于频繁的心跳收费".
#[test]
fn heartbeats_are_free_only_when_on_time() {
    ext().execute_with(|| {
        assert_ok!(register(ALICE, &[price(MODEL_A, 1, 1)], T2_MIN));
        at(1 + HEARTBEAT);
        let info = Providers::heartbeat(RuntimeOrigin::signed(ALICE)).unwrap();
        assert_eq!(info.pays_fee, Pays::No);
        at(1 + HEARTBEAT + HEARTBEAT / 2 - 1);
        let info = Providers::heartbeat(RuntimeOrigin::signed(ALICE)).unwrap();
        assert_eq!(info.pays_fee, Pays::Yes);
        assert_eq!(
            Providers::provider(&ALICE).unwrap().last_heartbeat,
            1 + HEARTBEAT + HEARTBEAT / 2 - 1
        );
        assert_noop!(
            Providers::heartbeat(RuntimeOrigin::signed(BOB)),
            Error::<Test>::NotProvider
        );
    });
}

#[test]
fn serviceable_providers_page_in_account_order() {
    ext().execute_with(|| {
        for who in [5u64, 3, 4] {
            let _ = Balances::mint_into_for_tests(who);
            assert_ok!(register(who, &[price(MODEL_A, 1, 1)], T2_MIN));
        }
        assert_eq!(serviceable_of(MODEL_A), vec![3, 4, 5]);
        let page = Providers::serviceable_providers(MODEL_A, Some(3), 1);
        assert_eq!(
            page.into_iter().map(|(w, _)| w).collect::<Vec<_>>(),
            vec![4]
        );
        let rest = Providers::serviceable_providers(MODEL_A, Some(4), 10);
        assert_eq!(
            rest.into_iter().map(|(w, _)| w).collect::<Vec<_>>(),
            vec![5]
        );
    });
}

#[test]
fn a_page_holds_at_most_256_providers() {
    ext().execute_with(|| {
        for who in 100u64..(100 + 257) {
            let _ = Balances::mint_into_for_tests(who);
            assert_ok!(register(who, &[price(MODEL_A, 1, 1)], T2_MIN));
        }
        let first = Providers::serviceable_providers(MODEL_A, None, u32::MAX);
        assert_eq!(first.len(), 256);
        // Order is the accounts' encoded byte order; the next page starts after the last entry.
        let last = first.last().map(|(w, _)| *w);
        let second = Providers::serviceable_providers(MODEL_A, last, u32::MAX);
        assert_eq!(second.len(), 1);
        assert!(!first.iter().any(|(w, _)| *w == second[0].0));
    });
}

trait MintForTests {
    fn mint_into_for_tests(who: u64) -> u128;
}
impl MintForTests for Balances {
    fn mint_into_for_tests(who: u64) -> u128 {
        use frame_support::traits::fungible::Mutate;
        <Balances as Mutate<u64>>::mint_into(&who, FUNDS).unwrap_or_default()
    }
}

// Requirement "质押调整、退出与解绑", Scenario "解绑期内不能取回".
#[test]
fn stake_stays_held_during_unbonding() {
    ext().execute_with(|| {
        assert_ok!(register(ALICE, &[price(MODEL_A, 1, 1)], T2_MIN));
        assert_ok!(Providers::exit(RuntimeOrigin::signed(ALICE)));
        let p = Providers::provider(&ALICE).unwrap();
        assert_eq!(p.status, ProviderStatus::Exiting);
        assert_eq!(p.stake, 0);
        assert!(!Providers::is_serviceable(&ALICE));
        assert!(
            ModelProviders::<Test>::iter_prefix(MODEL_A)
                .next()
                .is_none()
        );
        at(UNBOND);
        assert_noop!(
            Providers::withdraw_unbonded(RuntimeOrigin::signed(ALICE)),
            Error::<Test>::NothingToWithdraw
        );
        assert_eq!(held(ALICE), T2_MIN);
        assert_noop!(
            Providers::update(RuntimeOrigin::signed(ALICE), None, None, None),
            Error::<Test>::Exiting
        );
        assert_noop!(
            Providers::bond_extra(RuntimeOrigin::signed(ALICE), 1),
            Error::<Test>::Exiting
        );
    });
}

// Scenario "解绑期后取回".
#[test]
fn stake_is_released_after_unbonding() {
    ext().execute_with(|| {
        let issuance = issuance_and_burned();
        assert_ok!(register(ALICE, &[price(MODEL_A, 1, 1)], T2_MIN));
        assert_ok!(Providers::exit(RuntimeOrigin::signed(ALICE)));
        at(1 + UNBOND);
        assert_ok!(Providers::withdraw_unbonded(RuntimeOrigin::signed(ALICE)));
        assert!(Providers::provider(&ALICE).is_none());
        assert_eq!(held(ALICE), 0);
        assert_eq!(Balances::balance(&ALICE), FUNDS);
        assert_eq!(issuance_and_burned(), issuance);
        System::assert_last_event(Event::Removed { who: ALICE }.into());
        // The account may register again.
        assert_ok!(register(ALICE, &[price(MODEL_A, 1, 1)], T2_MIN));
    });
}

#[test]
fn partial_unbonding_keeps_the_threshold() {
    ext().execute_with(|| {
        assert_ok!(register(ALICE, &[price(MODEL_A, 1, 1)], T2_MIN + 500));
        assert_noop!(
            Providers::unbond(RuntimeOrigin::signed(ALICE), 501),
            Error::<Test>::BelowThreshold
        );
        assert_noop!(
            Providers::unbond(RuntimeOrigin::signed(ALICE), T2_MIN + 501),
            Error::<Test>::AmountTooLarge
        );
        assert_ok!(Providers::unbond(RuntimeOrigin::signed(ALICE), 500));
        let p = Providers::provider(&ALICE).unwrap();
        assert_eq!((p.stake, unlocking_total(&p.unlocking)), (T2_MIN, 500));
        assert!(Providers::is_serviceable(&ALICE));
        at(1 + UNBOND);
        assert_ok!(Providers::withdraw_unbonded(RuntimeOrigin::signed(ALICE)));
        assert_eq!(held(ALICE), T2_MIN);
        assert!(Providers::provider(&ALICE).is_some());
    });
}

// Requirement "预留的罚没与禁闭接口", Scenario "罚没销毁并计数".
#[test]
fn slashing_burns_and_is_counted() {
    ext().execute_with(|| {
        assert_ok!(register(ALICE, &[price(MODEL_A, 1, 1)], T2_MIN));
        let issuance = Balances::total_issuance();
        let slashed =
            <Providers as ProviderPenalty<u64, u128>>::slash(&ALICE, Perbill::from_percent(10));
        assert_eq!(slashed, T2_MIN / 10);
        assert_eq!(
            Providers::provider(&ALICE).unwrap().stake,
            T2_MIN - T2_MIN / 10
        );
        assert_eq!(held(ALICE), T2_MIN - T2_MIN / 10);
        assert_eq!(Balances::total_issuance(), issuance - slashed);
        assert_eq!(burned(), slashed);
    });
}

#[test]
fn slashing_reaches_unbonding_stake() {
    ext().execute_with(|| {
        assert_ok!(register(ALICE, &[price(MODEL_A, 1, 1)], T2_MIN));
        assert_ok!(Providers::exit(RuntimeOrigin::signed(ALICE)));
        let slashed =
            <Providers as ProviderPenalty<u64, u128>>::slash(&ALICE, Perbill::from_percent(50));
        assert_eq!(slashed, T2_MIN / 2);
        assert_eq!(held(ALICE), T2_MIN / 2);
        let slashed = <Providers as ProviderPenalty<u64, u128>>::slash(&ALICE, Perbill::one());
        assert_eq!(slashed, T2_MIN / 2);
        assert!(Providers::provider(&ALICE).is_none());
    });
}

#[test]
fn a_jailed_provider_is_never_serviceable() {
    ext().execute_with(|| {
        assert_ok!(register(ALICE, &[price(MODEL_A, 1, 1)], T2_MIN));
        assert_ok!(<Providers as ProviderPenalty<u64, u128>>::jail(&ALICE));
        assert_eq!(
            Providers::provider(&ALICE).unwrap().status,
            ProviderStatus::Jailed
        );
        assert!(!Providers::is_serviceable(&ALICE));
        assert!(serviceable_of(MODEL_A).is_empty());
        assert_noop!(
            Providers::heartbeat(RuntimeOrigin::signed(ALICE)),
            Error::<Test>::NotActive
        );
        assert!(<Providers as ProviderPenalty<u64, u128>>::jail(&BOB).is_err());
    });
}

// m5-work-settlement design D10: settlement accepts work of registered, non-jailed providers
// for the models they list, including exiting providers.
#[test]
fn which_work_can_be_settled() {
    ext().execute_with(|| {
        use ac_primitives::market::traits::ProviderLookup;
        let can = |who, model| <Providers as ProviderLookup<u64>>::can_settle(&who, &model);
        assert!(!can(ALICE, MODEL_A));
        assert_ok!(register(ALICE, &[price(MODEL_A, 1, 1)], T2_MIN));
        assert!(can(ALICE, MODEL_A));
        assert!(!can(ALICE, MODEL_B));
        assert_ok!(Providers::exit(RuntimeOrigin::signed(ALICE)));
        assert!(can(ALICE, MODEL_A));
        assert_ok!(register(BOB, &[price(MODEL_A, 1, 1)], T2_MIN));
        assert_ok!(<Providers as ProviderPenalty<u64, u128>>::jail(&BOB));
        assert!(!can(BOB, MODEL_A));
    });
}

// m5-work-settlement design D7: a successful jail notifies settlement exactly once; a failed one
// does not.
#[test]
fn jailing_calls_the_hook_once() {
    ext().execute_with(|| {
        let calls = || crate::mock::JAILED.with(|j| j.borrow().clone());
        let before = calls();
        assert_ok!(register(ALICE, &[price(MODEL_A, 1, 1)], T2_MIN));
        assert_ok!(<Providers as ProviderPenalty<u64, u128>>::jail(&ALICE));
        assert!(<Providers as ProviderPenalty<u64, u128>>::jail(&BOB).is_err());
        let after = calls();
        assert_eq!(after.get(before.len()..), Some(&[ALICE][..]));
    });
}

// Scenario "管理多签不能禁闭提供者": no call of this pallet slashes or jails.
#[test]
fn no_call_slashes_or_jails() {
    let names = <crate::Call<Test> as frame_support::traits::GetCallName>::get_call_names();
    assert_eq!(
        names,
        [
            "register",
            "update",
            "heartbeat",
            "bond_extra",
            "unbond",
            "exit",
            "withdraw_unbonded"
        ]
    );
}

#[derive(Clone, Debug)]
enum Op {
    Bond(u128),
    Unbond(u128),
    Exit,
    Advance(u64),
    Withdraw,
    Slash(u32),
    Register(u128),
}

fn op() -> impl Strategy<Value = Op> {
    prop_oneof![
        (1u128..50_000).prop_map(Op::Bond),
        (1u128..50_000).prop_map(Op::Unbond),
        Just(Op::Exit),
        (1u64..15).prop_map(Op::Advance),
        Just(Op::Withdraw),
        (0u32..=100).prop_map(Op::Slash),
        (T2_MIN..T2_MIN * 3).prop_map(Op::Register),
    ]
}

proptest! {
    // Conservation: the held stake always equals the record's stake plus unbonding, and
    // issuance only drops by what is burned.
    #[test]
    fn stake_accounting_is_conserved(ops in proptest::collection::vec(op(), 1..40)) {
        ext().execute_with(|| {
            let start = issuance_and_burned();
            let _ = register(ALICE, &[price(MODEL_A, 1, 1)], T2_MIN);
            let mut block = 1;
            for op in ops {
                let o = RuntimeOrigin::signed(ALICE);
                let _ = match op {
                    Op::Bond(a) => Providers::bond_extra(o, a),
                    Op::Unbond(a) => Providers::unbond(o, a),
                    Op::Exit => Providers::exit(o),
                    Op::Advance(n) => { block += n; at(block); Ok(()) }
                    Op::Withdraw => Providers::withdraw_unbonded(o),
                    Op::Slash(p) => {
                        <Providers as ProviderPenalty<u64, u128>>::slash(&ALICE, Perbill::from_percent(p));
                        Ok(())
                    }
                    Op::Register(s) => register(ALICE, &[price(MODEL_A, 1, 1)], s),
                };
                prop_assert_eq!(held(ALICE), Providers::total_stake(&ALICE));
                prop_assert_eq!(issuance_and_burned(), start);
            }
            Ok(())
        })?;
    }
}

// What audits read: registration and the current price of a listed model.
#[test]
fn audits_read_registration_and_prices() {
    use ac_primitives::market::traits::ProviderAudit;
    ext().execute_with(|| {
        assert!(!<Providers as ProviderAudit<u64>>::is_registered(&ALICE));
        assert_ok!(register(ALICE, &[price(MODEL_A, 100, 200)], T2_MIN));
        assert!(<Providers as ProviderAudit<u64>>::is_registered(&ALICE));
        assert_eq!(
            <Providers as ProviderAudit<u64>>::price(&ALICE, &MODEL_A),
            Some(price(MODEL_A, 100, 200).price)
        );
        assert_eq!(
            <Providers as ProviderAudit<u64>>::price(&ALICE, &MODEL_B),
            None
        );
    });
}

// Requirement "预留的罚没与禁闭接口", Scenario "被禁闭后退出" (m6-audit-chain).
#[test]
fn a_jailed_provider_exits_with_what_is_left() {
    ext().execute_with(|| {
        assert_ok!(register(ALICE, &[price(MODEL_A, 1, 1)], T2_MIN));
        let slashed =
            <Providers as ProviderPenalty<u64, u128>>::slash(&ALICE, Perbill::from_percent(10));
        assert_ok!(<Providers as ProviderPenalty<u64, u128>>::jail(&ALICE));
        assert!(!Providers::is_serviceable(&ALICE));
        assert_ok!(Providers::exit(RuntimeOrigin::signed(ALICE)));
        at(1 + UNBOND);
        let before = Balances::balance(&ALICE);
        assert_ok!(Providers::withdraw_unbonded(RuntimeOrigin::signed(ALICE)));
        assert_eq!(Balances::balance(&ALICE), before + T2_MIN - slashed);
        assert!(Providers::provider(&ALICE).is_none());
    });
}
