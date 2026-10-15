//! Unit tests of audit profiles, spot checks, evidence availability and locks
//! (m6-audit-hardening 5.1–5.8); each names the spec `market/audit` requirement and scenario it
//! covers.
// Test code: AGENT.md §5.3 permits unwrap and panics in tests.
#![allow(
    clippy::unwrap_used,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use frame_support::traits::fungible::{Inspect, InspectHold};
use frame_support::traits::{GetStorageVersion, OnRuntimeUpgrade};
use frame_support::{assert_noop, assert_ok};
use sp_runtime::AccountId32;

use ac_primitives::market::MicroUsd;
use ac_primitives::market::audit::{
    AdjustableParams, AuditMetric, AuditProfile, AuditorStatus, CURRENT_STATS, FailReason,
    PenaltyParams, ProfileSource, ReviewOutcome, ReviewVote, SprtEntry, SprtState, StatsConfig,
    VerdictOutcome, Vote, spot_checked,
};
use ac_primitives::market::traits::{DisputeLock, ModelCalibration};

use crate::mock::{
    ATC, Audit, Balances, GATEWAY, HONEST_STATS, INT8_STATS, MODEL, OTHER_MODEL, PROVIDER,
    PROVIDER2, ROUND, RuntimeOrigin, System, Test, acc, burned, current_round, new_test_ext,
    penalties, run_to, submission, submission_on, submission_with,
};
use crate::{
    Activity, Adjustable, DefaultProfile, Disputes, Error, Event, HoldReason, Locks,
    ModelSprtStates, OpenReviews, Penalty, RetainedVerdicts, ReviewActivity, ReviewRefs, Reviews,
    Seeds, StatsEnabled, UnavailableFindings, Verdicts,
};

const FAIL: VerdictOutcome = VerdictOutcome::Fail(FailReason::Threshold {
    chunk: 2,
    metric: AuditMetric::ExpMismatches,
});

fn assigned(provider: u8) -> Vec<AccountId32> {
    Audit::assignment(current_round(), &acc(provider))
}

fn has_event(e: Event<Test>) -> bool {
    System::events()
        .iter()
        .any(|r| r.event == crate::mock::RuntimeEvent::Audit(e.clone()))
}

fn next_round() {
    run_to((u64::from(current_round()) + 1) * ROUND + 1);
}

fn stake(who: &AccountId32) -> u128 {
    Balances::balance_on_hold(
        &crate::mock::RuntimeHoldReason::Audit(HoldReason::Stake),
        who,
    )
}

/// Request IDs distinct from each other.
struct Ids(u8);

impl Ids {
    fn next(&mut self) -> u8 {
        self.0 += 1;
        self.0
    }
}

/// Spot-checks half the verdicts judged by the thresholds.
fn spot_check_half() {
    assert_ok!(Audit::set_params(
        RuntimeOrigin::root(),
        AdjustableParams {
            spot_check_bps: 5_000,
            ..Adjustable::<Test>::get().unwrap()
        }
    ));
}

/// Honest passes on PROVIDER, round after round, until one is drawn for a spot check; checks
/// every round that exactly the verdicts the draw names open one. Returns the spot check.
fn open_spot_check(ids: &mut Ids) -> u64 {
    spot_check_half();
    for _ in 0..20 {
        let round = current_round();
        let mut submitted = Vec::new();
        for a in assigned(PROVIDER) {
            let id = ids.next();
            assert_ok!(Audit::submit_verdict(
                RuntimeOrigin::signed(a.clone()),
                submission(PROVIDER, id, VerdictOutcome::Pass)
            ));
            submitted.push((a, [id; 32]));
        }
        let before = crate::NextReview::<Test>::get();
        next_round();
        // Spec "裁决抽查", scenario "抽中与否可复算": the next round's seed and the request ID.
        let seed = Seeds::<Test>::get(current_round()).unwrap();
        let drawn: Vec<_> = submitted
            .iter()
            .filter(|(_, rid)| spot_checked(&seed, rid, 5_000))
            .collect();
        let opened = crate::NextReview::<Test>::get() - before;
        assert_eq!(opened as usize, drawn.len());
        for (k, (auditor, _)) in drawn.iter().enumerate() {
            let r = Reviews::<Test>::get(before + k as u64).unwrap();
            assert_eq!((&r.auditor, r.round), (auditor, round));
        }
        if opened > 0 {
            return before;
        }
    }
    panic!("no verdict drawn in 20 rounds");
}

