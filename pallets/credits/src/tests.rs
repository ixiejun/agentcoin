//! Unit tests; each names the spec `market/transparent-credits` scenario it covers.
// Test code: an overflow here fails the test loudly (debug builds panic), which is what we want.
#![allow(clippy::arithmetic_side_effects)]

use frame_support::traits::fungible::{Inspect, InspectHold};
use frame_support::{assert_noop, assert_ok};
use proptest::prelude::*;
use sp_core::H256;
use sp_runtime::{AccountId32, DispatchError};

use ac_primitives::market::traits::{Credit, Redemption};
use ac_primitives::market::voucher::key_fingerprint;
use ac_primitives::market::{MicroUsd, SignedVoucher, VoucherCheck, VoucherError};

use crate::mock::{
    ATC, Balances, Credits, DELAY, FUNDS, RuntimeHoldReason, RuntimeOrigin, System, Test, alice,
    bob, ext, gw, key, payee, set_gateway, set_key, set_rate, voucher,
};
use crate::{Channels, Error, Event, HoldReason};

fn held(who: &AccountId32) -> u128 {
    Balances::balance_on_hold(&RuntimeHoldReason::Credits(HoldReason::Escrow), who)
}

fn deposit(who: &AccountId32, amount: u128) -> sp_runtime::DispatchResult {
    Credits::deposit(RuntimeOrigin::signed(who.clone()), gw(), amount)
}

fn redeem(v: &SignedVoucher) -> Result<Redemption<u128>, DispatchError> {
    <Credits as Credit<AccountId32, u128>>::redeem(&gw(), v, &payee())
}

fn at(block: u64) {
    System::set_block_number(block);
}

// Requirement "托管", Scenario "首次托管".
#[test]
fn a_first_deposit_opens_the_channel() {
    ext().execute_with(|| {
        let issuance = Balances::total_issuance();
        assert_ok!(deposit(&alice(), 10 * ATC));
        let ch = Credits::channel(&alice(), &gw()).unwrap();
        assert_eq!(
            (ch.escrow, ch.number, ch.redeemed),
            (10 * ATC, 0, MicroUsd::ZERO)
        );
        assert_eq!(ch.key, key_fingerprint(&key("alice").public_key().unwrap()));
        assert_eq!(held(&alice()), 10 * ATC);
        assert_eq!(Balances::total_issuance(), issuance);
        System::assert_last_event(
            Event::Deposited {
                user: alice(),
                gateway: gw(),
                amount: 10 * ATC,
            }
            .into(),
        );
        assert_ok!(deposit(&alice(), ATC));
        assert_eq!(Credits::channel(&alice(), &gw()).unwrap().escrow, 11 * ATC);
    });
}

// Scenario "网关已退出" (spec market/gateways "退出后不能接受新托管" too), and the other refusals.
#[test]
fn deposits_need_an_active_gateway_and_a_key() {
    ext().execute_with(|| {
        set_gateway(&gw(), false);
        assert_noop!(deposit(&alice(), ATC), Error::<Test>::GatewayNotActive);
        set_gateway(&gw(), true);
        assert_noop!(deposit(&alice(), 0), Error::<Test>::ZeroAmount);
        assert_noop!(deposit(&payee(), ATC), Error::<Test>::NoKey);
        assert_noop!(
            deposit(&alice(), FUNDS + 1),
            Error::<Test>::InsufficientBalance
        );
    });
}

// Requirement "兑付规则", Scenario "部分兑付".
#[test]
fn a_voucher_pays_its_increment() {
    ext().execute_with(|| {
        assert_ok!(deposit(&alice(), 10 * ATC));
        assert_eq!(
            redeem(&voucher(&alice(), &key("alice"), 0, 100))
                .unwrap()
                .paid,
            100_000_000_000_000
        );
        let before = Balances::balance(&payee());
        let r = redeem(&voucher(&alice(), &key("alice"), 0, 250)).unwrap();
        assert_eq!(
            r,
            Redemption {
                paid: 150_000_000_000_000,
                shortfall: 0
            }
        );
        assert_eq!(Balances::balance(&payee()) - before, 150_000_000_000_000);
        let ch = Credits::channel(&alice(), &gw()).unwrap();
        assert_eq!(ch.redeemed, MicroUsd(250));
        assert_eq!(ch.escrow, 10 * ATC - 250_000_000_000_000);
        assert_eq!(held(&alice()), ch.escrow);
    });
}

