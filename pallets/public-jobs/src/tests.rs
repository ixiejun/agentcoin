//! Unit tests, one or more per scenario of spec `market/public-jobs` (named in comments).
// Test code: AGENT.md §5.3 permits unwrap, indexing and plain arithmetic.
#![allow(
    clippy::unwrap_used,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use frame_support::dispatch::Pays;
use frame_support::traits::fungible::{Inspect, InspectHold, Mutate};
use frame_support::{assert_noop, assert_ok};
use sp_core::H256;
use sp_runtime::{AccountId32, BuildStorage};

use ac_primitives::emission::PublicPayout;
use ac_primitives::market::MicroUsd;
use ac_primitives::market::public::{
    CanaryProof, CanaryReveal, MAX_WORKERS, PublicParams, Summary, UnitState, canary_leaf,
    canary_proof, canary_root,
};
use ac_primitives::market::work::JobKind;

use crate::mock::{
    ATC, Balances, MODEL, MODEL2, PublicJobs, RuntimeOrigin, System, Test, acc, burned, commit,
    models, new_test_ext, params, publish, ready, reveal, round_start, run_to, set_epoch,
    set_randomness, set_rate, spec,
};
use crate::{
    ActiveJobs, Assigned, CurrentRoster, Epochs, Error, Event, HoldReason, Jobs, Locked, Pending,
    Units, WorkerCount, Workers,
};

/// The three workers of a unit.
fn workers_of(job: u32, unit: u32) -> [AccountId32; 3] {
    Units::<Test>::get(job, unit).unwrap().assigned
}

/// Workers 1..=n ready in round 0, a job published, and the chain at round 1's first block.
fn opened(n: u8, s: ac_primitives::market::public::JobSpec) -> u32 {
    ready(&(1..=n).collect::<Vec<_>>());
    let job = publish(s);
    run_to(round_start(1));
    job
}

/// Commits `summaries` for the unit's three workers, moves past the commit deadline and
/// reveals them (`None`: that worker does nothing).
fn play(job: u32, unit: u32, summaries: [Option<Vec<u8>>; 3]) {
    let w = workers_of(job, unit);
    for (who, s) in w.iter().zip(&summaries) {
        if let Some(s) = s {
            commit(who, job, unit, s);
        }
    }
    let u = Units::<Test>::get(job, unit).unwrap();
    run_to(u.commit_by + 1);
    for (who, s) in w.iter().zip(&summaries) {
        if let Some(s) = s {
            assert_ok!(reveal(who, job, unit, s));
        }
    }
}

/// [`play`] for several units opened together: everyone commits first, then reveals.
fn play_all(job: u32, units: &[(u32, [Option<Vec<u8>>; 3])]) {
    for (unit, summaries) in units {
        for (who, s) in workers_of(job, *unit).iter().zip(summaries) {
            if let Some(s) = s {
                commit(who, job, *unit, s);
            }
        }
    }
    let commit_by = units
        .iter()
        .map(|(u, _)| Units::<Test>::get(job, *u).unwrap().commit_by)
        .max()
        .unwrap();
    run_to(commit_by + 1);
    for (unit, summaries) in units {
        for (who, s) in workers_of(job, *unit).iter().zip(summaries) {
            if let Some(s) = s {
                assert_ok!(reveal(who, job, *unit, s));
            }
        }
    }
}

fn hash(b: u8) -> Vec<u8> {
    vec![b; 32]
}

fn events() -> Vec<Event<Test>> {
    System::events()
        .into_iter()
        .filter_map(|r| match r.event {
            crate::mock::RuntimeEvent::PublicJobs(e) => Some(e),
            _ => None,
        })
        .collect()
}

// ---- 2.1 genesis ----

// Spec "参数与护栏" / "创世参数越界".
#[test]
#[should_panic(expected = "invalid public jobs genesis parameters")]
fn genesis_rejects_zero_units_per_round() {
    let mut storage = frame_system::GenesisConfig::<Test>::default()
        .build_storage()
        .unwrap();
    crate::GenesisConfig::<Test> {
        params: PublicParams {
            units_per_round: 0,
            ..params()
        },
        _marker: core::marker::PhantomData,
    }
    .assimilate_storage(&mut storage)
    .unwrap();
}

// ---- 2.2 workers ----

// Spec "工作者登记与就绪" / "零质押登记".
#[test]
fn zero_stake_registration() {
    new_test_ext(0).execute_with(|| {
        let before = Balances::balance(&acc(1));
        assert_ok!(PublicJobs::register(
            RuntimeOrigin::signed(acc(1)),
            models(&[MODEL])
        ));
        assert_eq!(Balances::balance(&acc(1)), before);
        assert_eq!(Balances::total_balance_on_hold(&acc(1)), 0);
        assert_eq!(WorkerCount::<Test>::get(), 1);
        assert_noop!(
            PublicJobs::register(RuntimeOrigin::signed(acc(1)), models(&[])),
            Error::<Test>::AlreadyRegistered
        );
    });
}

