//! Unit tests of the statistical judgment (m6-audit-sprt 3.1–3.5); each names the spec
//! `market/audit` requirement and scenario it covers.
// Test code: AGENT.md §5.3 permits unwrap and panics in tests.
#![allow(
    clippy::unwrap_used,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use frame_support::traits::fungible::InspectHold;
use frame_support::traits::{GetStorageVersion, OnRuntimeUpgrade};
use frame_support::{assert_noop, assert_ok};
use sp_runtime::AccountId32;

use ac_primitives::market::audit::{
    AuditStats, AuditorStatus, CURRENT_STATS, DisputeKind, DisputeOutcome, FailReason,
    InconclusiveReason, SprtEntry, SprtState, StatsConfig, VerdictOutcome, Vote,
};

use crate::mock::{
    ATC, Audit, GATEWAY, HONEST_STATS, INT8_STATS, PROVIDER, ROUND, RuntimeOrigin, System, Test,
    acc, burned, current_round, jailed, new_test_ext, penalties, run_to, submission,
    submission_with,
};
use crate::{
    Disputes, Error, Event, HoldReason, OpenDispute, RetainedVerdicts, SprtStates, StaleClearing,
    StatsSettings, Verdicts,
};

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

/// Request IDs for the submissions of these tests, distinct from each other.
struct Ids(u8);

impl Ids {
    fn next(&mut self) -> u8 {
        self.0 += 1;
        self.0
    }
}

/// Every auditor assigned to PROVIDER this round submits a pass with `stats`.
fn submit_round(ids: &mut Ids, stats: AuditStats) {
    for a in assigned(PROVIDER) {
        assert_ok!(Audit::submit_verdict(
            RuntimeOrigin::signed(a),
            submission_with(PROVIDER, ids.next(), VerdictOutcome::Pass, Some(stats))
        ));
    }
}

/// Int8-looking passes, verdict by verdict, until the statistical state opens a dispute (or
/// `max` rounds pass); returns the dispute.
fn cross(ids: &mut Ids, max: u32) -> Option<u64> {
    for _ in 0..max {
        for a in assigned(PROVIDER) {
            assert_ok!(Audit::submit_verdict(
                RuntimeOrigin::signed(a),
                submission_with(PROVIDER, ids.next(), VerdictOutcome::Pass, Some(INT8_STATS))
            ));
            if let Some(id) = OpenDispute::<Test>::get(acc(PROVIDER)) {
                return Some(id);
            }
        }
        next_round();
    }
    None
}

fn set_stats(version: u16, enabled: bool) {
    assert_ok!(Audit::set_stats_config(
        RuntimeOrigin::root(),
        StatsConfig { version, enabled }
    ));
}

// ---- 裁决 ----

#[test]
fn a_pass_without_statistics_is_refused() {
    // "裁决" / "通过的裁决缺少统计量".
    new_test_ext(6).execute_with(|| {
        let a = assigned(PROVIDER)[0].clone();
        assert_noop!(
            Audit::submit_verdict(
                RuntimeOrigin::signed(a),
                submission_with(PROVIDER, 1, VerdictOutcome::Pass, None)
            ),
            Error::<Test>::BadStats
        );
    });
}

#[test]
fn an_inconclusive_verdict_carries_no_statistics() {
    // "裁决" / "无法判定不带统计量"; a failure without a proof carries none either.
    new_test_ext(6).execute_with(|| {
        let a = assigned(PROVIDER)[0].clone();
        let tokens = VerdictOutcome::Inconclusive(InconclusiveReason::Tokens);
        assert_noop!(
            Audit::submit_verdict(
                RuntimeOrigin::signed(a.clone()),
                submission_with(PROVIDER, 1, tokens, Some(HONEST_STATS))
            ),
            Error::<Test>::BadStats
        );
        let no_proof = VerdictOutcome::Fail(FailReason::NoProof);
        assert_noop!(
            Audit::submit_verdict(
                RuntimeOrigin::signed(a.clone()),
                submission_with(PROVIDER, 1, no_proof, Some(HONEST_STATS))
            ),
            Error::<Test>::BadStats
        );
        assert_ok!(Audit::submit_verdict(
            RuntimeOrigin::signed(a),
            submission(PROVIDER, 1, tokens)
        ));
        // "计数与查询" / "无法判定计数": the state does not change.
        assert_eq!(Audit::sprt_state(&acc(PROVIDER)), SprtState::default());
    });
}

