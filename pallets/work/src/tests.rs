//! Unit tests: one per scenario of spec `market/work-settlement`, plus conservation properties.
// Test code: AGENT.md §5.3 permits unwrap/expect/panics; overflows fail loudly.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use frame_support::traits::fungible::{Inspect, InspectHold};
use frame_support::{BoundedVec, assert_noop, assert_ok};
use sp_runtime::AccountId32;

use ac_primitives::emission::{EpochIndex, WorkSource};
use ac_primitives::market::traits::OnJail;
use ac_primitives::market::work::JobKind;
use ac_primitives::market::{MicroUsd, ReportEntry, SignedVoucher};

use crate::mock::{
    Balances, Credits, GW_FUNDS, MODEL, OTHER_MODEL, P_FUNDS, PARAMS, RuntimeHoldReason,
    RuntimeOrigin, System, Test, UNITS_PER_USD, Work, alice, bob, burned, entry, ext, gw, key, p1,
    p2, set_epoch, set_gateway, set_rate, settle_epoch, stranger, voucher, voucher_for,
};
use crate::{Error, Event, HeldFor, HoldReason, Reports, WorkOf};

const ROOT: [u8; 32] = [0xab; 32];

fn deposit(user: &AccountId32, amount: u128) {
    assert_ok!(Credits::deposit(
        RuntimeOrigin::signed(user.clone()),
        gw(),
        amount
    ));
}

fn submit(
    who: &AccountId32,
    entries: Vec<ReportEntry<AccountId32>>,
    vouchers: Vec<SignedVoucher>,
) -> sp_runtime::DispatchResult {
    let receipts = u32::try_from(entries.len()).unwrap();
    Work::submit_report(
        RuntimeOrigin::signed(who.clone()),
        ROOT,
        receipts,
        BoundedVec::truncate_from(entries),
        BoundedVec::truncate_from(vouchers),
    )
}

fn claim(who: &AccountId32, items: &[(EpochIndex, AccountId32)]) -> sp_runtime::DispatchResult {
    Work::claim(
        RuntimeOrigin::signed(stranger()),
        who.clone(),
        BoundedVec::truncate_from(items.to_vec()),
    )
}

fn held_on(who: &AccountId32) -> u128 {
    Balances::balance_on_hold(&RuntimeHoldReason::Work(HoldReason::Pending), who)
}

fn free(who: &AccountId32) -> u128 {
    Balances::balance(who)
}

fn redeemed(user: &AccountId32) -> MicroUsd {
    Credits::channel(user, &gw()).unwrap().redeemed
}

/// The spec's allocation example: alice pays $1 through one voucher, p1 and p2 served 3:1.
fn one_dollar_report() {
    deposit(&alice(), 2 * UNITS_PER_USD);
    assert_ok!(submit(
        &gw(),
        vec![entry(&p1(), 750_000), entry(&p2(), 250_000)],
        vec![voucher(&alice(), 1_000_000)],
    ));
}

// Scenarios "汇总与凭证一致" and "分配结果".
#[test]
fn a_matching_report_is_settled_and_allocated() {
    ext().execute_with(|| {
        let before = free(&gw());
        one_dollar_report();
        // G = 1,000,000 units: 200,000 burned, 50,000 fee, 562,500 and 187,500 shares.
        assert_eq!(burned(), 200_000);
        assert_eq!(held_on(&gw()), 50_000 + 562_500 + 187_500);
        // The gateway received G and paid the burn and the held amounts out of it.
        assert_eq!(free(&gw()), before);
        assert_eq!(redeemed(&alice()), MicroUsd(1_000_000));
        let r = Work::report(0).unwrap();
        assert_eq!((r.submitted, r.matures), (0, 2));
        assert_eq!(
            (r.settled, r.burned, r.gateway_fee),
            (1_000_000, 200_000, 50_000)
        );
        assert_eq!(r.lines[0].share, 562_500);
        assert_eq!(r.lines[1].share, 187_500);
        // Scenario "工作量计算": G_i = 750,000 → 375,000 at k = 0.5.
        assert_eq!((r.lines[0].work, r.lines[1].work), (375_000, 125_000));
        assert_eq!(<Work as WorkSource>::verified_work(2), (500_000, 0));
        assert_eq!(
            HeldFor::<Test>::get((p1(), 2, gw())).unwrap().shares,
            562_500
        );
        assert_eq!(HeldFor::<Test>::get((gw(), 2, gw())).unwrap().fees, 50_000);
        System::assert_last_event(
            Event::ReportAccepted {
                id: 0,
                gateway: gw(),
                root: ROOT,
                settled: 1_000_000,
                burned: 200_000,
                matures: 2,
            }
            .into(),
        );
    });
}