// Spec "工作者登记与就绪" / "声明未登记的模型".
#[test]
fn unknown_models_are_refused() {
    new_test_ext(1).execute_with(|| {
        let unknown = ac_primitives::market::ModelId([9; 32]);
        assert_noop!(
            PublicJobs::register(RuntimeOrigin::signed(acc(2)), models(&[unknown])),
            Error::<Test>::UnknownModel
        );
        assert_noop!(
            PublicJobs::set_models(RuntimeOrigin::signed(acc(1)), models(&[MODEL, unknown])),
            Error::<Test>::UnknownModel
        );
        assert_ok!(PublicJobs::set_models(
            RuntimeOrigin::signed(acc(1)),
            models(&[MODEL, MODEL2])
        ));
    });
}

#[test]
fn the_worker_limit_holds() {
    new_test_ext(0).execute_with(|| {
        WorkerCount::<Test>::put(MAX_WORKERS);
        assert_noop!(
            PublicJobs::register(RuntimeOrigin::signed(acc(1)), models(&[])),
            Error::<Test>::TooManyWorkers
        );
    });
}

#[test]
fn ready_is_free_once_per_round() {
    new_test_ext(1).execute_with(|| {
        let info = PublicJobs::ready(RuntimeOrigin::signed(acc(1))).unwrap();
        assert_eq!(info.pays_fee, Pays::No);
        assert_noop!(
            PublicJobs::ready(RuntimeOrigin::signed(acc(1))),
            Error::<Test>::AlreadyReady
        );
        run_to(round_start(1));
        assert_ok!(PublicJobs::ready(RuntimeOrigin::signed(acc(1))));
        assert_noop!(
            PublicJobs::ready(RuntimeOrigin::signed(acc(2))),
            Error::<Test>::NotWorker
        );
    });
}

// Spec "工作者登记与就绪" / "未就绪不分配".
#[test]
fn workers_not_ready_are_not_drawn() {
    new_test_ext(5).execute_with(|| {
        let job = opened(4, spec(JobKind::DataClean, 4));
        let roster: Vec<AccountId32> = CurrentRoster::<Test>::get()
            .into_iter()
            .map(|(w, _)| w)
            .collect();
        assert!(!roster.contains(&acc(5)));
        for unit in 0..4 {
            assert!(!workers_of(job, unit).contains(&acc(5)));
        }
    });
}

#[test]
fn deregistering_needs_no_unsettled_units() {
    new_test_ext(3).execute_with(|| {
        let job = opened(3, spec(JobKind::DataClean, 1));
        assert_noop!(
            PublicJobs::deregister(RuntimeOrigin::signed(acc(1))),
            Error::<Test>::HasUnits
        );
        play(job, 0, [Some(hash(1)), Some(hash(1)), Some(hash(1))]);
        assert_ok!(PublicJobs::deregister(RuntimeOrigin::signed(acc(1))));
        assert_eq!(WorkerCount::<Test>::get(), 2);
        assert!(Workers::<Test>::get(acc(1)).is_none());
    });
}

// ---- 2.3 jobs ----

// Spec "任务发布与取消" / "管理权限发布评测任务".
#[test]
fn root_publishes_and_units_open_next_round() {
    new_test_ext(3).execute_with(|| {
        ready(&[1, 2, 3]);
        let mut s = spec(JobKind::Eval, 100);
        s.price = MicroUsd(10_000);
        let job = publish(s);
        assert!(events().contains(&Event::JobPublished {
            job,
            kind: JobKind::Eval,
            units: 100
        }));
        assert!(Units::<Test>::get(job, 0).is_none());
        run_to(round_start(1));
        assert_eq!(Jobs::<Test>::get(job).unwrap().opened, 4, "units per round");
        assert!(Units::<Test>::get(job, 0).is_some());
    });
}

// Spec "任务发布与取消" / "普通账户不能发布".
#[test]
fn signed_accounts_cannot_publish_or_cancel() {
    new_test_ext(1).execute_with(|| {
        assert_noop!(
            PublicJobs::publish(
                RuntimeOrigin::signed(acc(1)),
                Box::new(spec(JobKind::Eval, 1))
            ),
            sp_runtime::DispatchError::BadOrigin
        );
        let job = publish(spec(JobKind::Eval, 1));
        assert_noop!(
            PublicJobs::cancel(RuntimeOrigin::signed(acc(1)), job),
            sp_runtime::DispatchError::BadOrigin
        );
    });
}

// Spec "任务发布与取消" / "训练任务被拒绝".
#[test]
fn training_jobs_are_refused() {
    new_test_ext(0).execute_with(|| {
        let mut s = spec(JobKind::Eval, 1);
        s.kind = JobKind::Pretrain;
        assert_noop!(
            PublicJobs::publish(RuntimeOrigin::root(), Box::new(s)),
            Error::<Test>::InvalidSpec
        );
    });
}

#[test]
fn specs_need_registered_models_a_price_within_the_cap_and_room() {
    new_test_ext(0).execute_with(|| {
        let mut s = spec(JobKind::Eval, 1);
        s.model = Some(ac_primitives::market::ModelId([9; 32]));
        assert_noop!(
            PublicJobs::publish(RuntimeOrigin::root(), Box::new(s)),
            Error::<Test>::UnknownModel
        );
        let mut s = spec(JobKind::Eval, 1);
        s.price = MicroUsd(params().price_cap.0 + 1);
        assert_noop!(
            PublicJobs::publish(RuntimeOrigin::root(), Box::new(s)),
            Error::<Test>::InvalidSpec
        );
        for _ in 0..16 {
            publish(spec(JobKind::DataClean, 1));
        }
        assert_noop!(
            PublicJobs::publish(RuntimeOrigin::root(), Box::new(spec(JobKind::DataClean, 1))),
            Error::<Test>::TooManyJobs
        );
    });
}

