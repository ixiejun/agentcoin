//! Unit tests; each names the spec `market/audit` requirement and scenario it covers.
// Test code: AGENT.md §5.3 permits unwrap and panics in tests.
#![allow(
    clippy::unwrap_used,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use frame_support::traits::fungible::{Inspect, InspectHold};
use frame_support::{assert_noop, assert_ok};
use sp_core::H256;
use sp_runtime::{AccountId32, Perbill};

use ac_primitives::market::MicroUsd;
use ac_primitives::market::audit::{
    AdjustableParams, AuditMetric, AuditorStatus, DisputeKind, DisputeOutcome, Draw, FailReason,
    InconclusiveReason, Pool, VerdictOutcome, Vote, sample,
};

use crate::mock::{
    ATC, Audit, Balances, GATEWAY, HONEST_STATS, PROVIDER, PROVIDER2, ROUND, RuntimeOrigin, System,
    Test, acc, burned, current_round, jailed, new_test_ext, penalties, receipt, run_to,
    set_randomness, set_rate, submission,
};
use crate::{
    Activity, AuditGenesis, Disputes, Error, Event, HoldReason, OpenDispute, ProviderStats,
    Rosters, Seeds, UsedRequests, Verdicts,
};
use ac_primitives::market::audit::{CURRENT_STATS, VerdictStats};

const FAIL: VerdictOutcome = VerdictOutcome::Fail(FailReason::Threshold {
    chunk: 2,
    metric: AuditMetric::ExpMismatches,
});

fn assigned(provider: u8) -> Vec<AccountId32> {
    Audit::assignment(current_round(), &acc(provider))
}

fn auditors_not_assigned(provider: u8, count: u8) -> Vec<AccountId32> {
    let a = assigned(provider);
    (1..=count).map(acc).filter(|x| !a.contains(x)).collect()
}

fn has_event(e: Event<Test>) -> bool {
    System::events()
        .iter()
        .any(|r| r.event == crate::mock::RuntimeEvent::Audit(e.clone()))
}

fn payment() -> u128 {
    // $0.05 at 1 ATC per dollar.
    ATC / 20
}

// ---- 审计员登记与质押 ----

#[test]
fn registration_below_threshold_fails() {
    // "审计员登记与质押" / "质押不足".
    new_test_ext(0).execute_with(|| {
        assert_noop!(
            Audit::register(RuntimeOrigin::signed(acc(1)), 999 * ATC),
            Error::<Test>::BelowThreshold
        );
        assert_ok!(Audit::register(RuntimeOrigin::signed(acc(1)), 1_000 * ATC));
        assert_eq!(
            Balances::balance_on_hold(&HoldReason::Stake.into(), &acc(1)),
            1_000 * ATC
        );
    });
}

#[test]
fn providers_and_gateways_cannot_be_auditors() {
    // "审计员登记与质押" / "提供者不能兼任审计员".
    new_test_ext(0).execute_with(|| {
        for n in [PROVIDER, GATEWAY] {
            assert_noop!(
                Audit::register(RuntimeOrigin::signed(acc(n)), 1_000 * ATC),
                Error::<Test>::MarketParticipant
            );
        }
    });
}

#[test]
fn stake_cannot_be_withdrawn_while_unbonding() {
    // "审计员登记与质押" / "解绑期内不能取回".
    new_test_ext(1).execute_with(|| {
        assert_ok!(Audit::bond_extra(RuntimeOrigin::signed(acc(1)), 10 * ATC));
        assert_noop!(
            Audit::unbond(RuntimeOrigin::signed(acc(1)), 11 * ATC),
            Error::<Test>::BelowThreshold
        );
        assert_ok!(Audit::exit(RuntimeOrigin::signed(acc(1))));
        let exit_block = System::block_number();
        assert_noop!(
            Audit::withdraw_unbonded(RuntimeOrigin::signed(acc(1))),
            Error::<Test>::NothingToWithdraw
        );
        run_to(exit_block + 19);
        assert_noop!(
            Audit::withdraw_unbonded(RuntimeOrigin::signed(acc(1))),
            Error::<Test>::NothingToWithdraw
        );
        run_to(exit_block + 20);
        let before = Balances::balance(&acc(1));
        assert_ok!(Audit::withdraw_unbonded(RuntimeOrigin::signed(acc(1))));
        assert_eq!(Balances::balance(&acc(1)), before + 1_010 * ATC);
        assert!(Audit::auditor(&acc(1)).is_none());
    });
}