// Scenario "汇总与凭证一致" with two vouchers: 0.7 dollars on both sides.
#[test]
fn totals_across_vouchers_match() {
    ext().execute_with(|| {
        deposit(&alice(), UNITS_PER_USD);
        deposit(&bob(), UNITS_PER_USD);
        assert_ok!(submit(
            &gw(),
            vec![entry(&p1(), 400_000), entry(&p2(), 300_000)],
            vec![voucher(&alice(), 500_000), voucher(&bob(), 200_000)],
        ));
        assert_eq!(redeemed(&alice()), MicroUsd(500_000));
        assert_eq!(redeemed(&bob()), MicroUsd(200_000));
    });
}

// Scenario "汇总超出凭证".
#[test]
fn totals_above_the_vouchers_are_rejected() {
    ext().execute_with(|| {
        deposit(&alice(), UNITS_PER_USD);
        let before = free(&gw());
        assert_noop!(
            submit(
                &gw(),
                vec![entry(&p1(), 800_000)],
                vec![voucher(&alice(), 700_000)]
            ),
            Error::<Test>::TotalsMismatch
        );
        assert_eq!(redeemed(&alice()), MicroUsd(0));
        assert_eq!(free(&gw()), before);
    });
}

// Scenario "一张凭证无效": a stale channel number rejects the whole report.
#[test]
fn one_invalid_voucher_rejects_the_report() {
    ext().execute_with(|| {
        deposit(&alice(), UNITS_PER_USD);
        deposit(&bob(), UNITS_PER_USD);
        let stale = voucher_for(&bob(), &gw(), &key("bob"), 1, 200_000);
        assert_noop!(
            submit(
                &gw(),
                vec![entry(&p1(), 700_000)],
                vec![voucher(&alice(), 500_000), stale]
            ),
            pallet_credits::Error::<Test>::WrongChannel
        );
        assert_eq!(redeemed(&alice()), MicroUsd(0));
    });
}

// Scenario "汇总中的提供者不服务该模型".
#[test]
fn a_provider_must_list_the_model() {
    ext().execute_with(|| {
        deposit(&alice(), UNITS_PER_USD);
        let mut e = entry(&p1(), 500_000);
        e.model = OTHER_MODEL;
        assert_noop!(
            submit(&gw(), vec![e], vec![voucher(&alice(), 500_000)]),
            Error::<Test>::ProviderCannotSettle
        );
    });
}

// Scenario "非网关提交"; an exiting gateway still settles (design D10).
#[test]
fn only_registered_gateways_submit() {
    ext().execute_with(|| {
        deposit(&alice(), UNITS_PER_USD);
        assert_noop!(
            submit(
                &stranger(),
                vec![entry(&p1(), 500_000)],
                vec![voucher(&alice(), 500_000)]
            ),
            Error::<Test>::NotGateway
        );
        set_gateway(&gw(), false, 500);
        assert_ok!(submit(
            &gw(),
            vec![entry(&p1(), 500_000)],
            vec![voucher(&alice(), 500_000)]
        ));
    });
}

// Scenario "网关补足短缺": the voucher is worth 5 dollars, the escrow only 3.
#[test]
fn the_gateway_covers_a_shortfall() {
    ext().execute_with(|| {
        deposit(&alice(), 3 * UNITS_PER_USD);
        let before = free(&gw());
        assert_ok!(submit(
            &gw(),
            vec![entry(&p1(), 5_000_000)],
            vec![voucher(&alice(), 5_000_000)]
        ));
        assert_eq!(Work::report(0).unwrap().settled, 5 * UNITS_PER_USD);
        // Received 3, paid out 5 (burned or held): 2 less free balance.
        assert_eq!(before - free(&gw()), 2 * UNITS_PER_USD);
        assert_eq!(Credits::channel(&alice(), &gw()).unwrap().escrow, 0);
    });
}

// Scenario "网关无力补足".
#[test]
fn a_gateway_that_cannot_cover_is_rejected() {
    ext().execute_with(|| {
        deposit(&alice(), 3 * UNITS_PER_USD);
        // Leave the gateway one dollar of free balance.
        assert_ok!(
            <Balances as frame_support::traits::fungible::Mutate<_>>::transfer(
                &gw(),
                &stranger(),
                GW_FUNDS - UNITS_PER_USD,
                frame_support::traits::tokens::Preservation::Preserve
            )
        );
        assert_noop!(
            submit(
                &gw(),
                vec![entry(&p1(), 5_000_000)],
                vec![voucher(&alice(), 5_000_000)]
            ),
            Error::<Test>::ShortfallNotCovered
        );
        let ch = Credits::channel(&alice(), &gw()).unwrap();
        assert_eq!((ch.escrow, ch.redeemed), (3 * UNITS_PER_USD, MicroUsd(0)));
    });
}