// Spec "任务发布与取消" / "取消任务".
#[test]
fn cancelled_jobs_open_no_more_units() {
    new_test_ext(12).execute_with(|| {
        let job = opened(12, spec(JobKind::DataClean, 10));
        assert_eq!(Jobs::<Test>::get(job).unwrap().opened, 4);
        assert_ok!(PublicJobs::cancel(RuntimeOrigin::root(), job));
        assert!(ActiveJobs::<Test>::get().is_empty());
        assert_noop!(
            PublicJobs::cancel(RuntimeOrigin::root(), job),
            Error::<Test>::Cancelled
        );
        // The opened units still settle.
        play(job, 0, [Some(hash(1)), Some(hash(1)), Some(hash(1))]);
        assert!(matches!(
            Units::<Test>::get(job, 0).unwrap().state,
            UnitState::Accepted { .. }
        ));
        ready(&(1..=12).collect::<Vec<_>>());
        run_to(round_start(2));
        assert_eq!(Jobs::<Test>::get(job).unwrap().opened, 4);
    });
}

// Spec "参数与护栏" / "护栏外的价格上限".
#[test]
fn the_price_cap_has_a_guardrail() {
    new_test_ext(0).execute_with(|| {
        assert_noop!(
            PublicJobs::set_price_cap(RuntimeOrigin::root(), MicroUsd(20_000_000)),
            Error::<Test>::OutOfBounds
        );
        assert_noop!(
            PublicJobs::set_price_cap(RuntimeOrigin::signed(acc(1)), MicroUsd(1)),
            sp_runtime::DispatchError::BadOrigin
        );
        assert_ok!(PublicJobs::set_price_cap(
            RuntimeOrigin::root(),
            MicroUsd(10_000_000)
        ));
        assert_eq!(
            crate::Params::<Test>::get().unwrap().price_cap,
            MicroUsd(10_000_000)
        );
    });
}

// ---- 3.1 opening and assignment ----

/// Independent reference of the draw: BLAKE3 `derive_key` directly, probing forward.
fn reference_draw(
    roster: &[AccountId32],
    seed: &H256,
    unit: (u32, u32, u8),
    eligible: impl Fn(&AccountId32) -> bool,
) -> Vec<AccountId32> {
    use parity_scale_codec::Encode;
    let want = roster.iter().filter(|a| eligible(a)).count().min(3);
    let mut out: Vec<AccountId32> = Vec::new();
    let mut i: u32 = 0;
    while out.len() < want {
        let mut data = seed.as_bytes().to_vec();
        data.extend(unit.encode());
        data.extend(i.to_le_bytes());
        let h = ac_crypto::hash::derive("agentcoin 2026-10 public-assign v1", &data).unwrap();
        let mut j = (u64::from_le_bytes(h[..8].try_into().unwrap()) % roster.len() as u64) as usize;
        loop {
            let a = &roster[j];
            if eligible(a) && !out.contains(a) {
                out.push(a.clone());
                break;
            }
            j = (j + 1) % roster.len();
        }
        i += 1;
    }
    out
}

// Spec "单元开放与分配" / "复算分配".
#[test]
fn assignments_can_be_recomputed() {
    new_test_ext(20).execute_with(|| {
        let job = opened(20, spec(JobKind::Eval, 4));
        let roster: Vec<AccountId32> = CurrentRoster::<Test>::get()
            .into_iter()
            .map(|(w, _)| w)
            .collect();
        let seed = crate::Seed::<Test>::get().unwrap();
        let cap = ac_primitives::market::public::max_per_worker(4, roster.len());
        let mut load = std::collections::BTreeMap::<AccountId32, u32>::new();
        for unit in 0..4 {
            let drawn = reference_draw(&roster, &seed, (job, unit, 1), |a| {
                load.get(a).copied().unwrap_or(0) < cap
            });
            assert_eq!(drawn.as_slice(), workers_of(job, unit).as_slice());
            for a in drawn {
                *load.entry(a).or_default() += 1;
            }
        }
    });
}

// Spec "单元开放与分配" / "只分配给能运行模型的工作者".
#[test]
fn units_open_only_with_three_capable_workers() {
    new_test_ext(4).execute_with(|| {
        for n in 3..=4 {
            assert_ok!(PublicJobs::set_models(
                RuntimeOrigin::signed(acc(n)),
                models(&[MODEL2])
            ));
        }
        let job = opened(4, spec(JobKind::Eval, 2));
        assert!(Units::<Test>::get(job, 0).is_none());
        assert_eq!(Jobs::<Test>::get(job).unwrap().opened, 0);
        // Data cleaning takes no model: all four are eligible.
        let clean = publish(spec(JobKind::DataClean, 1));
        ready(&[1, 2, 3, 4]);
        run_to(round_start(2));
        assert!(Units::<Test>::get(job, 0).is_none());
        assert!(
            Units::<Test>::get(clean, 0).is_some(),
            "other jobs still open"
        );
    });
}