// Scenario "重复兑付", and an older voucher after a newer one.
#[test]
fn replays_pay_nothing() {
    ext().execute_with(|| {
        assert_ok!(deposit(&alice(), 10 * ATC));
        let v = voucher(&alice(), &key("alice"), 0, 500);
        assert_ok!(redeem(&v));
        let ch = Credits::channel(&alice(), &gw());
        assert_eq!(redeem(&v), Ok(Redemption::default()));
        assert_eq!(
            redeem(&voucher(&alice(), &key("alice"), 0, 400)),
            Ok(Redemption::default())
        );
        assert_eq!(Credits::channel(&alice(), &gw()), ch);
    });
}

// Scenario "托管不足".
#[test]
fn a_short_escrow_pays_what_it_has() {
    ext().execute_with(|| {
        assert_ok!(deposit(&alice(), 3 * ATC));
        let r = redeem(&voucher(&alice(), &key("alice"), 0, 5_000_000)).unwrap();
        assert_eq!(
            r,
            Redemption {
                paid: 3 * ATC,
                shortfall: 2 * ATC
            }
        );
        let ch = Credits::channel(&alice(), &gw()).unwrap();
        assert_eq!((ch.escrow, ch.redeemed), (0, MicroUsd(5_000_000)));
        System::assert_last_event(
            Event::Redeemed {
                user: alice(),
                gateway: gw(),
                paid: 3 * ATC,
                shortfall: 2 * ATC,
            }
            .into(),
        );
    });
}

// Scenario "通道号不符".
#[test]
fn vouchers_of_a_reset_channel_are_void() {
    ext().execute_with(|| {
        assert_ok!(deposit(&alice(), ATC));
        assert_ok!(Credits::request_withdrawal(
            RuntimeOrigin::signed(alice()),
            gw(),
            ATC
        ));
        at(1 + DELAY);
        assert_ok!(Credits::withdraw(RuntimeOrigin::signed(alice()), gw()));
        assert_ok!(deposit(&alice(), ATC));
        let ch = Credits::channel(&alice(), &gw());
        assert_noop!(
            redeem(&voucher(&alice(), &key("alice"), 0, 1)),
            Error::<Test>::WrongChannel
        );
        assert_eq!(Credits::channel(&alice(), &gw()), ch);
        assert_ok!(redeem(&voucher(&alice(), &key("alice"), 1, 1)));
    });
}

// Scenario "其他链的凭证无效", a protocol signature (tested in ac-primitives) and the other
// refusals leave the channel untouched.
#[test]
fn invalid_vouchers_change_nothing() {
    ext().execute_with(|| {
        assert_ok!(deposit(&alice(), ATC));
        let mut other_chain = voucher(&alice(), &key("alice"), 0, 1);
        other_chain.body.genesis = H256::repeat_byte(0x42);
        assert_noop!(redeem(&other_chain), Error::<Test>::WrongGenesis);
        assert_noop!(
            redeem(&voucher(&alice(), &key("bob"), 0, 1)),
            Error::<Test>::WrongKey
        );
        let mut tampered = voucher(&alice(), &key("alice"), 0, 1);
        tampered.body.cumulative = MicroUsd(2);
        assert_noop!(redeem(&tampered), Error::<Test>::BadSignature);
        assert_noop!(
            redeem(&voucher(&bob(), &key("bob"), 0, 1)),
            Error::<Test>::NoChannel
        );
        let v = voucher(&alice(), &key("alice"), 0, 1);
        assert_noop!(
            <Credits as Credit<AccountId32, u128>>::redeem(&bob(), &v, &payee()),
            Error::<Test>::NoChannel
        );
        set_rate(None);
        assert_noop!(redeem(&v), Error::<Test>::RateNotSet);
    });
}