#[test]
fn malformed_reports_are_rejected() {
    ext().execute_with(|| {
        deposit(&alice(), UNITS_PER_USD);
        deposit(&bob(), UNITS_PER_USD);
        let v = || vec![voucher(&alice(), 500_000)];
        assert_noop!(submit(&gw(), vec![], v()), Error::<Test>::NoEntries);
        assert_noop!(
            submit(&gw(), vec![entry(&p1(), 500_000)], vec![]),
            Error::<Test>::NoVouchers
        );
        let mut eval = entry(&p1(), 500_000);
        eval.kind = JobKind::Eval;
        assert_noop!(
            submit(&gw(), vec![eval], v()),
            Error::<Test>::UnsupportedJobKind
        );
        assert_noop!(
            submit(&gw(), vec![entry(&p1(), 0), entry(&p2(), 500_000)], v()),
            Error::<Test>::ZeroAmount
        );
        assert_noop!(
            submit(
                &gw(),
                vec![entry(&p1(), 250_000), entry(&p1(), 250_000)],
                v()
            ),
            Error::<Test>::DuplicateEntry
        );
        assert_noop!(
            submit(
                &gw(),
                vec![entry(&p1(), 700_000)],
                vec![voucher(&alice(), 500_000), voucher(&alice(), 200_000)]
            ),
            Error::<Test>::DuplicateChannel
        );
        assert_noop!(
            Work::submit_report(
                RuntimeOrigin::signed(gw()),
                ROOT,
                1,
                BoundedVec::truncate_from(vec![entry(&p1(), 250_000), entry(&p2(), 250_000)]),
                BoundedVec::truncate_from(v()),
            ),
            Error::<Test>::TooFewReceipts
        );
        set_rate(None);
        assert_noop!(
            submit(&gw(), vec![entry(&p1(), 500_000)], v()),
            Error::<Test>::RateNotSet
        );
    });
}

// Scenario "挑战期内不可领取".
#[test]
fn nothing_is_claimable_during_the_challenge_period() {
    ext().execute_with(|| {
        set_epoch(5);
        one_dollar_report();
        assert_eq!(Work::report(0).unwrap().matures, 7);
        for epoch in 5..=7 {
            set_epoch(epoch);
            let before = free(&p1());
            assert_ok!(claim(&p1(), &[(7, gw())]));
            assert_eq!(free(&p1()), before);
        }
    });
}

// Scenarios "到期后计入工作量" and "领取费用与排放".
#[test]
fn matured_payments_and_emission_are_claimed() {
    ext().execute_with(|| {
        set_epoch(5);
        one_dollar_report();
        // Epochs before 7 carry no work from this report.
        assert_eq!(<Work as WorkSource>::verified_work(6), (0, 0));
        assert_eq!(<Work as WorkSource>::verified_work(7), (500_000, 0));
        set_epoch(8);
        // Emission mints 400,000 for epoch 7; the pot keeps one unit (the existential deposit).
        settle_epoch(7, 400_001);
        assert_eq!(Work::epoch_work(7).market, Some(400_000));
        let pot_before = free(&Work::pot());
        let gw_held = held_on(&gw());

        assert_ok!(claim(&p1(), &[(7, gw())]));
        // Share 562,500 plus 3/4 of 400,000.
        assert_eq!(free(&p1()), P_FUNDS + 562_500 + 300_000);
        assert_eq!(held_on(&gw()), gw_held - 562_500);
        assert_eq!(free(&Work::pot()), pot_before - 300_000);
        assert_ok!(claim(&p2(), &[(7, gw())]));
        assert_eq!(free(&p2()), P_FUNDS + 187_500 + 100_000);
        // The gateway's fee is released on its own account.
        let gw_free = free(&gw());
        assert_ok!(claim(&gw(), &[(7, gw())]));
        assert_eq!(free(&gw()), gw_free + 50_000);
        assert_eq!(held_on(&gw()), 0);
        // Nothing left to claim: the records are gone, the pot keeps its deposit.
        assert!(WorkOf::<Test>::get(p1(), 7).is_none());
        assert!(HeldFor::<Test>::get((p1(), 7, gw())).is_none());
        assert_eq!(free(&Work::pot()), 1);
        let l = Work::lifetime(&p1());
        assert_eq!((l.pending, l.verified, l.claimed), (0, 375_000, 862_500));
    });
}