#[test]
fn no_randomness_opens_nothing() {
    new_test_ext(3).execute_with(|| {
        set_randomness(None);
        let job = opened(3, spec(JobKind::DataClean, 1));
        assert!(Units::<Test>::get(job, 0).is_none());
        assert!(crate::Seed::<Test>::get().is_none());
    });
}

#[test]
fn at_most_units_per_round_open_and_each_worker_is_capped() {
    new_test_ext(3).execute_with(|| {
        // Three workers, four units: each worker may take ⌈12 / 3⌉ + 1 = 5.
        let job = opened(3, spec(JobKind::DataClean, 9));
        assert_eq!(Jobs::<Test>::get(job).unwrap().opened, 4);
        for n in 1..=3 {
            assert_eq!(Assigned::<Test>::iter_prefix(acc(n)).count(), 4);
        }
    });
}

// ---- 3.2 commit and reveal ----

// Spec "承诺与揭示" / "未被分配者不能提交".
#[test]
fn only_assigned_workers_commit() {
    new_test_ext(4).execute_with(|| {
        let job = opened(4, spec(JobKind::DataClean, 1));
        let w = workers_of(job, 0);
        let outsider = (1..=4).map(acc).find(|a| !w.contains(a)).unwrap();
        assert_noop!(
            PublicJobs::commit(RuntimeOrigin::signed(outsider), job, 0, H256([1; 32])),
            Error::<Test>::NotAssigned
        );
        let info =
            PublicJobs::commit(RuntimeOrigin::signed(w[0].clone()), job, 0, H256([1; 32])).unwrap();
        assert_eq!(info.pays_fee, Pays::No);
        assert_noop!(
            PublicJobs::commit(RuntimeOrigin::signed(w[0].clone()), job, 0, H256([2; 32])),
            Error::<Test>::AlreadySubmitted
        );
    });
}

// Spec "承诺与揭示" / "承诺期限之后".
#[test]
fn commits_close_at_the_deadline() {
    new_test_ext(3).execute_with(|| {
        let job = opened(3, spec(JobKind::DataClean, 1));
        let u = Units::<Test>::get(job, 0).unwrap();
        run_to(u.commit_by + 1);
        assert_noop!(
            PublicJobs::commit(
                RuntimeOrigin::signed(u.assigned[0].clone()),
                job,
                0,
                H256([1; 32])
            ),
            Error::<Test>::CommitClosed
        );
    });
}

// Spec "承诺与揭示" / "过早揭示".
#[test]
fn reveals_before_the_commit_deadline_fail() {
    new_test_ext(3).execute_with(|| {
        let job = opened(3, spec(JobKind::DataClean, 1));
        let w = workers_of(job, 0);
        commit(&w[0], job, 0, &hash(1));
        assert_noop!(reveal(&w[0], job, 0, &hash(1)), Error::<Test>::RevealWindow);
        assert!(Units::<Test>::get(job, 0).unwrap().reveals[0].is_none());
        let u = Units::<Test>::get(job, 0).unwrap();
        run_to(u.reveal_by + 1);
        // Still open until someone closes it, but the reveal window is over.
        assert_noop!(reveal(&w[0], job, 0, &hash(1)), Error::<Test>::RevealWindow);
    });
}

// Spec "承诺与揭示" / "揭示与承诺不符".
#[test]
fn reveals_must_match_commitments_and_formats() {
    new_test_ext(3).execute_with(|| {
        let job = opened(3, spec(JobKind::DataClean, 1));
        let w = workers_of(job, 0);
        commit(&w[0], job, 0, &hash(1));
        let u = Units::<Test>::get(job, 0).unwrap();
        run_to(u.commit_by + 1);
        assert_noop!(
            reveal(&w[0], job, 0, &hash(2)),
            Error::<Test>::CommitmentMismatch
        );
        assert_noop!(reveal(&w[0], job, 0, &[1; 31]), Error::<Test>::BadSummary);
        assert_noop!(reveal(&w[1], job, 0, &hash(1)), Error::<Test>::NotCommitted);
        let info = reveal(&w[0], job, 0, &hash(1)).unwrap();
        assert_eq!(info.pays_fee, Pays::No);
        assert_noop!(
            reveal(&w[0], job, 0, &hash(1)),
            Error::<Test>::AlreadySubmitted
        );
    });
}

// ---- 3.3 settlement ----

fn pending(n: u8) -> u128 {
    Pending::<Test>::get(acc(n)).iter().map(|(_, w)| w).sum()
}

fn pending_of(who: &AccountId32) -> u128 {
    Pending::<Test>::get(who).iter().map(|(_, w)| w).sum()
}

// One cent at 1 ATC per dollar.
const CENT: u128 = ATC / 100;