#[test]
fn auditors_below_a_new_threshold_leave_the_roster() {
    // "审计员登记与质押": below the threshold after a rate change, off later rosters.
    new_test_ext(3).execute_with(|| {
        set_rate(Some(2 * ATC));
        run_to(2 * ROUND + 1);
        assert!(Rosters::<Test>::get(2).unwrap().is_empty());
        assert_ok!(Audit::bond_extra(
            RuntimeOrigin::signed(acc(2)),
            1_000 * ATC
        ));
        run_to(3 * ROUND + 1);
        assert_eq!(Rosters::<Test>::get(3).unwrap().to_vec(), vec![acc(2)]);
    });
}

// ---- 审计轮次与抽样 ----

#[test]
fn assignment_can_be_recomputed() {
    // "审计轮次与抽样" / "复算抽样".
    new_test_ext(10).execute_with(|| {
        let r = current_round();
        let roster = Rosters::<Test>::get(r).unwrap();
        let mut sorted = roster.to_vec();
        sorted.sort();
        assert_eq!(roster.to_vec(), sorted);
        let seed = Seeds::<Test>::get(r).unwrap();
        let expected = sample(
            Pool {
                roster: &roster,
                excluded: &[],
            },
            &seed,
            &acc(PROVIDER),
            Draw::Assign,
            2,
        );
        assert_eq!(assigned(PROVIDER), expected);
        assert_eq!(expected.len(), 2);
    });
}

#[test]
fn roster_is_fixed_within_a_round() {
    // "审计轮次与抽样" / "名单在轮内固定".
    new_test_ext(4).execute_with(|| {
        let before = assigned(PROVIDER);
        assert_ok!(Audit::register(RuntimeOrigin::signed(acc(30)), 1_000 * ATC));
        assert_eq!(Rosters::<Test>::get(1).unwrap().len(), 4);
        assert_eq!(assigned(PROVIDER), before);
        run_to(2 * ROUND + 1);
        assert!(Rosters::<Test>::get(2).unwrap().contains(&acc(30)));
    });
}

#[test]
fn no_randomness_means_no_assignments() {
    // "审计轮次与抽样" / "没有随机数".
    new_test_ext(4).execute_with(|| {
        set_randomness(None);
        run_to(2 * ROUND + 1);
        assert!(Seeds::<Test>::get(2).is_none());
        assert!(assigned(PROVIDER).is_empty());
        for n in 1..=4 {
            assert_noop!(
                Audit::submit_verdict(
                    RuntimeOrigin::signed(acc(n)),
                    submission(PROVIDER, n, VerdictOutcome::Pass)
                ),
                Error::<Test>::NotAssigned
            );
        }
    });
}

// ---- 裁决 ----

#[test]
fn assigned_auditor_submits_a_pass() {
    // "裁决" / "被分配的审计员提交通过"; "审计证据与承诺" / "通过的裁决也有证据承诺";
    // "审计资金池与支付" / "裁决获得支付".
    new_test_ext(6).execute_with(|| {
        let a = assigned(PROVIDER)[0].clone();
        let (pot, mine) = (Balances::balance(&Audit::pot()), Balances::balance(&a));
        let issuance = Balances::total_issuance();
        assert_ok!(Audit::submit_verdict(
            RuntimeOrigin::signed(a.clone()),
            submission(PROVIDER, 1, VerdictOutcome::Pass)
        ));
        assert_eq!(ProviderStats::<Test>::get(acc(PROVIDER)).pass, 1);
        assert_eq!(Activity::<Test>::get(&a).verdicts, 1);
        assert_eq!(Balances::balance(&a), mine + payment());
        assert_eq!(Balances::balance(&Audit::pot()), pot - payment());
        assert_eq!(Balances::total_issuance(), issuance);
        let v = &Verdicts::<Test>::get(current_round(), acc(PROVIDER))[0];
        assert_eq!(v.auditor, a);
        assert_eq!(v.evidence, Some([1; 32]));
        assert_eq!(
            v.stats,
            Some(VerdictStats {
                version: CURRENT_STATS.version,
                stats: HONEST_STATS
            })
        );
    });
}

