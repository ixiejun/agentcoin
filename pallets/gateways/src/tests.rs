//! Unit tests; each names the spec `market/gateways` scenario it covers.
// Test code: an overflow here fails the test loudly (debug builds panic), which is what we want.
#![allow(clippy::arithmetic_side_effects)]

use frame_support::traits::fungible::{Inspect, InspectHold};
use frame_support::{BoundedVec, assert_noop, assert_ok};
use proptest::prelude::*;

use ac_primitives::market::records::GatewayStatus;
use ac_primitives::market::traits::GatewayLookup;

use crate::mock::{
    ALICE, BOB, Balances, FUNDS, Gateways, MIN, RuntimeHoldReason, RuntimeOrigin, System, Test,
    UNBOND, ext, register, set_rate,
};
use crate::{Error, Event, HoldReason};

fn held(who: u64) -> u128 {
    Balances::balance_on_hold(&RuntimeHoldReason::Gateways(HoldReason::Stake), &who)
}

// Requirement "网关登记", Scenario "正常登记".
#[test]
fn a_gateway_registers() {
    ext().execute_with(|| {
        let issuance = Balances::total_issuance();
        assert_ok!(register(ALICE, MIN));
        let g = Gateways::gateway(&ALICE).unwrap();
        assert_eq!(g.endpoint.to_vec(), b"https://g.example".to_vec());
        assert_eq!(
            (g.fee_bps, g.stake, g.status),
            (300, MIN, GatewayStatus::Active)
        );
        assert_eq!(held(ALICE), MIN);
        assert_eq!(Balances::total_issuance(), issuance);
        assert!(<Gateways as GatewayLookup<u64>>::is_active(&ALICE));
        assert_eq!(<Gateways as GatewayLookup<u64>>::fee_bps(&ALICE), Some(300));
        System::assert_last_event(
            Event::Registered {
                who: ALICE,
                fee_bps: 300,
                stake: MIN,
            }
            .into(),
        );
        assert_noop!(register(ALICE, MIN), Error::<Test>::AlreadyRegistered);
    });
}

// Scenario "费率超过上限".
#[test]
fn a_fee_above_five_percent_is_refused() {
    ext().execute_with(|| {
        let e = BoundedVec::truncate_from(b"https://g".to_vec());
        assert_noop!(
            Gateways::register(RuntimeOrigin::signed(ALICE), e.clone(), 501, MIN),
            Error::<Test>::FeeTooHigh
        );
        assert_ok!(Gateways::register(
            RuntimeOrigin::signed(ALICE),
            e,
            500,
            MIN
        ));
    });
}

#[test]
fn threshold_endpoint_and_rate_are_checked() {
    ext().execute_with(|| {
        assert_noop!(register(ALICE, MIN - 1), Error::<Test>::BelowThreshold);
        assert_noop!(
            Gateways::register(RuntimeOrigin::signed(ALICE), BoundedVec::default(), 0, MIN),
            Error::<Test>::InvalidEndpoint
        );
        set_rate(None);
        assert_noop!(register(ALICE, MIN), Error::<Test>::RateNotSet);
    });
}

// Requirement "修改与质押调整", Scenario "调高费率超限".
#[test]
fn an_update_keeps_the_fee_cap() {
    ext().execute_with(|| {
        assert_ok!(register(ALICE, MIN));
        assert_noop!(
            Gateways::update(RuntimeOrigin::signed(ALICE), None, Some(600)),
            Error::<Test>::FeeTooHigh
        );
        assert_eq!(Gateways::gateway(&ALICE).unwrap().fee_bps, 300);
        assert_ok!(Gateways::update(
            RuntimeOrigin::signed(ALICE),
            Some(BoundedVec::truncate_from(b"https://new".to_vec())),
            Some(100)
        ));
        let g = Gateways::gateway(&ALICE).unwrap();
        assert_eq!(
            (g.endpoint.to_vec(), g.fee_bps),
            (b"https://new".to_vec(), 100)
        );
        assert_noop!(
            Gateways::update(RuntimeOrigin::signed(BOB), None, None),
            Error::<Test>::NotGateway
        );
    });
}