/// Two failing verdicts on PROVIDER open a dispute; returns it.
fn open_dispute() -> u64 {
    for (i, who) in assigned(PROVIDER).iter().enumerate() {
        assert_ok!(Audit::submit_verdict(
            RuntimeOrigin::signed(who.clone()),
            submission(PROVIDER, 100 + i as u8, FAIL)
        ));
    }
    crate::OpenDispute::<Test>::get(acc(PROVIDER)).unwrap()
}

// ---- 按模型的审计档案 ----

#[test]
fn the_default_profile_covers_bf16_models() {
    // "按模型的审计档案" / "默认档案".
    new_test_ext(6).execute_with(|| {
        let default = AuditProfile {
            thresholds_version: 4,
            stats_version: CURRENT_STATS.version,
        };
        assert_eq!(
            Audit::profile(&MODEL),
            Some((default, ProfileSource::Default))
        );
        assert!(<Audit as ModelCalibration>::is_calibrated(&MODEL));
        // An unregistered model has no profile.
        let unknown = ac_primitives::market::ModelId([9; 32]);
        assert_eq!(Audit::profile(&unknown), None);
        assert!(!<Audit as ModelCalibration>::is_calibrated(&unknown));
    });
}

#[test]
fn verdicts_on_uncalibrated_models_are_refused() {
    // "按模型的审计档案" / "未校准模型的裁决被拒绝": no default and no own profile.
    new_test_ext(6).execute_with(|| {
        DefaultProfile::<Test>::kill();
        assert!(!<Audit as ModelCalibration>::is_calibrated(&MODEL));
        let a = assigned(PROVIDER)[0].clone();
        assert_noop!(
            Audit::submit_verdict(
                RuntimeOrigin::signed(a.clone()),
                submission(PROVIDER, 1, VerdictOutcome::Pass)
            ),
            Error::<Test>::Uncalibrated
        );
        // "设定档案": once the model has its profile, the same verdict is accepted.
        let p = AuditProfile {
            thresholds_version: 4,
            stats_version: CURRENT_STATS.version,
        };
        assert_ok!(Audit::set_audit_profile(RuntimeOrigin::root(), MODEL, p));
        assert_eq!(Audit::profile(&MODEL), Some((p, ProfileSource::Own)));
        assert_ok!(Audit::submit_verdict(
            RuntimeOrigin::signed(a),
            submission(PROVIDER, 1, VerdictOutcome::Pass)
        ));
    });
}

// ---- 统计判定 ----

#[test]
fn states_accumulate_per_model() {
    // "统计判定" / "按模型分别累计".
    new_test_ext(6).execute_with(|| {
        let a = assigned(PROVIDER)[0].clone();
        assert_ok!(Audit::submit_verdict(
            RuntimeOrigin::signed(a),
            submission_on(
                OTHER_MODEL,
                PROVIDER,
                1,
                VerdictOutcome::Pass,
                Some(INT8_STATS)
            )
        ));
        assert_eq!(
            Audit::sprt_state(&acc(PROVIDER), &OTHER_MODEL).cumulative,
            3_000
        );
        assert_eq!(
            Audit::sprt_state(&acc(PROVIDER), &MODEL),
            SprtState::default()
        );
        assert!(ModelSprtStates::<Test>::contains_key(
            acc(PROVIDER),
            OTHER_MODEL
        ));
    });
}

// ---- 裁决抽查 ----