#[test]
fn unassigned_auditor_is_refused() {
    // "裁决" / "未被分配的审计员".
    new_test_ext(6).execute_with(|| {
        let other = auditors_not_assigned(PROVIDER, 6)[0].clone();
        assert_noop!(
            Audit::submit_verdict(
                RuntimeOrigin::signed(other),
                submission(PROVIDER, 1, VerdictOutcome::Pass)
            ),
            Error::<Test>::NotAssigned
        );
    });
}

#[test]
fn a_receipt_is_used_once() {
    // "裁决" / "重复使用收据".
    new_test_ext(6).execute_with(|| {
        let a = assigned(PROVIDER);
        assert_ok!(Audit::submit_verdict(
            RuntimeOrigin::signed(a[0].clone()),
            submission(PROVIDER, 1, VerdictOutcome::Pass)
        ));
        assert!(UsedRequests::<Test>::contains_key([1u8; 32]));
        assert_noop!(
            Audit::submit_verdict(
                RuntimeOrigin::signed(a[1].clone()),
                submission(PROVIDER, 1, VerdictOutcome::Pass)
            ),
            Error::<Test>::RequestUsed
        );
        assert_noop!(
            Audit::submit_verdict(
                RuntimeOrigin::signed(a[0].clone()),
                submission(PROVIDER, 2, VerdictOutcome::Pass)
            ),
            Error::<Test>::AlreadySubmitted
        );
    });
}

#[test]
fn bad_receipt_signature_is_refused() {
    // "裁决" / "收据签名无效".
    new_test_ext(6).execute_with(|| {
        let a = assigned(PROVIDER)[0].clone();
        let mut s = submission(PROVIDER, 1, VerdictOutcome::Pass);
        let mut sig = s.receipt.provider_sig.as_bytes().to_vec();
        sig[10] ^= 1;
        s.receipt.provider_sig =
            ac_crypto::PqSignature::new(s.receipt.provider_sig.alg(), &sig).unwrap();
        assert_noop!(
            Audit::submit_verdict(RuntimeOrigin::signed(a.clone()), s),
            Error::<Test>::ReceiptSignature
        );
        let mut s = submission(PROVIDER, 1, VerdictOutcome::Pass);
        s.receipt = receipt(PROVIDER2, GATEWAY, 1);
        assert_noop!(
            Audit::submit_verdict(RuntimeOrigin::signed(a), s),
            Error::<Test>::ReceiptProvider
        );
    });
}

#[test]
fn outdated_thresholds_version_is_refused() {
    // "裁决" / "阈值版本过期".
    new_test_ext(6).execute_with(|| {
        assert_ok!(Audit::set_params(
            RuntimeOrigin::root(),
            AdjustableParams {
                stake_usd: MicroUsd(1_000_000_000),
                payment_usd: MicroUsd(50_000),
                thresholds_version: 5,
            }
        ));
        let a = assigned(PROVIDER)[0].clone();
        assert_noop!(
            Audit::submit_verdict(
                RuntimeOrigin::signed(a),
                submission(PROVIDER, 1, VerdictOutcome::Pass)
            ),
            Error::<Test>::WrongThresholdsVersion
        );
    });
}

#[test]
fn evidence_goes_with_failures_and_judged_verdicts() {
    // "审计证据与承诺": failures and verdicts judged by the thresholds carry a commitment,
    // inconclusive ones do not.
    new_test_ext(6).execute_with(|| {
        let a = assigned(PROVIDER)[0].clone();
        let mut s = submission(PROVIDER, 1, FAIL);
        s.evidence = None;
        assert_noop!(
            Audit::submit_verdict(RuntimeOrigin::signed(a.clone()), s),
            Error::<Test>::BadEvidence
        );
        let mut s = submission(PROVIDER, 1, VerdictOutcome::Pass);
        s.evidence = None;
        assert_noop!(
            Audit::submit_verdict(RuntimeOrigin::signed(a.clone()), s),
            Error::<Test>::BadEvidence
        );
        let mut s = submission(
            PROVIDER,
            1,
            VerdictOutcome::Inconclusive(InconclusiveReason::Tokens),
        );
        s.evidence = Some([1; 32]);
        assert_noop!(
            Audit::submit_verdict(RuntimeOrigin::signed(a), s),
            Error::<Test>::BadEvidence
        );
    });
}