// Spec "单元结算" / "三人一致".
#[test]
fn three_agreeing_workers_all_earn() {
    new_test_ext(3).execute_with(|| {
        set_epoch(5);
        let job = opened(3, spec(JobKind::DataClean, 1));
        play(job, 0, [Some(hash(1)), Some(hash(1)), Some(hash(1))]);
        let u = Units::<Test>::get(job, 0).unwrap();
        assert_eq!(
            u.state,
            UnitState::Accepted {
                reference: 0,
                majority: [true; 3]
            }
        );
        for n in 1..=3 {
            assert_eq!(pending(n), CENT);
            assert_eq!(Pending::<Test>::get(acc(n))[0].0, 6, "epoch 5 + 1");
            assert_eq!(Workers::<Test>::get(acc(n)).unwrap().accepted, 1);
        }
        assert_eq!(Epochs::<Test>::get(6).verified, 3 * CENT);
        assert_eq!(Jobs::<Test>::get(job).unwrap().accepted, 1);
        assert_eq!(Assigned::<Test>::iter().count(), 0);
    });
}

// Spec "单元结算" / "二对一".
#[test]
fn two_against_one() {
    new_test_ext(3).execute_with(|| {
        let job = opened(3, spec(JobKind::DataClean, 1));
        let w = workers_of(job, 0);
        play(job, 0, [Some(hash(1)), Some(hash(2)), Some(hash(1))]);
        assert_eq!(pending_of(&w[0]), CENT);
        assert_eq!(pending_of(&w[1]), 0);
        assert_eq!(pending_of(&w[2]), CENT);
        assert_eq!(Workers::<Test>::get(&w[1]).unwrap().misses, 1);
        assert_eq!(Workers::<Test>::get(&w[0]).unwrap().misses, 0);
    });
}

// Spec "单元结算" / "无人一致则重开".
#[test]
fn no_agreement_reopens_to_other_workers() {
    new_test_ext(6).execute_with(|| {
        let job = opened(6, spec(JobKind::DataClean, 1));
        let first = workers_of(job, 0);
        play(job, 0, [Some(hash(1)), Some(hash(2)), Some(hash(3))]);
        assert_eq!(Units::<Test>::get(job, 0).unwrap().state, UnitState::Retry);
        for w in &first {
            assert_eq!(Workers::<Test>::get(w).unwrap().misses, 1);
        }
        ready(&[1, 2, 3, 4, 5, 6]);
        run_to(round_start(2));
        let u = Units::<Test>::get(job, 0).unwrap();
        assert_eq!(u.attempt, 2);
        assert_eq!(u.state, UnitState::Open);
        for w in &u.assigned {
            assert!(!first.contains(w));
        }
    });
}

#[test]
fn three_failed_attempts_fail_the_unit() {
    new_test_ext(9).execute_with(|| {
        let job = opened(9, spec(JobKind::DataClean, 1));
        for round in 1..=3u64 {
            play(job, 0, [Some(hash(1)), Some(hash(2)), Some(hash(3))]);
            ready(&(1..=9).collect::<Vec<_>>());
            run_to(round_start(round + 1));
        }
        let u = Units::<Test>::get(job, 0).unwrap();
        assert_eq!(u.state, UnitState::Failed);
        assert_eq!(u.tried.len(), 6);
        assert_eq!(Jobs::<Test>::get(job).unwrap().failed, 1);
    });
}

// Spec "单元结算" / "超时关闭".
#[test]
fn closing_after_the_deadline_settles_with_misses() {
    new_test_ext(3).execute_with(|| {
        let job = opened(3, spec(JobKind::DataClean, 1));
        let w = workers_of(job, 0);
        play(job, 0, [Some(hash(1)), None, None]);
        assert_noop!(
            PublicJobs::close(RuntimeOrigin::signed(acc(9)), job, 0),
            Error::<Test>::RevealNotOver
        );
        let u = Units::<Test>::get(job, 0).unwrap();
        run_to(u.reveal_by + 1);
        let info = PublicJobs::close(RuntimeOrigin::signed(w[1].clone()), job, 0).unwrap();
        assert_eq!(info.pays_fee, Pays::No);
        assert_eq!(Units::<Test>::get(job, 0).unwrap().state, UnitState::Retry);
        for who in &w {
            assert_eq!(Workers::<Test>::get(who).unwrap().misses, 1);
        }
    });
}

#[test]
fn no_rate_settles_without_work() {
    new_test_ext(3).execute_with(|| {
        let job = opened(3, spec(JobKind::DataClean, 1));
        set_rate(None);
        play(job, 0, [Some(hash(1)), Some(hash(1)), Some(hash(1))]);
        assert!(matches!(
            Units::<Test>::get(job, 0).unwrap().state,
            UnitState::Accepted { .. }
        ));
        assert_eq!(pending(1), 0);
    });
}

// ---- 3.4 misses and suspension ----