#[test]
fn spot_checks_are_drawn_from_the_next_seed() {
    // "裁决抽查" / "抽中与否可复算", "抽查的复核人".
    new_test_ext(8).execute_with(|| {
        let id = open_spot_check(&mut Ids(0));
        let r = Reviews::<Test>::get(id).unwrap();
        assert_eq!(r.reviewers.len(), 3);
        for (reviewer, ballot) in &r.reviewers {
            assert!(*reviewer != r.auditor && *reviewer != acc(PROVIDER));
            assert!(*reviewer != acc(GATEWAY) && ballot.is_none());
        }
        assert!(OpenReviews::<Test>::contains_key(id));
        assert_eq!(ReviewActivity::<Test>::get(&r.auditor).spot_checked, 1);
        assert!(has_event(Event::SpotCheckOpened {
            id,
            provider: acc(PROVIDER),
            round: r.round,
            auditor: r.auditor.clone(),
        }));
        // Both verdicts of the round may have been drawn.
        let open = Audit::open_reviews(None, 10);
        assert_eq!(open.first(), Some(&id));
        assert_eq!(open.len() as u64, crate::NextReview::<Test>::get() - id);
    });
}

#[test]
fn no_spot_checks_at_a_zero_rate() {
    // "参数与护栏": the mock's rate is 0, so nothing is drawn.
    new_test_ext(8).execute_with(|| {
        let mut ids = Ids(0);
        for _ in 0..3 {
            for a in assigned(PROVIDER) {
                assert_ok!(Audit::submit_verdict(
                    RuntimeOrigin::signed(a),
                    submission(PROVIDER, ids.next(), VerdictOutcome::Pass)
                ));
            }
            next_round();
        }
        assert_eq!(crate::NextReview::<Test>::get(), 0);
    });
}

#[test]
fn an_upheld_spot_check_pays_and_punishes_nobody() {
    // "裁决抽查" / "抽查维持"; "审计资金池与支付" / "抽查的复核人获得支付".
    new_test_ext(8).execute_with(|| {
        let id = open_spot_check(&mut Ids(0));
        let r = Reviews::<Test>::get(id).unwrap();
        let auditor_stake = stake(&r.auditor);
        let payees: Vec<_> = r.reviewers.iter().take(2).map(|(x, _)| x.clone()).collect();
        let before: Vec<_> = payees.iter().map(Balances::balance).collect();
        for p in &payees {
            assert_ok!(Audit::vote_review(
                RuntimeOrigin::signed(p.clone()),
                id,
                ReviewVote::Uphold,
                true
            ));
        }
        let r = Reviews::<Test>::get(id).unwrap();
        assert_eq!(r.outcome, Some(ReviewOutcome::Upheld));
        assert!(!OpenReviews::<Test>::contains_key(id));
        assert_eq!(stake(&r.auditor), auditor_stake);
        for (p, b) in payees.iter().zip(before) {
            assert_eq!(Balances::balance(p), b + ATC / 20);
        }
        assert!(penalties().is_empty());
        assert_eq!(Audit::locks(&r.auditor), 0);
    });
}

#[test]
fn an_overturned_verdict_slashes_its_auditor() {
    // "裁决抽查" / "放水的通过被推翻"; "处罚" / "抽查推翻罚没审计员"; conservation.
    new_test_ext(8).execute_with(|| {
        let id = open_spot_check(&mut Ids(0));
        let r = Reviews::<Test>::get(id).unwrap();
        let issuance = Balances::total_issuance();
        let burned_before = burned();
        for (p, _) in r.reviewers.iter().take(2) {
            assert_ok!(Audit::vote_review(
                RuntimeOrigin::signed(p.clone()),
                id,
                ReviewVote::Overturn,
                true
            ));
        }
        assert_eq!(
            Reviews::<Test>::get(id).unwrap().outcome,
            Some(ReviewOutcome::Overturned)
        );
        let a = Audit::auditor(&r.auditor).unwrap();
        assert_eq!(a.status, AuditorStatus::Exiting);
        assert_eq!(stake(&r.auditor), 900 * ATC);
        assert_eq!(burned() - burned_before, 100 * ATC);
        // Two winning ballots were paid from the pot (a transfer, not a mint).
        assert_eq!(Balances::total_issuance(), issuance - 100 * ATC);
        assert_eq!(ReviewActivity::<Test>::get(&r.auditor).overturned, 1);
        // A spot check never punishes the provider.
        assert!(penalties().is_empty());
    });
}