#[test]
fn verdicts_only_in_their_round() {
    new_test_ext(6).execute_with(|| {
        let a = assigned(PROVIDER)[0].clone();
        let mut s = submission(PROVIDER, 1, VerdictOutcome::Pass);
        s.round -= 1;
        assert_noop!(
            Audit::submit_verdict(RuntimeOrigin::signed(a), s),
            Error::<Test>::WrongRound
        );
    });
}

#[test]
fn inconclusive_is_counted_not_punished() {
    // "计数与查询" / "无法判定计数".
    new_test_ext(6).execute_with(|| {
        let a = assigned(PROVIDER)[0].clone();
        assert_ok!(Audit::submit_verdict(
            RuntimeOrigin::signed(a),
            submission(
                PROVIDER,
                1,
                VerdictOutcome::Inconclusive(InconclusiveReason::Tokens)
            )
        ));
        assert_eq!(ProviderStats::<Test>::get(acc(PROVIDER)).inconclusive, 1);
        assert!(penalties().is_empty());
        assert!(!jailed(&acc(PROVIDER)));
    });
}

#[test]
fn assigned_but_not_submitted_is_queryable() {
    // "计数与查询" / "被分配而未提交".
    new_test_ext(6).execute_with(|| {
        let a = assigned(PROVIDER);
        assert_ok!(Audit::submit_verdict(
            RuntimeOrigin::signed(a[0].clone()),
            submission(PROVIDER, 1, VerdictOutcome::Pass)
        ));
        let candidates = [acc(PROVIDER), acc(PROVIDER2)];
        let first = Audit::assigned_to(current_round(), &a[0], &candidates);
        assert!(first.contains(&(acc(PROVIDER), true)));
        let second = Audit::assigned_to(current_round(), &a[1], &candidates);
        assert!(second.contains(&(acc(PROVIDER), false)));
    });
}

// ---- 争议与升级复核、处罚 ----

/// Two assigned auditors fail PROVIDER in the current round; returns the dispute ID.
fn open_dispute() -> u64 {
    let a = assigned(PROVIDER);
    for (i, who) in a.iter().enumerate() {
        assert_ok!(Audit::submit_verdict(
            RuntimeOrigin::signed(who.clone()),
            submission(PROVIDER, 100 + i as u8, FAIL)
        ));
    }
    OpenDispute::<Test>::get(acc(PROVIDER)).unwrap()
}

#[test]
fn two_failures_open_a_dispute() {
    // "争议与升级复核" / "两名审计员判为不通过".
    new_test_ext(8).execute_with(|| {
        let accusers = assigned(PROVIDER);
        let id = open_dispute();
        let d = Disputes::<Test>::get(id).unwrap();
        assert_eq!(d.reviewers.len(), 3);
        for (r, v) in &d.reviewers {
            assert!(!accusers.contains(r));
            assert!(*r != acc(PROVIDER) && *r != acc(GATEWAY));
            assert!(v.is_none());
        }
        let accused: Vec<_> = d.accusers.iter().map(|a| a.auditor.clone()).collect();
        assert_eq!(accused, accusers);
        assert!(has_event(Event::DisputeOpened {
            id,
            provider: acc(PROVIDER),
            kind: DisputeKind::Fail,
        }));
        assert_eq!(d.kind, DisputeKind::Fail);
    });
}

#[test]
fn one_auditor_failing_twice_opens_nothing() {
    // "争议与升级复核" / "一名审计员两次不通过".
    new_test_ext(8).execute_with(|| {
        let mut done = 0;
        let mut target: Option<AccountId32> = None;
        for round in 1..30u64 {
            if round > 1 {
                run_to(round * ROUND + 1);
            }
            let a = assigned(PROVIDER);
            let who = match &target {
                None => a[0].clone(),
                Some(t) if a.contains(t) => t.clone(),
                Some(_) => continue,
            };
            assert_ok!(Audit::submit_verdict(
                RuntimeOrigin::signed(who.clone()),
                submission(PROVIDER, round as u8, FAIL)
            ));
            target = Some(who);
            done += 1;
            if done == 2 {
                break;
            }
        }
        assert_eq!(done, 2);
        assert!(OpenDispute::<Test>::get(acc(PROVIDER)).is_none());
    });
}