// Spec "失误与暂停" / "连续三次失误".
#[test]
fn three_consecutive_misses_suspend() {
    new_test_ext(3).execute_with(|| {
        let job = opened(3, spec(JobKind::DataClean, 3));
        let plays: Vec<(u32, [Option<Vec<u8>>; 3])> = (0..3)
            .map(|unit| {
                let lazy = workers_of(job, unit)
                    .iter()
                    .position(|a| *a == acc(1))
                    .unwrap();
                let mut s = [Some(hash(1)), Some(hash(1)), Some(hash(1))];
                s[lazy] = None;
                (unit, s)
            })
            .collect();
        play_all(job, &plays);
        run_to(Units::<Test>::get(job, 0).unwrap().reveal_by + 1);
        for unit in 0..3 {
            assert_ok!(PublicJobs::close(RuntimeOrigin::signed(acc(9)), job, unit));
        }
        let w = Workers::<Test>::get(acc(1)).unwrap();
        let until = w.suspended_until.unwrap();
        assert_eq!(w.missed, 3);
        ready(&[1, 2, 3]);
        run_to(round_start(2));
        let on_roster = |n: u8| {
            CurrentRoster::<Test>::get()
                .iter()
                .any(|(a, _)| *a == acc(n))
        };
        assert!(!on_roster(1));
        assert!(on_roster(2));
        // After the suspension the worker is back.
        while System::block_number() < until {
            ready(&[1]);
            let next = (System::block_number() - 1) / 10 + 1;
            run_to(round_start(next));
        }
        ready(&[1]);
        let next = (System::block_number() - 1) / 10 + 1;
        run_to(round_start(next));
        assert!(on_roster(1));
    });
}

// ---- 3.5 queries and retention ----

// Spec "查询与保留" / "工作者查询自己的分配".
#[test]
fn workers_query_their_units() {
    new_test_ext(3).execute_with(|| {
        let job = opened(3, spec(JobKind::DataClean, 2));
        let mine = PublicJobs::assigned(&acc(1));
        assert_eq!(mine.len(), 2);
        let u = Units::<Test>::get(job, 0).unwrap();
        assert_eq!(
            (mine[0].commit_by, mine[0].reveal_by),
            (u.commit_by, u.reveal_by)
        );
        assert!(!mine[0].committed);
        assert_eq!(
            PublicJobs::round_bounds(),
            Some((1, round_start(1) as u32, round_start(2) as u32 - 1))
        );
    });
}

#[test]
fn settled_units_are_pruned_after_retention_a_few_per_block() {
    new_test_ext(3).execute_with(|| {
        let job = opened(3, spec(JobKind::DataClean, 4));
        let all = [Some(hash(1)), Some(hash(1)), Some(hash(1))];
        play_all(job, &(0..4).map(|u| (u, all.clone())).collect::<Vec<_>>());
        let settled_at = System::block_number();
        let retention = u64::from(params().retention_blocks);
        run_to(settled_at + retention - 1);
        assert!(Units::<Test>::get(job, 3).is_some());
        run_to(settled_at + retention + 1);
        for unit in 0..4 {
            assert!(Units::<Test>::get(job, unit).is_none());
        }
    });
}

#[test]
fn pruning_removes_at_most_the_limit_per_block() {
    new_test_ext(12).execute_with(|| {
        let job = opened(12, spec(JobKind::DataClean, 8));
        let all = [Some(hash(1)), Some(hash(1)), Some(hash(1))];
        play_all(job, &(0..4).map(|u| (u, all.clone())).collect::<Vec<_>>());
        ready(&(1..=12).collect::<Vec<_>>());
        run_to(round_start(2));
        play_all(job, &(4..8).map(|u| (u, all.clone())).collect::<Vec<_>>());
        // Eight settled units, a limit of four per block.
        let at = System::block_number() + u64::from(params().retention_blocks);
        assert_eq!(PublicJobs::prune_step(at, &params()), 4);
        assert_eq!(PublicJobs::prune_step(at, &params()), 4);
        assert_eq!(PublicJobs::prune_step(at, &params()), 0);
    });
}

// ---- 4.1 canaries ----

/// A job whose single unit is a canary with expected summary `expected`; returns the job and
/// the canary's salt and proof.
fn canary_job(expected: &[u8]) -> (u32, [u8; 32], CanaryProof) {
    let job = crate::NextJob::<Test>::get();
    let salt = [0x5a; 32];
    let leaves = vec![
        canary_leaf(job, 0, expected, &salt),
        canary_leaf(job, 7, &[0; 32], &salt),
    ];
    let mut s = spec(JobKind::DataClean, 1);
    s.canary_root = canary_root(&leaves);
    let id = publish(s);
    assert_eq!(id, job);
    (job, salt, canary_proof(&leaves, 0).unwrap())
}

fn summary(v: &[u8]) -> Summary {
    Summary::truncate_from(v.to_vec())
}

/// Gives `who` locked rewards: credits work in its own epoch, settles it and claims.
fn lock_rewards(who: &AccountId32, amount: u128) {
    let epoch = Epochs::<Test>::iter_keys().max().unwrap_or(0) + 10;
    set_epoch(epoch);
    crate::Pallet::<Test>::credit_for_test(who, amount);
    set_epoch(0);
    let epoch = Pending::<Test>::get(who).last().unwrap().0;
    settle_epoch(epoch, amount);
    assert_ok!(PublicJobs::claim(
        RuntimeOrigin::signed(who.clone()),
        frame_support::BoundedVec::truncate_from(vec![epoch])
    ));
}

/// Mints `amount` into the payout account and settles `epoch` with it.
fn settle_epoch(epoch: u64, amount: u128) {
    let work = Epochs::<Test>::get(epoch).verified;
    Balances::mint_into(&PublicJobs::pot(), amount).unwrap();
    <PublicJobs as PublicPayout<AccountId32>>::settled(epoch, amount, work);
}