#[test]
fn a_spot_check_without_evidence_is_only_a_light_penalty() {
    // "处罚": overturned because the evidence was unavailable: 1%, no exit.
    new_test_ext(8).execute_with(|| {
        let id = open_spot_check(&mut Ids(0));
        let r = Reviews::<Test>::get(id).unwrap();
        for (p, _) in r.reviewers.iter().take(2) {
            assert_ok!(Audit::vote_review(
                RuntimeOrigin::signed(p.clone()),
                id,
                ReviewVote::Overturn,
                false
            ));
        }
        let a = Audit::auditor(&r.auditor).unwrap();
        assert_eq!(a.status, AuditorStatus::Active);
        assert_eq!(stake(&r.auditor), 990 * ATC);
        assert_eq!(
            UnavailableFindings::<Test>::get(&r.auditor).unwrap().len(),
            1
        );
        assert!(has_event(Event::EvidenceUnavailable {
            who: r.auditor.clone(),
            full: false
        }));
    });
}

#[test]
fn spot_check_ballots_are_checked() {
    // "争议与升级复核" / "非复核人投票", for spot checks; votes after the deadline.
    new_test_ext(8).execute_with(|| {
        let id = open_spot_check(&mut Ids(0));
        let r = Reviews::<Test>::get(id).unwrap();
        let outsider = (1..=8)
            .map(acc)
            .find(|a| !r.reviewers.iter().any(|(x, _)| x == a))
            .unwrap();
        assert_noop!(
            Audit::vote_review(
                RuntimeOrigin::signed(outsider),
                id,
                ReviewVote::Uphold,
                true
            ),
            Error::<Test>::NotReviewer
        );
        assert_noop!(
            Audit::vote_review(
                RuntimeOrigin::signed(acc(1)),
                u64::MAX,
                ReviewVote::Uphold,
                true
            ),
            Error::<Test>::NoReview
        );
        let first = r.reviewers[0].0.clone();
        assert_ok!(Audit::vote_review(
            RuntimeOrigin::signed(first.clone()),
            id,
            ReviewVote::Uphold,
            true
        ));
        assert_noop!(
            Audit::vote_review(RuntimeOrigin::signed(first), id, ReviewVote::Uphold, true),
            Error::<Test>::NotReviewer
        );
        assert_noop!(
            Audit::close_spot_check(RuntimeOrigin::signed(acc(1)), id),
            Error::<Test>::DeadlineNotPassed
        );
        run_to(u64::from(r.deadline as u32) + 1);
        assert_noop!(
            Audit::vote_review(
                RuntimeOrigin::signed(r.reviewers[1].0.clone()),
                id,
                ReviewVote::Uphold,
                true
            ),
            Error::<Test>::DeadlinePassed
        );
        // "计数与查询" / "缺席投票": undecided, the two silent reviewers missed a vote.
        let missed = |x: &AccountId32| Activity::<Test>::get(x).missed_votes;
        let before: Vec<_> = r.reviewers.iter().map(|(x, _)| missed(x)).collect();
        assert_ok!(Audit::close_spot_check(RuntimeOrigin::signed(acc(1)), id));
        assert_eq!(
            Reviews::<Test>::get(id).unwrap().outcome,
            Some(ReviewOutcome::Undecided)
        );
        let after: Vec<_> = r.reviewers.iter().map(|(x, _)| missed(x)).collect();
        assert_eq!(after, vec![before[0], before[1] + 1, before[2] + 1]);
    });
}

// ---- 证据可得性与处罚 ----