#[test]
fn non_reviewers_cannot_vote() {
    // "争议与升级复核" / "非复核人投票".
    new_test_ext(8).execute_with(|| {
        let id = open_dispute();
        let d = Disputes::<Test>::get(id).unwrap();
        let outsider = (1..=8)
            .map(acc)
            .find(|a| !d.reviewers.iter().any(|(r, _)| r == a))
            .unwrap();
        assert_noop!(
            Audit::vote(
                RuntimeOrigin::signed(outsider),
                acc(PROVIDER),
                id,
                Vote::Confirm
            ),
            Error::<Test>::NotReviewer
        );
        let r = d.reviewers[0].0.clone();
        assert_ok!(Audit::vote(
            RuntimeOrigin::signed(r.clone()),
            acc(PROVIDER),
            id,
            Vote::Confirm
        ));
        assert_noop!(
            Audit::vote(RuntimeOrigin::signed(r), acc(PROVIDER), id, Vote::Reject),
            Error::<Test>::NotReviewer
        );
    });
}

#[test]
fn undecided_after_the_deadline() {
    // "争议与升级复核" / "期满未决"; "计数与查询" / "缺席投票".
    new_test_ext(8).execute_with(|| {
        let id = open_dispute();
        let d = Disputes::<Test>::get(id).unwrap();
        assert_ok!(Audit::vote(
            RuntimeOrigin::signed(d.reviewers[0].0.clone()),
            acc(PROVIDER),
            id,
            Vote::Confirm
        ));
        assert_noop!(
            Audit::close_dispute(RuntimeOrigin::signed(acc(1)), acc(PROVIDER), id),
            Error::<Test>::DeadlineNotPassed
        );
        run_to(d.deadline + 1);
        assert_noop!(
            Audit::vote(
                RuntimeOrigin::signed(d.reviewers[1].0.clone()),
                acc(PROVIDER),
                id,
                Vote::Confirm
            ),
            Error::<Test>::DeadlinePassed
        );
        let before = Balances::balance(&d.reviewers[0].0);
        assert_ok!(Audit::close_dispute(
            RuntimeOrigin::signed(acc(1)),
            acc(PROVIDER),
            id
        ));
        let closed = Disputes::<Test>::get(id).unwrap();
        assert_eq!(closed.outcome, Some(DisputeOutcome::Undecided));
        assert!(OpenDispute::<Test>::get(acc(PROVIDER)).is_none());
        assert!(penalties().is_empty() && !jailed(&acc(PROVIDER)));
        assert_eq!(Balances::balance(&d.reviewers[0].0), before);
        assert_eq!(Activity::<Test>::get(&d.reviewers[1].0).missed_votes, 1);
        assert_eq!(Activity::<Test>::get(&d.reviewers[0].0).missed_votes, 0);
    });
}

#[test]
fn too_few_eligible_auditors_open_nothing() {
    // "争议与升级复核" / "合格审计员不足": 2 accusers + 2 others < 3 reviewers needed... the
    // test parameters need 3 reviewers, so 4 auditors leave only 2 eligible.
    new_test_ext(4).execute_with(|| {
        let a = assigned(PROVIDER);
        for (i, who) in a.iter().enumerate() {
            assert_ok!(Audit::submit_verdict(
                RuntimeOrigin::signed(who.clone()),
                submission(PROVIDER, 100 + i as u8, FAIL)
            ));
        }
        assert!(OpenDispute::<Test>::get(acc(PROVIDER)).is_none());
        assert_eq!(
            Verdicts::<Test>::get(current_round(), acc(PROVIDER)).len(),
            2
        );
        assert!(has_event(Event::DisputeNotOpened {
            provider: acc(PROVIDER)
        }));
    });
}