#[test]
fn another_statistics_version_is_refused() {
    // "裁决" / "统计参数版本不符".
    new_test_ext(6).execute_with(|| {
        let a = assigned(PROVIDER)[0].clone();
        let mut s = submission(PROVIDER, 1, VerdictOutcome::Pass);
        s.stats.as_mut().unwrap().version = CURRENT_STATS.version + 1;
        assert_noop!(
            Audit::submit_verdict(RuntimeOrigin::signed(a), s),
            Error::<Test>::WrongStatsVersion
        );
    });
}

#[test]
fn statistics_must_be_possible() {
    // "裁决": statistics out of the allowed range (no prompt token; a decode mean without
    // decode chunks) are refused.
    new_test_ext(6).execute_with(|| {
        let a = assigned(PROVIDER)[0].clone();
        for bad in [
            AuditStats {
                prompt_tokens: 0,
                ..HONEST_STATS
            },
            AuditStats {
                decode_chunks: 0,
                ..HONEST_STATS
            },
        ] {
            assert_noop!(
                Audit::submit_verdict(
                    RuntimeOrigin::signed(a.clone()),
                    submission_with(PROVIDER, 1, VerdictOutcome::Pass, Some(bad))
                ),
                Error::<Test>::BadStats
            );
        }
    });
}

// ---- 统计判定 ----

#[test]
fn a_verdict_adds_its_contribution() {
    // "统计判定" / "贡献按查表计算" with the published version 1: an int8-looking verdict adds
    // the clamped +3.0 nats; "查询累计值" / "复算累计值".
    new_test_ext(6).execute_with(|| {
        let a = assigned(PROVIDER);
        assert_ok!(Audit::submit_verdict(
            RuntimeOrigin::signed(a[0].clone()),
            submission_with(PROVIDER, 1, VerdictOutcome::Pass, Some(INT8_STATS))
        ));
        let s = Audit::sprt_state(&acc(PROVIDER));
        assert_eq!(s.cumulative, 3_000);
        assert_eq!(
            s.entries.to_vec(),
            vec![SprtEntry {
                auditor: a[0].clone(),
                round: current_round(),
                contribution: 3_000
            }]
        );
        // An honest-looking one counts −0.5 nats.
        assert_ok!(Audit::submit_verdict(
            RuntimeOrigin::signed(a[1].clone()),
            submission_with(PROVIDER, 2, VerdictOutcome::Pass, Some(HONEST_STATS))
        ));
        let s = Audit::sprt_state(&acc(PROVIDER));
        assert_eq!(s.cumulative, 2_500);
        assert_eq!(s, SprtState::replay(&CURRENT_STATS, s.entries.clone()));
    });
}

#[test]
fn an_honest_provider_keeps_no_state() {
    // "统计判定" / "累计值不低于零".
    new_test_ext(6).execute_with(|| {
        let mut ids = Ids(0);
        for _ in 0..3 {
            submit_round(&mut ids, HONEST_STATS);
            next_round();
        }
        assert!(!SprtStates::<Test>::contains_key(acc(PROVIDER)));
        assert_eq!(Audit::sprt_state(&acc(PROVIDER)), SprtState::default());
    });
}

#[test]
fn crossing_the_bound_opens_a_statistical_dispute() {
    // "统计判定" / "越界开启统计争议"; "争议与升级复核" / "统计争议的复核人".
    new_test_ext(20).execute_with(|| {
        let mut ids = Ids(0);
        let id = cross(&mut ids, 20).expect("no statistical dispute");
        let d = Disputes::<Test>::get(id).unwrap();
        assert_eq!(
            d.kind,
            DisputeKind::Statistical {
                stats_version: CURRENT_STATS.version
            }
        );
        assert!(d.accusers.len() >= 9, "{} verdicts", d.accusers.len());
        let accusers: Vec<AccountId32> = d.accusers.iter().map(|a| a.auditor.clone()).collect();
        assert!(
            accusers
                .iter()
                .collect::<std::collections::BTreeSet<_>>()
                .len()
                >= 3,
            "at least three auditors"
        );
        for (r, _) in &d.reviewers {
            assert!(!accusers.contains(r));
            assert!(*r != acc(PROVIDER) && *r != acc(GATEWAY));
        }
        // Every accuser names a verdict on the provider.
        for a in &d.accusers {
            assert!(
                Audit::verdict_list(a.round, &acc(PROVIDER))
                    .iter()
                    .any(|v| v.auditor == a.auditor && v.stats.is_some())
            );
        }
        assert_eq!(Audit::sprt_state(&acc(PROVIDER)), SprtState::default());
        assert!(has_event(Event::DisputeOpened {
            id,
            provider: acc(PROVIDER),
            kind: d.kind,
        }));
        // While the dispute is open, verdicts still count but open nothing new.
        let before = Audit::sprt_state(&acc(PROVIDER)).cumulative;
        next_round();
        submit_round(&mut ids, INT8_STATS);
        assert!(Audit::sprt_state(&acc(PROVIDER)).cumulative > before);
        assert_eq!(OpenDispute::<Test>::get(acc(PROVIDER)), Some(id));
    });
}