#[test]
fn marks_must_name_cited_verdicts() {
    // "争议与升级复核" / "投票标明证据": a dispute citing 2 verdicts takes marks in bits 0–1.
    new_test_ext(8).execute_with(|| {
        let id = open_dispute();
        let r = Disputes::<Test>::get(id).unwrap().reviewers[0].0.clone();
        assert_noop!(
            Audit::vote_dispute(
                RuntimeOrigin::signed(r.clone()),
                acc(PROVIDER),
                id,
                Vote::Reject,
                0b100
            ),
            Error::<Test>::BadMarks
        );
        assert_ok!(Audit::vote_dispute(
            RuntimeOrigin::signed(r.clone()),
            acc(PROVIDER),
            id,
            Vote::Reject,
            0b10
        ));
        assert_eq!(crate::DisputeMarks::<Test>::get(id, &r), Some(0b10));
    });
}

#[test]
fn unavailable_evidence_is_a_light_penalty() {
    // "处罚" / "证据不可用只受轻罚": both deciding ballots mark accuser A (bit 0); accuser B's
    // evidence was refuted. Conservation: issuance falls by what is burned.
    new_test_ext(8).execute_with(|| {
        let id = open_dispute();
        let d = Disputes::<Test>::get(id).unwrap();
        let (a, b) = (d.accusers[0].auditor.clone(), d.accusers[1].auditor.clone());
        let issuance = Balances::total_issuance();
        let burned_before = burned();
        for (r, _) in d.reviewers.iter().take(2) {
            assert_ok!(Audit::vote_dispute(
                RuntimeOrigin::signed(r.clone()),
                acc(PROVIDER),
                id,
                Vote::Reject,
                0b01
            ));
        }
        assert_eq!(stake(&a), 990 * ATC);
        assert_eq!(Audit::auditor(&a).unwrap().status, AuditorStatus::Active);
        assert_eq!(stake(&b), 900 * ATC);
        assert_eq!(Audit::auditor(&b).unwrap().status, AuditorStatus::Exiting);
        assert_eq!(burned() - burned_before, 110 * ATC);
        assert_eq!(Balances::total_issuance(), issuance - 110 * ATC);
        assert_eq!(ReviewActivity::<Test>::get(&a).unavailable, 1);
        // The marks go with the dispute.
        assert_eq!(crate::DisputeMarks::<Test>::iter_prefix(id).count(), 0);
        assert!(penalties().is_empty());
    });
}

#[test]
fn the_third_finding_is_the_full_penalty() {
    // "处罚" / "多次证据不可用升级为完整处罚".
    new_test_ext(8).execute_with(|| {
        let id = open_dispute();
        let d = Disputes::<Test>::get(id).unwrap();
        let a = d.accusers[0].auditor.clone();
        let now = current_round();
        UnavailableFindings::<Test>::insert(&a, crate::Findings::truncate_from(vec![now, now]));
        for (r, _) in d.reviewers.iter().take(2) {
            assert_ok!(Audit::vote_dispute(
                RuntimeOrigin::signed(r.clone()),
                acc(PROVIDER),
                id,
                Vote::Reject,
                0b01
            ));
        }
        assert_eq!(stake(&a), 900 * ATC);
        assert_eq!(Audit::auditor(&a).unwrap().status, AuditorStatus::Exiting);
        assert!(has_event(Event::EvidenceUnavailable { who: a, full: true }));
    });
}

#[test]
fn a_mark_below_the_quorum_changes_nothing() {
    // One ballot of two marking A is not enough: A's evidence counts as obtained.
    new_test_ext(8).execute_with(|| {
        let id = open_dispute();
        let d = Disputes::<Test>::get(id).unwrap();
        let a = d.accusers[0].auditor.clone();
        for (k, (r, _)) in d.reviewers.iter().take(2).enumerate() {
            assert_ok!(Audit::vote_dispute(
                RuntimeOrigin::signed(r.clone()),
                acc(PROVIDER),
                id,
                Vote::Reject,
                if k == 0 { 0b01 } else { 0 }
            ));
        }
        assert_eq!(stake(&a), 900 * ATC);
        assert!(UnavailableFindings::<Test>::get(&a).is_none());
    });
}

// ---- 争议当事方的罚没锁 ----