#[test]
fn confirmation_slashes_and_jails_the_provider() {
    // "处罚" / "确认后罚没并禁闭"; "审计资金池与支付" / "投票失败一方不获支付".
    new_test_ext(8).execute_with(|| {
        let id = open_dispute();
        let d = Disputes::<Test>::get(id).unwrap();
        let reviewers: Vec<_> = d.reviewers.iter().map(|(r, _)| r.clone()).collect();
        let before: Vec<_> = reviewers.iter().map(Balances::balance).collect();
        assert_ok!(Audit::vote(
            RuntimeOrigin::signed(reviewers[0].clone()),
            acc(PROVIDER),
            id,
            Vote::Reject
        ));
        assert_ok!(Audit::vote(
            RuntimeOrigin::signed(reviewers[1].clone()),
            acc(PROVIDER),
            id,
            Vote::Confirm
        ));
        assert!(penalties().is_empty());
        assert_ok!(Audit::vote(
            RuntimeOrigin::signed(reviewers[2].clone()),
            acc(PROVIDER),
            id,
            Vote::Confirm
        ));
        assert_eq!(
            penalties(),
            vec![(acc(PROVIDER), Perbill::from_percent(10))]
        );
        assert!(jailed(&acc(PROVIDER)));
        assert_eq!(ProviderStats::<Test>::get(acc(PROVIDER)).confirmed, 1);
        assert_eq!(
            Disputes::<Test>::get(id).unwrap().outcome,
            Some(DisputeOutcome::Confirmed)
        );
        assert_eq!(Balances::balance(&reviewers[0]), before[0]);
        assert_eq!(Balances::balance(&reviewers[1]), before[1] + payment());
        assert_eq!(Balances::balance(&reviewers[2]), before[2] + payment());
    });
}

#[test]
fn rejection_slashes_the_accusers() {
    // "处罚" / "驳回后罚没提出者": total conservation, burned = issuance decrease.
    new_test_ext(8).execute_with(|| {
        let accusers = assigned(PROVIDER);
        let id = open_dispute();
        let d = Disputes::<Test>::get(id).unwrap();
        let issuance = Balances::total_issuance();
        for (r, _) in d.reviewers.iter().take(2) {
            assert_ok!(Audit::vote(
                RuntimeOrigin::signed(r.clone()),
                acc(PROVIDER),
                id,
                Vote::Reject
            ));
        }
        assert!(penalties().is_empty() && !jailed(&acc(PROVIDER)));
        for a in &accusers {
            let rec = Audit::auditor(a).unwrap();
            assert_eq!(rec.status, AuditorStatus::Exiting);
            assert_eq!(rec.stake, 0);
            assert_eq!(
                Balances::balance_on_hold(&HoldReason::Stake.into(), a),
                900 * ATC
            );
        }
        assert_eq!(burned(), 200 * ATC);
        // Two winning votes were paid from the pot (a transfer, not a mint).
        assert_eq!(Balances::total_issuance(), issuance - 200 * ATC);
        assert!(!Rosters::<Test>::get(current_round()).unwrap().is_empty());
        run_to((current_round() as u64 + 1) * ROUND + 1);
        let roster = Rosters::<Test>::get(current_round()).unwrap();
        for a in &accusers {
            assert!(!roster.contains(a));
        }
    });
}

#[test]
fn administration_cannot_punish() {
    // "处罚" / "管理权限不能处罚": no call slashes, jails or decides; the call list is fixed.
    let names = <crate::Call<Test> as frame_support::traits::GetCallName>::get_call_names();
    assert_eq!(
        names,
        [
            "register",
            "bond_extra",
            "unbond",
            "exit",
            "withdraw_unbonded",
            "submit_verdict",
            "vote",
            "close_dispute",
            "set_params",
            // m6-auditor-agent: an auditor's own evidence endpoint; moves no funds.
            "set_endpoint",
            // m6-audit-sprt: the statistical judgment's version and switch; decides nothing.
            "set_stats_config"
        ]
    );
    new_test_ext(8).execute_with(|| {
        let id = open_dispute();
        assert_noop!(
            Audit::vote(RuntimeOrigin::root(), acc(PROVIDER), id, Vote::Confirm),
            sp_runtime::DispatchError::BadOrigin
        );
    });
}

// ---- 审计资金池与支付 ----

#[test]
fn empty_pot_skips_payment() {
    // "审计资金池与支付" / "资金池不足".
    new_test_ext(6).execute_with(|| {
        let pot = Audit::pot();
        let all = Balances::balance(&pot);
        assert_ok!(
            <Balances as frame_support::traits::fungible::Mutate<_>>::transfer(
                &pot,
                &acc(40),
                all - 1,
                frame_support::traits::tokens::Preservation::Expendable
            )
        );
        let a = assigned(PROVIDER)[0].clone();
        let before = Balances::balance(&a);
        assert_ok!(Audit::submit_verdict(
            RuntimeOrigin::signed(a.clone()),
            submission(PROVIDER, 1, VerdictOutcome::Pass)
        ));
        assert_eq!(Balances::balance(&a), before);
        assert!(has_event(Event::PaymentSkipped { who: a }));
        assert_eq!(ProviderStats::<Test>::get(acc(PROVIDER)).pass, 1);
    });
}