#[test]
fn a_disabled_judgment_only_computes() {
    // "统计判定" / "未启用时只计算不开争议".
    new_test_ext(20).execute_with(|| {
        set_stats(CURRENT_STATS.version, false);
        let mut ids = Ids(0);
        assert_eq!(cross(&mut ids, 8), None);
        let s = Audit::sprt_state(&acc(PROVIDER));
        assert!(s.crossed(&CURRENT_STATS), "{}", s.cumulative);
        assert!(!s.entries.is_empty());
        // "参数与护栏" / "启用统计判定": enabled, the next verdict opens the dispute.
        set_stats(CURRENT_STATS.version, true);
        assert!(has_event(Event::StatsConfigSet {
            config: StatsConfig {
                version: CURRENT_STATS.version,
                enabled: true
            }
        }));
        next_round();
        submit_round(&mut ids, INT8_STATS);
        assert!(OpenDispute::<Test>::contains_key(acc(PROVIDER)));
    });
}

#[test]
fn too_few_reviewers_keep_the_state() {
    // "争议与升级复核" / "合格审计员不足": no dispute, the state is not reset.
    new_test_ext(4).execute_with(|| {
        let mut ids = Ids(0);
        assert_eq!(cross(&mut ids, 12), None);
        assert!(Audit::sprt_state(&acc(PROVIDER)).crossed(&CURRENT_STATS));
        assert!(has_event(Event::DisputeNotOpened {
            provider: acc(PROVIDER)
        }));
    });
}

#[test]
fn a_confirmed_statistical_dispute_slashes_and_jails() {
    // "处罚" / "统计争议确认后罚没并禁闭".
    new_test_ext(20).execute_with(|| {
        let mut ids = Ids(0);
        let id = cross(&mut ids, 20).unwrap();
        let d = Disputes::<Test>::get(id).unwrap();
        for (r, _) in d.reviewers.iter().take(2) {
            assert_ok!(Audit::vote(
                RuntimeOrigin::signed(r.clone()),
                acc(PROVIDER),
                id,
                Vote::Confirm
            ));
        }
        let d = Disputes::<Test>::get(id).unwrap();
        assert_eq!(d.outcome, Some(DisputeOutcome::Confirmed));
        assert_eq!(penalties().len(), 1);
        assert_eq!(penalties()[0].0, acc(PROVIDER));
        assert!(jailed(&acc(PROVIDER)));
    });
}

#[test]
fn a_rejected_statistical_dispute_slashes_no_auditor() {
    // "处罚" / "统计争议驳回不罚没审计员".
    new_test_ext(20).execute_with(|| {
        let mut ids = Ids(0);
        let id = cross(&mut ids, 20).unwrap();
        let d = Disputes::<Test>::get(id).unwrap();
        let held = |a: &AccountId32| Audit::auditor(a).unwrap().stake;
        let before: Vec<_> = d.accusers.iter().map(|a| held(&a.auditor)).collect();
        let issuance = pallet_balances::Pallet::<Test>::total_issuance();
        for (r, _) in d.reviewers.iter().take(2) {
            assert_ok!(Audit::vote(
                RuntimeOrigin::signed(r.clone()),
                acc(PROVIDER),
                id,
                Vote::Reject
            ));
        }
        assert_eq!(
            Disputes::<Test>::get(id).unwrap().outcome,
            Some(DisputeOutcome::Rejected)
        );
        let after: Vec<_> = d.accusers.iter().map(|a| held(&a.auditor)).collect();
        assert_eq!(before, after);
        for a in &d.accusers {
            let rec = Audit::auditor(&a.auditor).unwrap();
            assert_eq!(rec.status, AuditorStatus::Active);
            assert_eq!(
                pallet_balances::Pallet::<Test>::balance_on_hold(
                    &HoldReason::Stake.into(),
                    &a.auditor
                ),
                1_000 * ATC
            );
        }
        assert_eq!(burned(), 0);
        // Nothing minted or burned: the winning votes are paid from the pot.
        assert_eq!(pallet_balances::Pallet::<Test>::total_issuance(), issuance);
        assert!(penalties().is_empty());
        assert!(!jailed(&acc(PROVIDER)));
    });
}

