//! Unit tests of "每条裁决至多被裁定一次" (m6-dispute-judged-verdicts 2.1–2.4, I-030); each
//! names the spec `market/audit` requirement and scenario it covers.
// Test code: AGENT.md §5.3 permits unwrap and panics in tests.
#![allow(
    clippy::unwrap_used,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use frame_support::assert_ok;
use frame_support::traits::fungible::InspectHold;
use sp_runtime::AccountId32;

use ac_primitives::market::audit::{
    AdjustableParams, AuditMetric, DisputeOutcome, FailReason, Judgment, VerdictOutcome, Vote,
    spot_checked,
};

use crate::mock::{
    ATC, Audit, Balances, PROVIDER, ROUND, RuntimeOrigin, Test, acc, burned, current_round,
    new_test_ext, run_to, submission,
};
use crate::{
    Adjustable, Disputes, HoldReason, Judged, OpenDispute, RetainedVerdicts, Reviews, Seeds,
    Verdicts,
};

const FAIL: VerdictOutcome = VerdictOutcome::Fail(FailReason::Threshold {
    chunk: 2,
    metric: AuditMetric::ExpMismatches,
});

fn assigned() -> Vec<AccountId32> {
    Audit::assignment(current_round(), &acc(PROVIDER))
}

fn next_round() {
    run_to((u64::from(current_round()) + 1) * ROUND + 1);
}

fn held(who: &AccountId32) -> u128 {
    Balances::balance_on_hold(
        &crate::mock::RuntimeHoldReason::Audit(HoldReason::Stake),
        who,
    )
}

fn fail(who: &AccountId32, id: u8) {
    assert_ok!(Audit::submit_verdict(
        RuntimeOrigin::signed(who.clone()),
        submission(PROVIDER, id, FAIL)
    ));
}

/// The two auditors assigned to PROVIDER fail it; returns them and the dispute they open.
fn open_dispute(first_id: u8) -> (Vec<AccountId32>, u64) {
    let accusers = assigned();
    for (i, who) in accusers.iter().enumerate() {
        fail(who, first_id + i as u8);
    }
    (accusers, OpenDispute::<Test>::get(acc(PROVIDER)).unwrap())
}

fn vote(id: u64, vote: Vote) {
    let d = Disputes::<Test>::get(id).unwrap();
    for (r, _) in d.reviewers.iter().take(2) {
        assert_ok!(Audit::vote_dispute(
            RuntimeOrigin::signed(r.clone()),
            acc(PROVIDER),
            id,
            vote,
            0
        ));
    }
}

fn accused(id: u64) -> Vec<AccountId32> {
    Disputes::<Test>::get(id)
        .unwrap()
        .accusers
        .iter()
        .map(|a| a.auditor.clone())
        .collect()
}

fn spot_check_half() {
    assert_ok!(Audit::set_params(
        RuntimeOrigin::root(),
        AdjustableParams {
            spot_check_bps: 5_000,
            ..Adjustable::<Test>::get().unwrap()
        }
    ));
}

#[test]
fn a_rejected_dispute_s_verdicts_are_not_cited_again() {
    // "每条裁决至多被裁定一次" / "驳回后的裁决不再被引用", "新的两名提出者", "查询裁定情况";
    // "处罚": the refuted accusers are slashed once; conservation.
    new_test_ext(8).execute_with(|| {
        let r = current_round();
        let (first, id) = open_dispute(100);
        for a in &first {
            assert_eq!(
                Audit::judged(r, &acc(PROVIDER))
                    .iter()
                    .find(|(x, _)| x == a)
                    .map(|(_, j)| *j),
                Some(Judgment::Dispute(id))
            );
        }
        vote(id, Vote::Reject);
        assert_eq!(
            Disputes::<Test>::get(id).unwrap().outcome,
            Some(DisputeOutcome::Rejected)
        );
        let slashed: Vec<u128> = first.iter().map(held).collect();
        assert_eq!(slashed, vec![900 * ATC, 900 * ATC]);

        next_round();
        let next = assigned();
        assert!(next.iter().all(|a| !first.contains(a)), "{next:?}");
        let issuance = Balances::total_issuance();
        let burned_before = burned();
        fail(&next[0], 110);
        // The round-r failures are judged: one new failure opens nothing.
        assert!(OpenDispute::<Test>::get(acc(PROVIDER)).is_none());
        fail(&next[1], 111);
        let id2 = OpenDispute::<Test>::get(acc(PROVIDER)).unwrap();
        assert_eq!(accused(id2), next);
        assert_eq!(
            Audit::judged(current_round(), &acc(PROVIDER)),
            vec![
                (next[0].clone(), Judgment::Dispute(id2)),
                (next[1].clone(), Judgment::Dispute(id2)),
            ]
        );
        vote(id2, Vote::Reject);
        // The first accusers lose nothing more; the new ones are slashed once each.
        assert_eq!(first.iter().map(held).collect::<Vec<_>>(), slashed);
        assert_eq!(burned() - burned_before, 200 * ATC);
        assert_eq!(Balances::total_issuance(), issuance - 200 * ATC);
    });
}