// Scenario "重复领取".
#[test]
fn a_second_claim_pays_nothing() {
    ext().execute_with(|| {
        one_dollar_report();
        set_epoch(3);
        settle_epoch(2, 400_001);
        assert_ok!(claim(&p1(), &[(2, gw()), (2, gw())]));
        let after = free(&p1());
        assert_eq!(after, P_FUNDS + 562_500 + 300_000);
        assert_ok!(claim(&p1(), &[(2, gw())]));
        assert_eq!(free(&p1()), after);
    });
}

// Epochs without work: no division by zero, nothing paid.
#[test]
fn an_epoch_without_work_pays_nothing() {
    ext().execute_with(|| {
        settle_epoch(0, 0);
        assert_eq!(Work::epoch_work(0).market, Some(0));
        assert_ok!(claim(&p1(), &[(0, gw())]));
        assert_eq!(free(&p1()), P_FUNDS);
    });
}

// The pot never pays below its existential deposit: a first mint of exactly one deposit is kept.
#[test]
fn the_pot_keeps_its_deposit() {
    ext().execute_with(|| {
        one_dollar_report();
        set_epoch(3);
        settle_epoch(2, 1);
        assert_eq!(Work::epoch_work(2).market, Some(0));
        assert_ok!(claim(&p1(), &[(2, gw())]));
        assert_eq!(free(&Work::pot()), 1);
        assert_eq!(free(&p1()), P_FUNDS + 562_500);
    });
}

// Scenario "挑战期内被禁闭".
#[test]
fn a_jailed_providers_unsettled_work_is_voided() {
    ext().execute_with(|| {
        one_dollar_report();
        set_epoch(1);
        <Work as OnJail<AccountId32>>::on_jail(&p1());
        assert_eq!(<Work as WorkSource>::verified_work(2), (125_000, 0));
        assert!(WorkOf::<Test>::get(p1(), 2).unwrap().voided);
        assert_eq!(Work::lifetime(&p1()).pending, 0);
        set_epoch(3);
        settle_epoch(2, 100_001);
        let before_burned = burned();
        assert_ok!(claim(&p1(), &[(2, gw())]));
        // The share is burned instead of paid, no emission.
        assert_eq!(free(&p1()), P_FUNDS);
        assert_eq!(burned(), before_burned + 562_500);
        // p2 gets all the emission; the gateway its fee.
        assert_ok!(claim(&p2(), &[(2, gw())]));
        assert_eq!(free(&p2()), P_FUNDS + 187_500 + 100_000);
        let gw_free = free(&gw());
        assert_ok!(claim(&gw(), &[(2, gw())]));
        assert_eq!(free(&gw()), gw_free + 50_000);
    });
}

// Settled epochs are not affected by a later jail.
#[test]
fn a_jail_after_settlement_changes_nothing() {
    ext().execute_with(|| {
        one_dollar_report();
        set_epoch(3);
        settle_epoch(2, 400_001);
        <Work as OnJail<AccountId32>>::on_jail(&p1());
        assert!(!WorkOf::<Test>::get(p1(), 2).unwrap().voided);
        assert_ok!(claim(&p1(), &[(2, gw())]));
        assert_eq!(free(&p1()), P_FUNDS + 562_500 + 300_000);
    });
}

// A gateway that is also a provider keeps its fees when jailed as a provider.
#[test]
fn jailing_never_voids_gateway_fees() {
    ext().execute_with(|| {
        crate::mock::offer(&gw(), MODEL);
        deposit(&alice(), UNITS_PER_USD);
        assert_ok!(submit(
            &gw(),
            vec![entry(&gw(), 1_000_000)],
            vec![voucher(&alice(), 1_000_000)]
        ));
        <Work as OnJail<AccountId32>>::on_jail(&gw());
        set_epoch(3);
        settle_epoch(2, 0);
        let before = free(&gw());
        assert_ok!(claim(&gw(), &[(2, gw())]));
        // Fee 50,000 released; share 750,000 burned.
        assert_eq!(free(&gw()), before + 50_000);
        assert_eq!(held_on(&gw()), 0);
    });
}