// ---- 参数与护栏 ----

#[test]
fn unknown_statistics_versions_are_refused() {
    // "参数与护栏" / "未内置的统计参数版本"; only the administration sets the configuration.
    new_test_ext(6).execute_with(|| {
        assert_noop!(
            Audit::set_stats_config(
                RuntimeOrigin::root(),
                StatsConfig {
                    version: CURRENT_STATS.version + 1,
                    enabled: true
                }
            ),
            Error::<Test>::UnknownStatsVersion
        );
        assert_noop!(
            Audit::set_stats_config(
                RuntimeOrigin::root(),
                StatsConfig {
                    version: 0,
                    enabled: true
                }
            ),
            Error::<Test>::UnknownStatsVersion
        );
        assert_noop!(
            Audit::set_stats_config(
                RuntimeOrigin::signed(acc(1)),
                StatsConfig {
                    version: CURRENT_STATS.version,
                    enabled: false
                }
            ),
            sp_runtime::DispatchError::BadOrigin
        );
        assert!(!StaleClearing::<Test>::exists());
    });
}

#[test]
fn states_of_an_older_version_count_as_empty_and_are_cleared() {
    // "参数与护栏": a version change empties every state. Only version 1 is built in, so the
    // older version is written directly.
    new_test_ext(6).execute_with(|| {
        let a = assigned(PROVIDER);
        let old = SprtState {
            cumulative: 9_000,
            entries: vec![SprtEntry {
                auditor: a[0].clone(),
                round: current_round(),
                contribution: 3_000,
            }]
            .try_into()
            .unwrap(),
        };
        SprtStates::<Test>::insert(acc(PROVIDER), (0, old.clone()));
        SprtStates::<Test>::insert(acc(1), (0, old));
        assert_eq!(Audit::sprt_state(&acc(PROVIDER)), SprtState::default());
        // A verdict starts from empty under the current version.
        assert_ok!(Audit::submit_verdict(
            RuntimeOrigin::signed(a[0].clone()),
            submission_with(PROVIDER, 1, VerdictOutcome::Pass, Some(INT8_STATS))
        ));
        assert_eq!(Audit::sprt_state(&acc(PROVIDER)).cumulative, 3_000);
        // The clearing removes the stale state over the next blocks, not the current one.
        StaleClearing::<Test>::put(crate::ClearCursor::new());
        next_round();
        assert!(!StaleClearing::<Test>::exists());
        assert!(!SprtStates::<Test>::contains_key(acc(1)));
        assert_eq!(Audit::sprt_state(&acc(PROVIDER)).cumulative, 3_000);
    });
}

// ---- 计数与查询 ----

#[test]
fn verdicts_in_a_state_outlive_their_retention() {
    // "计数与查询" / "参与列表中的裁决不被删除": the list moves aside while the state refers to
    // it and is removed once the state lets it go.
    new_test_ext(6).execute_with(|| {
        let mut ids = Ids(0);
        let first = current_round();
        let a = assigned(PROVIDER);
        assert_ok!(Audit::submit_verdict(
            RuntimeOrigin::signed(a[0].clone()),
            submission_with(PROVIDER, ids.next(), VerdictOutcome::Pass, Some(INT8_STATS))
        ));
        assert_ok!(Audit::submit_verdict(
            RuntimeOrigin::signed(a[1].clone()),
            submission_with(PROVIDER, ids.next(), VerdictOutcome::Pass, Some(INT8_STATS))
        ));
        // Past retention (3 rounds in the mock) and pruned.
        for _ in 0..6 {
            next_round();
        }
        assert!(Verdicts::<Test>::get(first, acc(PROVIDER)).is_empty());
        assert!(RetainedVerdicts::<Test>::contains_key(first, acc(PROVIDER)));
        assert_eq!(Audit::verdict_list(first, &acc(PROVIDER)).len(), 2);
        // Honest verdicts bring the state back to 0 (−0.5 nats each against +6.0).
        for _ in 0..8 {
            if Audit::sprt_state(&acc(PROVIDER)).entries.is_empty() {
                break;
            }
            submit_round(&mut ids, HONEST_STATS);
            next_round();
        }
        assert_eq!(Audit::sprt_state(&acc(PROVIDER)), SprtState::default());
        assert!(!RetainedVerdicts::<Test>::contains_key(
            first,
            acc(PROVIDER)
        ));
        assert!(Audit::verdict_list(first, &acc(PROVIDER)).is_empty());
    });
}

