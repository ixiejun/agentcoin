//! Unit tests of "争议投票与关闭按实际工作计费" (m6-audit-vote-weights 2.1, I-027): votes and
//! closings reserve the worst case and are charged for the work done; each test names the spec
//! `market/audit` scenario it covers.
// Test code: AGENT.md §5.3 permits unwrap and panics in tests.
#![allow(
    clippy::unwrap_used,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use frame_support::dispatch::GetDispatchInfo;
use frame_support::weights::Weight;
use sp_core::H256;
use sp_runtime::AccountId32;

use ac_primitives::market::audit::{
    AuditMetric, CURRENT_STATS, DisputeOutcome, FailReason, MAX_DISPUTE_ACCUSERS, SprtEntry,
    SprtState, VerdictOutcome, VerdictRecord, VerdictStats, Vote,
};

use crate::mock::{
    Audit, Balances, INT8_STATS, MODEL, PROVIDER, RuntimeOrigin, Test, acc, burned, current_round,
    new_test_ext, run_to, submission, submission_with,
};
use crate::weights::WeightInfo;
use crate::{Call, Disputes, ModelSprtStates, OpenDispute, RetainedVerdicts, VerdictList};

const FAIL: VerdictOutcome = VerdictOutcome::Fail(FailReason::Threshold {
    chunk: 2,
    metric: AuditMetric::ExpMismatches,
});

type W = ();

fn assigned() -> Vec<AccountId32> {
    Audit::assignment(current_round(), &acc(PROVIDER))
}

/// The two assigned auditors fail the provider: a failing dispute citing 2 verdicts.
fn failing_dispute() -> u64 {
    for (i, who) in assigned().iter().enumerate() {
        Audit::submit_verdict(
            RuntimeOrigin::signed(who.clone()),
            submission(PROVIDER, 100 + i as u8, FAIL),
        )
        .unwrap();
    }
    OpenDispute::<Test>::get(acc(PROVIDER)).unwrap()
}

/// A statistical dispute citing MAX_DISPUTE_ACCUSERS verdicts: a state of
/// `MAX_DISPUTE_ACCUSERS - 1` entries of unregistered auditors just below the bound, crossed by
/// one int8-looking pass of an assigned auditor.
fn largest_dispute() -> u64 {
    let entries = MAX_DISPUTE_ACCUSERS - 1;
    let total = (CURRENT_STATS.bound / i64::from(MAX_DISPUTE_ACCUSERS) - 3)
        * i64::from(MAX_DISPUTE_ACCUSERS);
    let per = total / i64::from(entries);
    let mut state = SprtState::default();
    for i in 0..entries {
        let mut raw = [0xee; 32];
        raw[..4].copy_from_slice(&i.to_le_bytes());
        let auditor = AccountId32::new(raw);
        state
            .entries
            .try_push(SprtEntry {
                auditor: auditor.clone(),
                round: i,
                contribution: i32::try_from(per).unwrap(),
            })
            .unwrap();
        RetainedVerdicts::<Test>::insert(
            i,
            acc(PROVIDER),
            VerdictList::truncate_from(vec![VerdictRecord {
                auditor,
                outcome: VerdictOutcome::Pass,
                thresholds_version: 4,
                evidence: Some([1; 32]),
                receipt_hash: H256([2; 32]),
                request_id: [3; 32],
                gateway: acc(250),
                stats: Some(VerdictStats {
                    version: CURRENT_STATS.version,
                    stats: INT8_STATS,
                }),
            }]),
        );
    }
    state.cumulative = per * i64::from(entries);
    ModelSprtStates::<Test>::insert(acc(PROVIDER), MODEL, (CURRENT_STATS.version, state));
    Audit::submit_verdict(
        RuntimeOrigin::signed(assigned()[0].clone()),
        submission_with(PROVIDER, 100, VerdictOutcome::Pass, Some(INT8_STATS)),
    )
    .unwrap();
    let id = OpenDispute::<Test>::get(acc(PROVIDER)).unwrap();
    assert_eq!(
        Disputes::<Test>::get(id).unwrap().accusers.len() as u32,
        MAX_DISPUTE_ACCUSERS
    );
    id
}

fn vote(id: u64, k: usize, vote: Vote) -> Option<Weight> {
    let reviewer = Disputes::<Test>::get(id).unwrap().reviewers[k].0.clone();
    Audit::vote_dispute(RuntimeOrigin::signed(reviewer), acc(PROVIDER), id, vote, 0)
        .unwrap()
        .actual_weight
}