// Scenario "查询与状态一致" and pruning after the retention period.
#[test]
fn queries_match_and_old_reports_are_pruned() {
    ext().execute_with(|| {
        one_dollar_report();
        let r = Work::report(0).unwrap();
        let shares: u128 = r.lines.iter().map(|l| l.share).sum();
        assert_eq!(r.burned + r.gateway_fee + shares, r.settled);
        assert_eq!(
            Work::held(&p1()),
            vec![(
                2,
                gw(),
                ac_primitives::market::Held {
                    shares: 562_500,
                    fees: 0
                }
            )]
        );
        assert_eq!(Work::work(&p1())[0].1.work, 375_000);
        assert_eq!(Work::params(), PARAMS);
        // Retention 3: report 0 (matures 2) is pruned by a submission in epoch 6.
        set_epoch(5);
        deposit(&bob(), UNITS_PER_USD);
        assert_ok!(submit(
            &gw(),
            vec![entry(&p1(), 100_000)],
            vec![voucher(&bob(), 100_000)]
        ));
        assert!(Reports::<Test>::get(0).is_some());
        set_epoch(6);
        assert_ok!(submit(
            &gw(),
            vec![entry(&p1(), 100_000)],
            vec![voucher(&bob(), 200_000)]
        ));
        assert!(Reports::<Test>::get(0).is_none());
        assert!(Reports::<Test>::get(1).is_some());
        // Pending payments outlive the report.
        assert!(HeldFor::<Test>::get((p1(), 2, gw())).is_some());
    });
}

// No call voids payments (scenario "无人能在本阶段触发", pallet side).
#[test]
fn only_submit_and_claim_are_calls() {
    let names = <crate::Call<Test> as frame_support::traits::GetCallName>::get_call_names();
    assert_eq!(names, ["submit_report", "claim"]);
}

mod properties {
    use super::*;
    use proptest::prelude::*;

    #[derive(Clone, Debug)]
    enum Op {
        Submit {
            dollars: u8,
            split: u8,
            epoch_step: u8,
        },
        Settle {
            market: u32,
        },
        Claim(u8),
        Jail(bool),
    }

    fn op() -> impl Strategy<Value = Op> {
        prop_oneof![
            (1u8..20, 0u8..=10, 0u8..2).prop_map(|(dollars, split, epoch_step)| Op::Submit {
                dollars,
                split,
                epoch_step
            }),
            (0u32..2_000_000).prop_map(|market| Op::Settle { market }),
            (0u8..3).prop_map(Op::Claim),
            any::<bool>().prop_map(Op::Jail),
        ]
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(64))]
        // Holds equal what is held for accounts; the pot covers unclaimed emission plus its
        // deposit; issuance changes only by what the test mints (emission) and burns.
        #[test]
        fn balances_and_holds_stay_consistent(ops in proptest::collection::vec(op(), 1..25)) {
            ext().execute_with(|| {
                deposit(&alice(), 90 * UNITS_PER_USD);
                let mut cumulative = 0u128;
                let mut epoch: EpochIndex = 0;
                let mut settled: EpochIndex = 0; // next epoch to settle
                let mut minted = 0u128;
                let issuance0 = Balances::total_issuance();
                for op in ops {
                    match op {
                        Op::Submit { dollars, split, epoch_step } => {
                            epoch += EpochIndex::from(epoch_step);
                            crate::mock::set_epoch(epoch);
                            let usd = u128::from(dollars) * 100_000;
                            let a = usd * u128::from(split) / 10;
                            let mut entries = vec![];
                            if a > 0 { entries.push(entry(&p1(), a)); }
                            if usd - a > 0 { entries.push(entry(&p2(), usd - a)); }
                            cumulative += usd;
                            let r = submit(&gw(), entries, vec![voucher(&alice(), cumulative)]);
                            if r.is_err() { cumulative -= usd; }
                        }
                        Op::Settle { market } => {
                            if settled < epoch {
                                let before = Balances::total_issuance();
                                settle_epoch(settled, u128::from(market));
                                minted += Balances::total_issuance() - before;
                                settled += 1;
                            }
                        }
                        Op::Claim(who) => {
                            let who = [p1(), p2(), gw()][usize::from(who)].clone();
                            let items: Vec<_> = (0..settled).map(|e| (e, gw())).collect();
                            let _ = claim(&who, &items[..items.len().min(64)]);
                        }
                        Op::Jail(first) => {
                            let who = if first { p1() } else { p2() };
                            <Work as OnJail<AccountId32>>::on_jail(&who);
                        }
                    }
                    // Holds on the gateway = everything recorded as held.
                    let recorded: u128 = HeldFor::<Test>::iter().map(|(_, h)| h.shares + h.fees).sum();
                    prop_assert_eq!(held_on(&gw()), recorded);
                    // The pot covers unclaimed emission plus one deposit once funded.
                    let pot = free(&Work::pot());
                    if pot > 0 {
                        prop_assert!(pot > crate::Unclaimed::<Test>::get());
                    }
                    // Issuance: only emission mints and burns change it.
                    prop_assert_eq!(Balances::total_issuance() + burned(), issuance0 + minted);
                }
                Ok(())
            })?;
        }
    }
}