#[test]
fn an_undecided_dispute_s_verdicts_are_not_cited_again() {
    // "每条裁决至多被裁定一次" / "未决后的裁决不再被引用".
    new_test_ext(8).execute_with(|| {
        let (_, id) = open_dispute(100);
        let d = Disputes::<Test>::get(id).unwrap();
        run_to(d.deadline + 1);
        assert_ok!(Audit::close_dispute(
            RuntimeOrigin::signed(acc(1)),
            acc(PROVIDER),
            id
        ));
        assert_eq!(
            Disputes::<Test>::get(id).unwrap().outcome,
            Some(DisputeOutcome::Undecided)
        );
        next_round();
        fail(&assigned()[0], 110);
        assert!(OpenDispute::<Test>::get(acc(PROVIDER)).is_none());
    });
}

#[test]
fn a_failure_filed_during_a_dispute_is_cited_after_it() {
    // "争议与升级复核" / "争议期间的不通过在争议结束后可被引用".
    new_test_ext(8).execute_with(|| {
        // Open the dispute late in its round, so that it is still open in the next one.
        run_to(u64::from(current_round()) * ROUND + ROUND - 1);
        let (first, id) = open_dispute(100);
        next_round();
        let next = assigned();
        assert!(next.iter().all(|a| !first.contains(a)), "{next:?}");
        fail(&next[0], 110);
        // Recorded, not cited: the provider already has an open dispute.
        assert_eq!(OpenDispute::<Test>::get(acc(PROVIDER)), Some(id));
        assert!(Audit::judged(current_round(), &acc(PROVIDER)).is_empty());
        vote(id, Vote::Reject);
        assert!(OpenDispute::<Test>::get(acc(PROVIDER)).is_none());
        fail(&next[1], 111);
        let id2 = OpenDispute::<Test>::get(acc(PROVIDER)).unwrap();
        assert_eq!(accused(id2), next);
    });
}

#[test]
fn a_dispute_that_did_not_open_judges_nothing() {
    // "每条裁决至多被裁定一次" / "未能开启的争议不裁定": 4 auditors leave too few reviewers.
    new_test_ext(4).execute_with(|| {
        let r = current_round();
        let first = assigned();
        for (i, who) in first.iter().enumerate() {
            fail(who, 100 + i as u8);
        }
        assert!(OpenDispute::<Test>::get(acc(PROVIDER)).is_none());
        assert!(Judged::<Test>::get(r, acc(PROVIDER)).is_empty());
        // Two more auditors join; in the next round a third one fails the provider.
        for n in [5u8, 6] {
            assert_ok!(Audit::register(RuntimeOrigin::signed(acc(n)), 1_000 * ATC));
        }
        next_round();
        let third = assigned()
            .into_iter()
            .find(|a| !first.contains(a))
            .expect("an auditor other than the first two is assigned");
        fail(&third, 110);
        let id = OpenDispute::<Test>::get(acc(PROVIDER)).unwrap();
        let mut expected = first.clone();
        expected.push(third);
        assert_eq!(accused(id), expected);
    });
}