// ---- 参数与护栏 ----

#[test]
fn adjustments_stay_within_guardrails() {
    // "参数与护栏" / "护栏外的支付金额", "阈值版本不能回退".
    new_test_ext(0).execute_with(|| {
        let ok = AdjustableParams {
            stake_usd: MicroUsd(1_000_000_000),
            payment_usd: MicroUsd(50_000),
            thresholds_version: 5,
        };
        assert_noop!(
            Audit::set_params(RuntimeOrigin::signed(acc(1)), ok),
            sp_runtime::DispatchError::BadOrigin
        );
        assert_noop!(
            Audit::set_params(
                RuntimeOrigin::root(),
                AdjustableParams {
                    payment_usd: MicroUsd(2_000_000),
                    ..ok
                }
            ),
            Error::<Test>::OutOfBounds
        );
        assert_ok!(Audit::set_params(RuntimeOrigin::root(), ok));
        assert_noop!(
            Audit::set_params(
                RuntimeOrigin::root(),
                AdjustableParams {
                    thresholds_version: 4,
                    ..ok
                }
            ),
            Error::<Test>::OutOfBounds
        );
    });
}

#[test]
#[should_panic(expected = "invalid audit genesis parameters")]
fn genesis_rejects_broken_guardrails() {
    // "参数与护栏" / "创世参数不满足护栏": N = 5, Q = 2.
    use sp_runtime::BuildStorage;
    let _ = crate::GenesisConfig::<Test> {
        params: AuditGenesis {
            reviewers: 5,
            quorum: 2,
            ..AuditGenesis::LIVE
        },
        _marker: core::marker::PhantomData,
    }
    .build_storage();
}

// ---- 保留与清理 ----

#[test]
fn old_rounds_are_pruned() {
    // Design D11: verdicts and used request IDs go after the retention period, a few per block.
    new_test_ext(6).execute_with(|| {
        let r = current_round();
        for (i, who) in assigned(PROVIDER).iter().enumerate() {
            assert_ok!(Audit::submit_verdict(
                RuntimeOrigin::signed(who.clone()),
                submission(PROVIDER, 1 + i as u8, VerdictOutcome::Pass)
            ));
        }
        for (i, who) in assigned(PROVIDER2).iter().enumerate() {
            assert_ok!(Audit::submit_verdict(
                RuntimeOrigin::signed(who.clone()),
                submission(PROVIDER2, 10 + i as u8, VerdictOutcome::Pass)
            ));
        }
        // Retention is 3 rounds in the mock: round r is pruned from round r + 4 on.
        run_to((u64::from(r) + 4) * ROUND);
        assert!(UsedRequests::<Test>::contains_key([1u8; 32]));
        run_to((u64::from(r) + 4) * ROUND + 3);
        assert!(!UsedRequests::<Test>::contains_key([1u8; 32]));
        assert!(Verdicts::<Test>::get(r, acc(PROVIDER)).is_empty());
        assert!(Verdicts::<Test>::get(r, acc(PROVIDER2)).is_empty());
        let _ = H256::zero();
    });
}

// ---- 审计员证据地址与未关闭争议列表（m6-auditor-agent）----

fn endpoint(url: &[u8]) -> crate::AuditorEndpoint {
    (
        frame_support::BoundedVec::truncate_from(url.to_vec()),
        ac_crypto::KemPublicKey::new(ac_crypto::KemAlg::XWing, &[7; 1216]).unwrap(),
    )
}

#[test]
fn auditors_set_and_clear_their_endpoint() {
    // "审计员证据地址" / "审计员登记证据地址", "非审计员不能设置".
    new_test_ext(1).execute_with(|| {
        let e = endpoint(b"http://auditor.example:8500");
        assert_ok!(Audit::set_endpoint(
            RuntimeOrigin::signed(acc(1)),
            Some(e.clone())
        ));
        assert_eq!(Audit::endpoint(&acc(1)), Some(e.clone()));
        assert!(has_event(Event::EndpointSet {
            who: acc(1),
            set: true
        }));
        assert_noop!(
            Audit::set_endpoint(RuntimeOrigin::signed(acc(PROVIDER)), Some(e.clone())),
            Error::<Test>::NotAuditor
        );
        // The administration cannot set or clear it for an auditor.
        assert_noop!(
            Audit::set_endpoint(RuntimeOrigin::root(), Some(e)),
            sp_runtime::DispatchError::BadOrigin
        );
        assert_ok!(Audit::set_endpoint(RuntimeOrigin::signed(acc(1)), None));
        assert_eq!(Audit::endpoint(&acc(1)), None);
    });
}