#[test]
fn an_open_dispute_locks_its_accusers() {
    // "争议当事方的罚没锁" / "争议未关闭时不能取回"; the provider is locked too.
    new_test_ext(8).execute_with(|| {
        let accusers = assigned(PROVIDER);
        let a = accusers[0].clone();
        assert_ok!(Audit::submit_verdict(
            RuntimeOrigin::signed(a.clone()),
            submission(PROVIDER, 100, FAIL)
        ));
        assert_ok!(Audit::exit(RuntimeOrigin::signed(a.clone())));
        let exit_block = System::block_number();
        assert_ok!(Audit::submit_verdict(
            RuntimeOrigin::signed(accusers[1].clone()),
            submission(PROVIDER, 101, FAIL)
        ));
        let id = crate::OpenDispute::<Test>::get(acc(PROVIDER)).unwrap();
        assert_eq!(Locks::<Test>::get(&a), Some(1));
        assert!(<Audit as DisputeLock<AccountId32>>::is_locked(&acc(
            PROVIDER
        )));
        assert!(<Audit as DisputeLock<AccountId32>>::is_locked(&a));
        run_to(exit_block + 30);
        assert_noop!(
            Audit::withdraw_unbonded(RuntimeOrigin::signed(a.clone())),
            Error::<Test>::Locked
        );
        assert_ok!(Audit::close_dispute(
            RuntimeOrigin::signed(acc(1)),
            acc(PROVIDER),
            id
        ));
        assert!(Locks::<Test>::get(&a).is_none());
        assert!(!<Audit as DisputeLock<AccountId32>>::is_locked(&acc(
            PROVIDER
        )));
        assert_ok!(Audit::withdraw_unbonded(RuntimeOrigin::signed(a)));
    });
}

#[test]
fn an_open_spot_check_locks_its_auditor() {
    // "争议当事方的罚没锁" / "抽查未关闭时不能取回".
    new_test_ext(8).execute_with(|| {
        let id = open_spot_check(&mut Ids(0));
        let r = Reviews::<Test>::get(id).unwrap();
        assert_ok!(Audit::exit(RuntimeOrigin::signed(r.auditor.clone())));
        run_to(System::block_number() + 30);
        assert_noop!(
            Audit::withdraw_unbonded(RuntimeOrigin::signed(r.auditor.clone())),
            Error::<Test>::Locked
        );
        assert_ok!(Audit::close_spot_check(RuntimeOrigin::signed(acc(1)), id));
        assert_ok!(Audit::withdraw_unbonded(RuntimeOrigin::signed(r.auditor)));
    });
}

// ---- 计数与查询 ----

#[test]
fn a_spot_checked_verdict_outlives_its_retention() {
    // "计数与查询" / "被引用的裁决不被删除".
    new_test_ext(8).execute_with(|| {
        let id = open_spot_check(&mut Ids(0));
        let r = Reviews::<Test>::get(id).unwrap();
        // One reference per open spot check of the round (both verdicts may have been drawn).
        let open = Audit::open_reviews(None, 10);
        assert_eq!(
            ReviewRefs::<Test>::get(r.round, acc(PROVIDER)),
            Some(open.len() as u32)
        );
        // Past retention (3 rounds in the mock) and pruned.
        for _ in 0..6 {
            next_round();
        }
        assert!(Verdicts::<Test>::get(r.round, acc(PROVIDER)).is_empty());
        assert!(RetainedVerdicts::<Test>::contains_key(
            r.round,
            acc(PROVIDER)
        ));
        assert!(
            Audit::verdict_list(r.round, &acc(PROVIDER))
                .iter()
                .any(|v| v.auditor == r.auditor)
        );
        for open_id in open {
            assert_ok!(Audit::close_spot_check(
                RuntimeOrigin::signed(acc(1)),
                open_id
            ));
        }
        assert!(!RetainedVerdicts::<Test>::contains_key(
            r.round,
            acc(PROVIDER)
        ));
        assert!(!ReviewRefs::<Test>::contains_key(r.round, acc(PROVIDER)));
    });
}

// ---- 时间窗口的耦合约束 ----