#[test]
fn a_cited_verdict_is_not_spot_checked() {
    // "每条裁决至多被裁定一次" / "被争议引用的裁决不被抽查"; "裁决抽查" / "抽中与否可复算"
    // with the judgments.
    new_test_ext(8).execute_with(|| {
        spot_check_half();
        let mut id = 100u8;
        for _ in 0..20 {
            let r = current_round();
            let (accusers, d) = open_dispute(id);
            let rids = [[id; 32], [id + 1; 32]];
            id += 2;
            // Close it undecided so that the next round can open another one.
            run_to(Disputes::<Test>::get(d).unwrap().deadline + 1);
            assert_ok!(Audit::close_dispute(
                RuntimeOrigin::signed(acc(1)),
                acc(PROVIDER),
                d
            ));
            let before = crate::NextReview::<Test>::get();
            next_round();
            // Nothing of round r is reviewed, drawn or not.
            assert_eq!(crate::NextReview::<Test>::get(), before);
            assert_eq!(
                Audit::judged(r, &acc(PROVIDER))
                    .iter()
                    .map(|(a, _)| a.clone())
                    .collect::<Vec<_>>(),
                accusers
            );
            let seed = Seeds::<Test>::get(current_round()).unwrap();
            if rids.iter().any(|rid| spot_checked(&seed, rid, 5_000)) {
                return;
            }
        }
        panic!("no cited verdict would have been drawn in 20 rounds");
    });
}

#[test]
fn a_spot_checked_failure_does_not_count_for_a_dispute() {
    // "每条裁决至多被裁定一次" / "被抽查的裁决不计入争议"; "裁决抽查": the spot check opened in
    // the first block of the round comes before the failure submitted in it.
    new_test_ext(8).execute_with(|| {
        spot_check_half();
        let mut id = 100u8;
        for _ in 0..20 {
            let r = current_round();
            let a = assigned()[0].clone();
            fail(&a, id);
            id += 1;
            next_round();
            let judged = Audit::judged(r, &acc(PROVIDER));
            if let Some((_, Judgment::SpotCheck(review))) = judged.first() {
                assert_eq!(Reviews::<Test>::get(*review).unwrap().auditor, a);
                let b = assigned()
                    .into_iter()
                    .find(|x| *x != a)
                    .expect("another auditor is assigned");
                fail(&b, id);
                assert!(OpenDispute::<Test>::get(acc(PROVIDER)).is_none());
                return;
            }
            // Not drawn: let the failure leave the two-round window before the next attempt.
            next_round();
        }
        panic!("no failure drawn in 20 rounds");
    });
}

#[test]
fn judgments_are_pruned_and_retained_with_their_verdicts() {
    // "计数与查询" / "裁定记录随裁决删除", "被引用的裁决不被删除".
    new_test_ext(8).execute_with(|| {
        let r = current_round();
        let (_, id) = open_dispute(100);
        // Past retention (3 rounds in the mock) while the dispute is still open: kept.
        for _ in 0..6 {
            next_round();
        }
        assert!(Verdicts::<Test>::get(r, acc(PROVIDER)).is_empty());
        assert!(RetainedVerdicts::<Test>::contains_key(r, acc(PROVIDER)));
        assert_eq!(Audit::judged(r, &acc(PROVIDER)).len(), 2);
        // Closing the dispute releases the verdicts and their judgments together.
        assert_ok!(Audit::close_dispute(
            RuntimeOrigin::signed(acc(1)),
            acc(PROVIDER),
            id
        ));
        assert!(!RetainedVerdicts::<Test>::contains_key(r, acc(PROVIDER)));
        assert!(!Judged::<Test>::contains_key(r, acc(PROVIDER)));

        // A judged round nothing refers to any more goes with the pruning.
        let r2 = current_round();
        let (_, id2) = open_dispute(110);
        let d = Disputes::<Test>::get(id2).unwrap();
        run_to(d.deadline + 1);
        assert_ok!(Audit::close_dispute(
            RuntimeOrigin::signed(acc(1)),
            acc(PROVIDER),
            id2
        ));
        assert!(Judged::<Test>::contains_key(r2, acc(PROVIDER)));
        for _ in 0..6 {
            next_round();
        }
        assert!(Verdicts::<Test>::get(r2, acc(PROVIDER)).is_empty());
        assert!(!RetainedVerdicts::<Test>::contains_key(r2, acc(PROVIDER)));
        assert!(!Judged::<Test>::contains_key(r2, acc(PROVIDER)));
    });
}