// Spec "金丝雀单元" / "串通伪造被金丝雀查出".
#[test]
fn a_colluding_majority_is_caught_by_its_canary() {
    new_test_ext(3).execute_with(|| {
        let (job, salt, proof) = canary_job(&hash(7));
        ready(&[1, 2, 3]);
        run_to(round_start(1));
        let w = workers_of(job, 0);
        for who in &w[..2] {
            lock_rewards(who, 50 * ATC);
        }
        let issuance = Balances::total_issuance();
        play(job, 0, [Some(hash(6)), Some(hash(6)), Some(hash(7))]);
        assert!(pending_of(&w[0]) > 0);
        assert_ok!(PublicJobs::reveal_canary(
            RuntimeOrigin::signed(acc(40)),
            job,
            0,
            Box::new(CanaryReveal {
                summary: summary(&hash(7)),
                salt,
                proof: proof.clone()
            })
        ));
        for who in &w[..2] {
            assert_eq!(pending_of(who), 0);
            assert!(Locked::<Test>::get(who).is_empty());
            assert_eq!(
                Balances::balance_on_hold(&HoldReason::Locked.into(), who),
                0
            );
            assert!(Workers::<Test>::get(who).unwrap().suspended_until.is_some());
        }
        assert_eq!(
            pending_of(&w[2]),
            CENT,
            "the honest minority earns the unit"
        );
        assert_eq!(Workers::<Test>::get(&w[2]).unwrap().misses, 0);
        assert_eq!(Units::<Test>::get(job, 0).unwrap().state, UnitState::Failed);
        // Spec "公共工作量与报酬" / "罚没守恒": the locked rewards are burned.
        assert_eq!(burned(), 100 * ATC);
        assert_eq!(Balances::total_issuance(), issuance - 100 * ATC);
        // Only once.
        assert_noop!(
            PublicJobs::reveal_canary(
                RuntimeOrigin::signed(acc(40)),
                job,
                0,
                Box::new(CanaryReveal {
                    summary: summary(&hash(7)),
                    salt,
                    proof
                })
            ),
            Error::<Test>::NoCanary
        );
    });
}

// Spec "金丝雀单元" / "诚实单元的金丝雀".
#[test]
fn an_honest_canary_changes_nothing() {
    new_test_ext(3).execute_with(|| {
        let (job, salt, proof) = canary_job(&hash(7));
        ready(&[1, 2, 3]);
        run_to(round_start(1));
        play(job, 0, [Some(hash(7)), Some(hash(7)), Some(hash(7))]);
        assert_ok!(PublicJobs::reveal_canary(
            RuntimeOrigin::signed(acc(40)),
            job,
            0,
            Box::new(CanaryReveal {
                summary: summary(&hash(7)),
                salt,
                proof
            })
        ));
        for n in 1..=3 {
            assert_eq!(pending(n), CENT);
        }
        assert!(Units::<Test>::get(job, 0).unwrap().canary_revealed);
        assert!(events().contains(&Event::CanaryPassed { job, unit: 0 }));
    });
}

// Spec "金丝雀单元" / "证明不成立".
#[test]
fn a_bad_canary_proof_fails() {
    new_test_ext(3).execute_with(|| {
        let (job, salt, proof) = canary_job(&hash(7));
        ready(&[1, 2, 3]);
        run_to(round_start(1));
        play(job, 0, [Some(hash(6)), Some(hash(6)), Some(hash(6))]);
        assert_noop!(
            PublicJobs::reveal_canary(
                RuntimeOrigin::signed(acc(40)),
                job,
                0,
                Box::new(CanaryReveal {
                    summary: summary(&hash(6)),
                    salt,
                    proof: proof.clone()
                })
            ),
            Error::<Test>::BadProof
        );
        assert_noop!(
            PublicJobs::reveal_canary(
                RuntimeOrigin::signed(acc(40)),
                job,
                0,
                Box::new(CanaryReveal {
                    summary: summary(&hash(7)),
                    salt: [0; 32],
                    proof
                })
            ),
            Error::<Test>::BadProof
        );
        assert!(!Units::<Test>::get(job, 0).unwrap().canary_revealed);
    });
}

// ---- 4.3 claims and locks ----

// Spec "公共工作量与报酬" / "按工作量分配公共排放".
#[test]
fn emission_is_shared_by_work() {
    new_test_ext(2).execute_with(|| {
        set_epoch(3);
        crate::Pallet::<Test>::credit_for_test(&acc(1), 3 * ATC);
        crate::Pallet::<Test>::credit_for_test(&acc(2), ATC);
        let epoch = 3 + u64::from(params().challenge_epochs);
        assert_eq!(Epochs::<Test>::get(epoch).verified, 4 * ATC);
        settle_epoch(epoch, 400 * ATC);
        for (n, share) in [(1u8, 300 * ATC), (2, 100 * ATC)] {
            assert_ok!(PublicJobs::claim(
                RuntimeOrigin::signed(acc(n)),
                frame_support::BoundedVec::truncate_from(vec![epoch])
            ));
            assert_eq!(
                Balances::balance_on_hold(&HoldReason::Locked.into(), &acc(n)),
                share
            );
        }
        assert_noop!(
            PublicJobs::claim(
                RuntimeOrigin::signed(acc(1)),
                frame_support::BoundedVec::truncate_from(vec![epoch])
            ),
            Error::<Test>::NothingToClaim
        );
    });
}