#[test]
fn the_payee_must_exist() {
    ext().execute_with(|| {
        assert_ok!(deposit(&alice(), ATC));
        let v = voucher(&alice(), &key("alice"), 0, 1);
        let nobody = AccountId32::new([99; 32]);
        assert_noop!(
            <Credits as Credit<AccountId32, u128>>::redeem(&gw(), &v, &nobody),
            Error::<Test>::TransferFailed
        );
    });
}

// Requirement "链下校验与链上一致", Scenario "一致性向量".
#[test]
fn off_chain_checks_agree_with_redemption() {
    ext().execute_with(|| {
        assert_ok!(deposit(&alice(), ATC));
        let (a, b) = (key("alice"), key("bob"));
        let mut bad_sig = voucher(&alice(), &a, 0, 10);
        bad_sig.body.cumulative = MicroUsd(11);
        let cases = [
            voucher(&alice(), &a, 0, 10),        // valid
            bad_sig,                             // bad signature
            voucher(&alice(), &a, 7, 10),        // wrong channel
            voucher(&alice(), &b, 0, 10),        // wrong key
            voucher(&alice(), &a, 0, 5_000_000), // beyond the escrow
            voucher(&alice(), &a, 0, 10),        // no longer grows after the first
        ];
        for v in cases {
            let checked = Credits::check(&v);
            let before = Credits::channel(&alice(), &gw()).unwrap();
            let redeemed = redeem(&v);
            match (checked, redeemed) {
                (Ok(c), Ok(r)) => {
                    assert_eq!(r.paid + r.shortfall, c.increment_atc);
                    assert_eq!(c.covered, r.shortfall == 0);
                    assert_eq!(
                        c.increment,
                        v.body.cumulative.saturating_sub(before.redeemed)
                    );
                }
                (Err(e), Err(d)) => assert_eq!(d, Error::<Test>::from(e).into()),
                other => panic!("check and redemption disagree: {other:?}"),
            }
        }
    });
}

#[test]
fn the_check_reports_coverage() {
    ext().execute_with(|| {
        assert_ok!(deposit(&alice(), ATC));
        assert_eq!(
            Credits::check(&voucher(&alice(), &key("alice"), 0, 2_000_000)),
            Ok(VoucherCheck {
                increment: MicroUsd(2_000_000),
                increment_atc: 2 * ATC,
                covered: false
            })
        );
        assert_eq!(
            Credits::check(&voucher(&alice(), &key("alice"), 3, 1)),
            Err(VoucherError::WrongChannel)
        );
    });
}

// Requirement "取回托管", Scenario "等待期内仍可兑付".
#[test]
fn a_pending_withdrawal_can_still_be_redeemed_from() {
    ext().execute_with(|| {
        assert_ok!(deposit(&alice(), 2 * ATC));
        assert_ok!(Credits::request_withdrawal(
            RuntimeOrigin::signed(alice()),
            gw(),
            2 * ATC
        ));
        assert_ok!(redeem(&voucher(&alice(), &key("alice"), 0, 500_000)));
        at(1 + DELAY);
        assert_ok!(Credits::withdraw(RuntimeOrigin::signed(alice()), gw()));
        assert_eq!(Balances::balance(&alice()), FUNDS - ATC / 2);
        assert_eq!(held(&alice()), 0);
    });
}