#[test]
fn verdicts_of_an_open_dispute_outlive_their_retention() {
    // "计数与查询": a verdict an open dispute refers to is kept until the dispute closes.
    new_test_ext(20).execute_with(|| {
        let mut ids = Ids(0);
        let id = cross(&mut ids, 20).unwrap();
        let d = Disputes::<Test>::get(id).unwrap();
        let oldest = d.accusers.iter().map(|a| a.round).min().unwrap();
        for _ in 0..6 {
            next_round();
        }
        assert!(Verdicts::<Test>::get(oldest, acc(PROVIDER)).is_empty());
        assert!(!Audit::verdict_list(oldest, &acc(PROVIDER)).is_empty());
        // Closed undecided after its deadline: nothing refers to them any more.
        assert_ok!(Audit::close_dispute(
            RuntimeOrigin::signed(acc(1)),
            acc(PROVIDER),
            id
        ));
        assert!(Audit::verdict_list(oldest, &acc(PROVIDER)).is_empty());
    });
}

// ---- 迁移 ----

#[test]
fn migration_to_v1_keeps_verdicts_and_disputes() {
    // m6-audit-sprt 3.5: version-0 records gain `stats: None` and `kind: Fail`; the
    // configuration starts at the latest version, disabled.
    use crate::migrations::v1::{MigrateToV1, OldDisputeRecord, OldVerdictRecord};
    use frame_support::storage::unhashed;
    use parity_scale_codec::Encode;
    new_test_ext(6).execute_with(|| {
        let old = OldVerdictRecord {
            auditor: acc(1),
            outcome: VerdictOutcome::Pass,
            thresholds_version: 3,
            evidence: None,
            receipt_hash: sp_core::H256([5; 32]),
            request_id: [6; 32],
            gateway: acc(GATEWAY),
        };
        let key = Verdicts::<Test>::hashed_key_for(7, acc(PROVIDER));
        unhashed::put_raw(&key, &vec![old.clone()].encode());
        let dispute = OldDisputeRecord::<u64> {
            provider: acc(PROVIDER),
            round: 7,
            accusers: Default::default(),
            reviewers: Default::default(),
            deadline: 40,
            outcome: Some(DisputeOutcome::Undecided),
            closed_at: Some(41),
        };
        unhashed::put_raw(&Disputes::<Test>::hashed_key_for(3), &dispute.encode());
        StatsSettings::<Test>::kill();
        frame_support::traits::StorageVersion::new(0).put::<Audit>();

        MigrateToV1::<Test>::on_runtime_upgrade();

        assert_eq!(Audit::on_chain_storage_version(), 1);
        let v = &Verdicts::<Test>::get(7, acc(PROVIDER))[0];
        assert_eq!(
            (v.auditor.clone(), v.thresholds_version, v.stats),
            (acc(1), 3, None)
        );
        let d = Disputes::<Test>::get(3).unwrap();
        assert_eq!(
            (d.kind, d.deadline, d.closed_at),
            (DisputeKind::Fail, 40, Some(41))
        );
        assert_eq!(
            StatsSettings::<Test>::get(),
            Some(StatsConfig {
                version: CURRENT_STATS.version,
                enabled: false
            })
        );
        // Run again: the version check makes it a no-op.
        MigrateToV1::<Test>::on_runtime_upgrade();
        assert_eq!(Verdicts::<Test>::get(7, acc(PROVIDER)).len(), 1);
    });
}