#[test]
fn stake_can_grow_and_shrink_down_to_the_threshold() {
    ext().execute_with(|| {
        assert_ok!(register(ALICE, MIN));
        assert_ok!(Gateways::bond_extra(RuntimeOrigin::signed(ALICE), 500));
        assert_noop!(
            Gateways::unbond(RuntimeOrigin::signed(ALICE), 501),
            Error::<Test>::BelowThreshold
        );
        assert_ok!(Gateways::unbond(RuntimeOrigin::signed(ALICE), 500));
        assert_eq!(held(ALICE), MIN + 500);
        System::set_block_number(1 + UNBOND);
        assert_ok!(Gateways::withdraw_unbonded(RuntimeOrigin::signed(ALICE)));
        assert_eq!(held(ALICE), MIN);
    });
}

// Requirement "退出与解绑", Scenario "退出后不能接受新托管" (the gateway side: it is no longer
// active, which pallet-credits checks before accepting escrow).
#[test]
fn an_exiting_gateway_is_not_active() {
    ext().execute_with(|| {
        assert_ok!(register(ALICE, MIN));
        assert_ok!(Gateways::exit(RuntimeOrigin::signed(ALICE)));
        assert!(!<Gateways as GatewayLookup<u64>>::is_active(&ALICE));
        assert_eq!(
            Gateways::gateway(&ALICE).unwrap().status,
            GatewayStatus::Exiting
        );
        assert_noop!(
            Gateways::update(RuntimeOrigin::signed(ALICE), None, Some(1)),
            Error::<Test>::Exiting
        );
        assert_noop!(
            Gateways::bond_extra(RuntimeOrigin::signed(ALICE), 1),
            Error::<Test>::Exiting
        );
        assert_noop!(
            Gateways::withdraw_unbonded(RuntimeOrigin::signed(ALICE)),
            Error::<Test>::NothingToWithdraw
        );
    });
}

// Scenario "解绑期后取回".
#[test]
fn stake_is_released_after_unbonding() {
    ext().execute_with(|| {
        let issuance = Balances::total_issuance();
        assert_ok!(register(ALICE, MIN));
        assert_ok!(Gateways::exit(RuntimeOrigin::signed(ALICE)));
        System::set_block_number(1 + UNBOND);
        assert_ok!(Gateways::withdraw_unbonded(RuntimeOrigin::signed(ALICE)));
        assert!(Gateways::gateway(&ALICE).is_none());
        assert_eq!(Balances::balance(&ALICE), FUNDS);
        assert_eq!(Balances::total_issuance(), issuance);
        System::assert_last_event(Event::Removed { who: ALICE }.into());
    });
}

#[derive(Clone, Debug)]
enum Op {
    Bond(u128),
    Unbond(u128),
    Exit,
    Advance(u64),
    Withdraw,
    Register(u128),
}

fn op() -> impl Strategy<Value = Op> {
    prop_oneof![
        (1u128..500_000).prop_map(Op::Bond),
        (1u128..500_000).prop_map(Op::Unbond),
        Just(Op::Exit),
        (1u64..15).prop_map(Op::Advance),
        Just(Op::Withdraw),
        (MIN..MIN * 3).prop_map(Op::Register),
    ]
}

proptest! {
    // Conservation: held stake equals stake plus unbonding; the issuance never changes.
    #[test]
    fn stake_accounting_is_conserved(ops in proptest::collection::vec(op(), 1..40)) {
        ext().execute_with(|| {
            let issuance = Balances::total_issuance();
            let mut block = 1;
            for op in ops {
                let o = RuntimeOrigin::signed(ALICE);
                let _ = match op {
                    Op::Bond(a) => Gateways::bond_extra(o, a),
                    Op::Unbond(a) => Gateways::unbond(o, a),
                    Op::Exit => Gateways::exit(o),
                    Op::Advance(n) => { block += n; System::set_block_number(block); Ok(()) }
                    Op::Withdraw => Gateways::withdraw_unbonded(o),
                    Op::Register(s) => register(ALICE, s),
                };
                prop_assert_eq!(held(ALICE), Gateways::total_stake(&ALICE));
                prop_assert_eq!(Balances::total_issuance(), issuance);
            }
            Ok(())
        })?;
    }
}