#[test]
fn bad_endpoints_and_keys_are_refused() {
    // "审计员证据地址" / "非 X-Wing 公钥被拒绝": the record stays as it was.
    new_test_ext(1).execute_with(|| {
        let good = endpoint(b"http://auditor.example:8500");
        assert_ok!(Audit::set_endpoint(
            RuntimeOrigin::signed(acc(1)),
            Some(good.clone())
        ));
        assert_noop!(
            Audit::set_endpoint(RuntimeOrigin::signed(acc(1)), Some(endpoint(b""))),
            Error::<Test>::InvalidEndpoint
        );
        assert_noop!(
            Audit::set_endpoint(RuntimeOrigin::signed(acc(1)), Some(endpoint(&[0xff, 0xfe]))),
            Error::<Test>::InvalidEndpoint
        );
        // The reserved ML-KEM-1024 AlgId is not even constructible today; if `ac-crypto` ever
        // allows it, the call must still refuse it.
        if let Ok(other) = ac_crypto::KemPublicKey::new(ac_crypto::KemAlg::MlKem1024, &[7; 1568]) {
            assert_noop!(
                Audit::set_endpoint(RuntimeOrigin::signed(acc(1)), Some((good.0.clone(), other))),
                Error::<Test>::UnsupportedKem
            );
        }
        assert_eq!(Audit::endpoint(&acc(1)), Some(good));
    });
}

#[test]
fn the_endpoint_goes_with_the_auditor() {
    // "审计员证据地址" / "退出后删除".
    new_test_ext(1).execute_with(|| {
        assert_ok!(Audit::set_endpoint(
            RuntimeOrigin::signed(acc(1)),
            Some(endpoint(b"http://a"))
        ));
        assert_ok!(Audit::exit(RuntimeOrigin::signed(acc(1))));
        // An exiting auditor may still be an accuser, so it keeps (and may change) its endpoint.
        assert_ok!(Audit::set_endpoint(
            RuntimeOrigin::signed(acc(1)),
            Some(endpoint(b"http://b"))
        ));
        run_to(System::block_number() + 20);
        assert_ok!(Audit::withdraw_unbonded(RuntimeOrigin::signed(acc(1))));
        assert_eq!(Audit::endpoint(&acc(1)), None);
    });
}

#[test]
fn open_disputes_are_listed_until_decided() {
    // "未关闭争议列表" / "列出未关闭争议".
    new_test_ext(8).execute_with(|| {
        let id = open_dispute();
        // A second provider's open dispute (written directly: the list only reads the index).
        OpenDispute::<Test>::insert(acc(PROVIDER2), 77);
        let mut expected = vec![(acc(PROVIDER), id), (acc(PROVIDER2), 77)];
        expected.sort();
        assert_eq!(Audit::open_disputes(None, 10), expected);
        let d = Disputes::<Test>::get(id).unwrap();
        for (r, _) in d.reviewers.iter().take(2) {
            assert_ok!(Audit::vote(
                RuntimeOrigin::signed(r.clone()),
                acc(PROVIDER),
                id,
                Vote::Confirm
            ));
        }
        assert_eq!(Audit::open_disputes(None, 10), vec![(acc(PROVIDER2), 77)]);
    });
}

#[test]
fn open_disputes_page() {
    // "未关闭争议列表" / "分页".
    new_test_ext(1).execute_with(|| {
        for (i, p) in [40u8, 41, 42].into_iter().enumerate() {
            OpenDispute::<Test>::insert(acc(p), i as u64);
        }
        let first = Audit::open_disputes(None, 2);
        assert_eq!(first.len(), 2);
        let rest = Audit::open_disputes(Some(&first[1].0), 2);
        let mut all: Vec<_> = first.into_iter().chain(rest).collect();
        assert_eq!(all.len(), 3);
        all.dedup();
        assert_eq!(all.len(), 3);
        assert!(Audit::open_disputes(None, 10_000).len() == 3);
    });
}