#[test]
fn unsettled_epochs_cannot_be_claimed() {
    new_test_ext(1).execute_with(|| {
        crate::Pallet::<Test>::credit_for_test(&acc(1), ATC);
        let epoch = Pending::<Test>::get(acc(1))[0].0;
        assert_noop!(
            PublicJobs::claim(
                RuntimeOrigin::signed(acc(1)),
                frame_support::BoundedVec::truncate_from(vec![epoch])
            ),
            Error::<Test>::NothingToClaim
        );
    });
}

// Spec "公共工作量与报酬" / "锁定期内不能取出".
#[test]
fn rewards_unlock_after_the_lock() {
    new_test_ext(1).execute_with(|| {
        lock_rewards(&acc(1), 10 * ATC);
        let free = Balances::balance(&acc(1));
        let (unlock_at, amount) = Locked::<Test>::get(acc(1))[0];
        assert_eq!(amount, 10 * ATC);
        assert!(unlock_at >= System::block_number() + u64::from(params().lock_blocks));
        assert_noop!(
            PublicJobs::withdraw(RuntimeOrigin::signed(acc(1))),
            Error::<Test>::NothingToWithdraw
        );
        run_to(unlock_at);
        let info = PublicJobs::withdraw(RuntimeOrigin::signed(acc(1))).unwrap();
        assert_eq!(info.pays_fee, Pays::No);
        assert_eq!(Balances::balance(&acc(1)), free + 10 * ATC);
        assert!(Locked::<Test>::get(acc(1)).is_empty());
    });
}

#[test]
fn claims_of_one_stretch_share_a_segment_and_deregistered_workers_still_claim() {
    new_test_ext(1).execute_with(|| {
        lock_rewards(&acc(1), ATC);
        set_epoch(1);
        lock_rewards(&acc(1), ATC);
        assert_eq!(Locked::<Test>::get(acc(1)).len(), 1);
        assert_ok!(PublicJobs::deregister(RuntimeOrigin::signed(acc(1))));
        set_epoch(2);
        crate::Pallet::<Test>::credit_for_test(&acc(1), ATC);
        let epoch = Pending::<Test>::get(acc(1))[0].0;
        settle_epoch(epoch, ATC);
        assert_ok!(PublicJobs::claim(
            RuntimeOrigin::signed(acc(1)),
            frame_support::BoundedVec::truncate_from(vec![epoch])
        ));
    });
}

#[test]
fn the_payout_account_keeps_its_existential_deposit() {
    new_test_ext(1).execute_with(|| {
        crate::Pallet::<Test>::credit_for_test(&acc(1), ATC);
        let epoch = Pending::<Test>::get(acc(1))[0].0;
        // The account holds 1 (the existential deposit) and receives exactly the emission.
        settle_epoch(epoch, ATC);
        assert_eq!(Epochs::<Test>::get(epoch).emission, Some(ATC));
        assert_ok!(PublicJobs::claim(
            RuntimeOrigin::signed(acc(1)),
            frame_support::BoundedVec::truncate_from(vec![epoch])
        ));
        assert_eq!(PublicJobs::pot_balance(), 1);
    });
}

#[test]
fn a_caught_majority_loses_settled_unclaimed_shares_too() {
    new_test_ext(3).execute_with(|| {
        let (job, salt, proof) = canary_job(&hash(7));
        ready(&[1, 2, 3]);
        run_to(round_start(1));
        let w = workers_of(job, 0);
        // w[0] has a settled, unclaimed share of 40 ATC in its own epoch.
        set_epoch(20);
        crate::Pallet::<Test>::credit_for_test(&w[0], ATC);
        settle_epoch(21, 40 * ATC);
        set_epoch(0);
        let pot = PublicJobs::pot_balance();
        play(job, 0, [Some(hash(6)), Some(hash(6)), Some(hash(7))]);
        assert_ok!(PublicJobs::reveal_canary(
            RuntimeOrigin::signed(acc(40)),
            job,
            0,
            Box::new(CanaryReveal {
                summary: summary(&hash(7)),
                salt,
                proof
            })
        ));
        assert_eq!(PublicJobs::pot_balance(), pot - 40 * ATC);
        assert_eq!(burned(), 40 * ATC);
        assert_eq!(crate::Unclaimed::<Test>::get(), 0);
        assert!(events().iter().any(|e| matches!(
            e,
            Event::Punished { burned, .. } if *burned == 40 * ATC
        )));
    });
}

#[test]
fn claims_are_free() {
    new_test_ext(1).execute_with(|| {
        crate::Pallet::<Test>::credit_for_test(&acc(1), ATC);
        let epoch = Pending::<Test>::get(acc(1))[0].0;
        settle_epoch(epoch, ATC);
        let info = PublicJobs::claim(
            RuntimeOrigin::signed(acc(1)),
            frame_support::BoundedVec::truncate_from(vec![epoch]),
        )
        .unwrap();
        assert_eq!(info.pays_fee, Pays::No);
    });
}