#[test]
#[should_panic(expected = "AuditorUnbond")]
fn genesis_rejects_a_short_unbonding() {
    // "时间窗口的耦合约束" / "解绑期过短": rounds of 1,200, votes of 600, unbonding of 1,000.
    use sp_runtime::BuildStorage;
    let _ = crate::GenesisConfig::<Test> {
        params: crate::AuditGenesis {
            unbond_blocks: 1_000,
            ..crate::AuditGenesis::LIVE
        },
        _marker: core::marker::PhantomData,
    }
    .build_storage();
}

#[test]
#[should_panic(expected = "invalid audit genesis parameters")]
fn genesis_rejects_a_heavy_light_penalty() {
    // "参数与护栏": the light slash may not exceed the auditor slash.
    use sp_runtime::BuildStorage;
    let _ = crate::GenesisConfig::<Test> {
        params: crate::AuditGenesis {
            light_slash_percent: 11,
            ..crate::AuditGenesis::LIVE
        },
        _marker: core::marker::PhantomData,
    }
    .build_storage();
}

// ---- 迁移 ----

#[test]
fn migration_to_v2_moves_versions_into_profiles() {
    // m6-audit-hardening 5.8: the global versions become the default profile, the switch
    // `StatsEnabled`, the adjustable parameters gain the live spot-check rate, the light
    // penalty its live values, and per-provider states are emptied.
    use crate::migrations::old::{SprtStates, StatsSettings};
    use crate::migrations::v2::{MigrateToV2, OldAdjustable};
    use frame_support::storage::unhashed;
    use parity_scale_codec::Encode;
    new_test_ext(6).execute_with(|| {
        let key = Adjustable::<Test>::hashed_key();
        unhashed::put_raw(
            &key,
            &OldAdjustable {
                stake_usd: MicroUsd(1_000_000_000),
                payment_usd: MicroUsd(50_000),
                thresholds_version: 4,
            }
            .encode(),
        );
        StatsSettings::<Test>::put(StatsConfig {
            version: CURRENT_STATS.version,
            enabled: true,
        });
        let state = SprtState {
            cumulative: 3_000,
            entries: vec![SprtEntry {
                auditor: acc(1),
                round: 0,
                contribution: 3_000,
            }]
            .try_into()
            .unwrap(),
        };
        SprtStates::<Test>::insert(acc(PROVIDER2), (CURRENT_STATS.version, state));
        DefaultProfile::<Test>::kill();
        Penalty::<Test>::kill();
        StatsEnabled::<Test>::kill();
        frame_support::traits::StorageVersion::new(1).put::<Audit>();

        MigrateToV2::<Test>::on_runtime_upgrade();

        assert_eq!(Audit::on_chain_storage_version(), 2);
        assert_eq!(Adjustable::<Test>::get().unwrap().spot_check_bps, 500);
        assert_eq!(
            DefaultProfile::<Test>::get(),
            Some(AuditProfile {
                thresholds_version: 4,
                stats_version: CURRENT_STATS.version
            })
        );
        assert!(StatsEnabled::<Test>::get());
        assert_eq!(Penalty::<Test>::get(), Some(PenaltyParams::LIVE));
        assert!(!StatsSettings::<Test>::exists());
        assert_eq!(SprtStates::<Test>::iter().count(), 0);
        // Run again: the version check makes it a no-op.
        MigrateToV2::<Test>::on_runtime_upgrade();
        assert_eq!(Adjustable::<Test>::get().unwrap().spot_check_bps, 500);
    });
}

#[test]
fn honest_stats_still_count_against_the_state() {
    // A spot check reads no statistics: the state of an upheld verdict's provider is unchanged.
    new_test_ext(8).execute_with(|| {
        let a = assigned(PROVIDER)[0].clone();
        assert_ok!(Audit::submit_verdict(
            RuntimeOrigin::signed(a),
            submission_with(PROVIDER, 1, VerdictOutcome::Pass, Some(HONEST_STATS))
        ));
        assert_eq!(
            Audit::sprt_state(&acc(PROVIDER), &MODEL),
            SprtState::default()
        );
    });
}