fn reserved_vote() -> Weight {
    Call::<Test>::vote_dispute {
        provider: acc(PROVIDER),
        id: 0,
        vote: Vote::Reject,
        unavailable: 0,
    }
    .get_dispatch_info()
    .call_weight
}

#[test]
fn a_vote_that_decides_nothing_pays_for_itself() {
    // "争议投票与关闭按实际工作计费" / "不决定结果的投票只付一票的费用".
    new_test_ext(8).execute_with(|| {
        let id = largest_dispute();
        let charged = vote(id, 0, Vote::Reject).unwrap();
        assert_eq!(charged, W::vote_dispute_open());
        assert_eq!(OpenDispute::<Test>::get(acc(PROVIDER)), Some(id));
        // Reserved for the worst deciding vote, refunded down to the vote itself.
        assert!(reserved_vote().all_gte(W::vote_dispute_close(MAX_DISPUTE_ACCUSERS)));
        assert!(charged.ref_time() < reserved_vote().ref_time());
    });
    // Whatever the dispute's size.
    new_test_ext(8).execute_with(|| {
        let id = failing_dispute();
        assert_eq!(vote(id, 0, Vote::Reject), Some(W::vote_dispute_open()));
    });
}

#[test]
fn a_deciding_vote_pays_by_the_verdicts_cited() {
    // "争议投票与关闭按实际工作计费" / "决定结果的投票按引用的裁决数计费".
    new_test_ext(8).execute_with(|| {
        let id = failing_dispute();
        vote(id, 0, Vote::Reject);
        let charged = vote(id, 1, Vote::Reject).unwrap();
        assert_eq!(
            Disputes::<Test>::get(id).unwrap().outcome,
            Some(DisputeOutcome::Rejected)
        );
        assert_eq!(charged, W::vote_dispute_close(2));
        assert!(charged.ref_time() < W::vote_dispute_close(MAX_DISPUTE_ACCUSERS).ref_time());
        assert!(charged.ref_time() > W::vote_dispute_open().ref_time());
    });
}

#[test]
fn closing_a_large_dispute_pays_for_its_work() {
    // "争议投票与关闭按实际工作计费" / "期满关闭大争议按其工作计费".
    new_test_ext(8).execute_with(|| {
        let reserved = Call::<Test>::close_dispute {
            provider: acc(PROVIDER),
            id: 0,
        }
        .get_dispatch_info()
        .call_weight;
        assert!(reserved.all_gte(W::close_dispute(MAX_DISPUTE_ACCUSERS)));

        let id = largest_dispute();
        run_to(Disputes::<Test>::get(id).unwrap().deadline + 1);
        let charged = Audit::close_dispute(RuntimeOrigin::signed(acc(1)), acc(PROVIDER), id)
            .unwrap()
            .actual_weight;
        assert_eq!(charged, Some(W::close_dispute(MAX_DISPUTE_ACCUSERS)));
        assert_eq!(
            Disputes::<Test>::get(id).unwrap().outcome,
            Some(DisputeOutcome::Undecided)
        );
    });
    // A small one is charged less.
    new_test_ext(8).execute_with(|| {
        let id = failing_dispute();
        run_to(Disputes::<Test>::get(id).unwrap().deadline + 1);
        let charged = Audit::close_dispute(RuntimeOrigin::signed(acc(1)), acc(PROVIDER), id)
            .unwrap()
            .actual_weight
            .unwrap();
        assert_eq!(charged, W::close_dispute(2));
        assert!(charged.ref_time() < W::close_dispute(MAX_DISPUTE_ACCUSERS).ref_time());
    });
}

#[test]
fn charging_does_not_change_the_outcome() {
    // "争议投票与关闭按实际工作计费" / "计费方式不改变结果": the second rejecting ballot
    // slashes both accusers in the same call; burned = issuance decrease.
    new_test_ext(8).execute_with(|| {
        let accusers = assigned();
        let id = failing_dispute();
        let issuance = Balances::total_issuance();
        vote(id, 0, Vote::Reject);
        vote(id, 1, Vote::Reject);
        assert!(OpenDispute::<Test>::get(acc(PROVIDER)).is_none());
        for a in &accusers {
            assert_eq!(Audit::auditor(a).unwrap().stake, 0);
        }
        assert_eq!(burned(), 200 * crate::mock::ATC);
        assert_eq!(
            Balances::total_issuance(),
            issuance - 200 * crate::mock::ATC
        );
    });
}