// Scenario "全部取回后重置".
#[test]
fn an_emptied_channel_resets() {
    ext().execute_with(|| {
        assert_ok!(deposit(&alice(), ATC));
        assert_ok!(redeem(&voucher(&alice(), &key("alice"), 0, 10)));
        assert_ok!(Credits::request_withdrawal(
            RuntimeOrigin::signed(alice()),
            gw(),
            ATC / 2
        ));
        assert_ok!(Credits::request_withdrawal(
            RuntimeOrigin::signed(alice()),
            gw(),
            ATC / 4
        ));
        assert_noop!(
            Credits::request_withdrawal(RuntimeOrigin::signed(alice()), gw(), ATC),
            Error::<Test>::AmountTooLarge
        );
        at(1 + DELAY);
        assert_ok!(Credits::withdraw(RuntimeOrigin::signed(alice()), gw()));
        assert_eq!(Credits::channel(&alice(), &gw()).unwrap().number, 0);
        let rest = Credits::channel(&alice(), &gw()).unwrap().escrow;
        assert_ok!(Credits::request_withdrawal(
            RuntimeOrigin::signed(alice()),
            gw(),
            rest
        ));
        at(1 + 2 * DELAY);
        assert_ok!(Credits::withdraw(RuntimeOrigin::signed(alice()), gw()));
        let ch = Credits::channel(&alice(), &gw()).unwrap();
        assert_eq!((ch.escrow, ch.number, ch.redeemed), (0, 1, MicroUsd::ZERO));
        System::assert_last_event(
            Event::ChannelReset {
                user: alice(),
                gateway: gw(),
                number: 1,
            }
            .into(),
        );
    });
}

// Scenario "等待期未到".
#[test]
fn a_withdrawal_waits_for_the_delay() {
    ext().execute_with(|| {
        assert_ok!(deposit(&alice(), ATC));
        assert_noop!(
            Credits::withdraw(RuntimeOrigin::signed(alice()), gw()),
            Error::<Test>::NoWithdrawal
        );
        at(5);
        assert_ok!(Credits::request_withdrawal(
            RuntimeOrigin::signed(alice()),
            gw(),
            ATC
        ));
        at(5 + DELAY - 1);
        assert_noop!(
            Credits::withdraw(RuntimeOrigin::signed(alice()), gw()),
            Error::<Test>::TooEarly
        );
        assert_eq!(held(&alice()), ATC);
    });
}

// Requirement "更换凭证密钥", Scenario "轮换账户密钥不影响在途凭证".
#[test]
fn a_key_rotation_keeps_vouchers_in_flight_valid() {
    ext().execute_with(|| {
        assert_ok!(deposit(&alice(), ATC));
        let v = voucher(&alice(), &key("alice"), 0, 10);
        set_key(&alice(), &key("alice-2").public_key().unwrap());
        assert_ok!(redeem(&v));
        // A later deposit into the open channel keeps the voucher key too.
        assert_ok!(deposit(&alice(), ATC));
        assert_ok!(redeem(&voucher(&alice(), &key("alice"), 0, 20)));
    });
}

// Scenario "更换生效后旧凭证无效".
#[test]
fn a_key_change_takes_effect_after_the_delay() {
    ext().execute_with(|| {
        assert_ok!(deposit(&alice(), ATC));
        set_key(&alice(), &key("alice-2").public_key().unwrap());
        assert_ok!(Credits::request_key_change(
            RuntimeOrigin::signed(alice()),
            gw()
        ));
        at(DELAY);
        assert_ok!(redeem(&voucher(&alice(), &key("alice"), 0, 10)));
        at(1 + DELAY);
        assert_noop!(
            redeem(&voucher(&alice(), &key("alice"), 0, 20)),
            Error::<Test>::WrongKey
        );
        assert_ok!(redeem(&voucher(&alice(), &key("alice-2"), 0, 20)));
        let ch = Channels::<Test>::get(alice(), gw()).unwrap();
        assert_eq!(
            ch.key,
            key_fingerprint(&key("alice-2").public_key().unwrap())
        );
        assert_eq!(ch.pending_key, None);
    });
}

// Requirement "额度接口与实现可替换", Scenario "以模拟实现替换": code written against the
// `Credit` interface runs unchanged on another implementation.
#[test]
fn the_credit_interface_is_replaceable() {
    fn settle<C: Credit<AccountId32, u128>>(
        gateway: &AccountId32,
        vouchers: &[C::Voucher],
    ) -> (u128, usize) {
        vouchers.iter().fold((0, 0), |(paid, failed), v| {
            match C::redeem(gateway, v, &AccountId32::new([20; 32])) {
                Ok(r) => (paid + r.paid, failed),
                Err(_) => (paid, failed + 1),
            }
        })
    }
    struct Fake;
    impl Credit<AccountId32, u128> for Fake {
        type Voucher = u128;
        fn redeem(
            _: &AccountId32,
            v: &u128,
            _: &AccountId32,
        ) -> Result<Redemption<u128>, DispatchError> {
            if *v == 0 {
                Err(DispatchError::Other("empty"))
            } else {
                Ok(Redemption {
                    paid: *v,
                    shortfall: 0,
                })
            }
        }
    }
    assert_eq!(settle::<Fake>(&gw(), &[3, 0, 4]), (7, 1));
    ext().execute_with(|| {
        assert_ok!(deposit(&alice(), ATC));
        let vouchers = [
            voucher(&alice(), &key("alice"), 0, 1),
            voucher(&alice(), &key("bob"), 0, 2),
        ];
        assert_eq!(settle::<Credits>(&gw(), &vouchers), (1_000_000_000_000, 1));
    });
}

#[derive(Clone, Debug)]
enum Op {
    Deposit(u128),
    Redeem(u128),
    Request(u128),
    Withdraw,
    Advance(u64),
    ChangeKey,
}

fn op() -> impl Strategy<Value = Op> {
    prop_oneof![
        (1u128..5).prop_map(|n| Op::Deposit(n * ATC / 2)),
        (1u128..3_000_000).prop_map(Op::Redeem),
        (1u128..4).prop_map(|n| Op::Request(n * ATC / 3)),
        Just(Op::Withdraw),
        (1u64..8).prop_map(Op::Advance),
        Just(Op::ChangeKey),
    ]
}

proptest! {
    // Conservation: the escrow hold always equals the channel's escrow, nothing is minted or
    // burned, and what left the escrow went to the payee or back to the user.
    #[test]
    fn escrow_accounting_is_conserved(ops in proptest::collection::vec(op(), 1..30)) {
        ext().execute_with(|| {
            let issuance = Balances::total_issuance();
            let start = Balances::total_balance(&alice()) + Balances::total_balance(&payee());
            let (mut block, mut cumulative) = (1u64, 0u128);
            let k = key("alice");
            for op in ops {
                let o = RuntimeOrigin::signed(alice());
                match op {
                    Op::Deposit(a) => { let _ = Credits::deposit(o, gw(), a); }
                    Op::Redeem(step) => {
                        cumulative += step;
                        let number = Credits::channel(&alice(), &gw()).map_or(0, |c| c.number);
                        let _ = redeem(&voucher(&alice(), &k, number, cumulative));
                    }
                    Op::Request(a) => { let _ = Credits::request_withdrawal(o, gw(), a); }
                    Op::Withdraw => {
                        if Credits::withdraw(o, gw()).is_ok()
                            && Credits::channel(&alice(), &gw()).is_some_and(|c| c.redeemed == MicroUsd::ZERO)
                        {
                            cumulative = 0;
                        }
                    }
                    Op::Advance(n) => { block += n; at(block); }
                    Op::ChangeKey => { let _ = Credits::request_key_change(o, gw()); }
                }
                let escrow = Credits::channel(&alice(), &gw()).map_or(0, |c| c.escrow);
                prop_assert_eq!(held(&alice()), escrow);
                prop_assert_eq!(Balances::total_issuance(), issuance);
                prop_assert_eq!(Balances::total_balance(&alice()) + Balances::total_balance(&payee()), start);
            }
            Ok(())
        })?;
    }
}
