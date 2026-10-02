// Test code: AGENT.md §5.3 permits unwrap, indexing and plain arithmetic.
#![allow(
    clippy::unwrap_used,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use proptest::prelude::*;

use super::*;

fn spec(kind: JobKind) -> JobSpec {
    JobSpec {
        kind,
        model: (kind != JobKind::DataClean).then_some(ModelId([7; 32])),
        rules: RULES_V1,
        manifest_hash: [1; 32],
        manifest_url: Url::truncate_from(b"https://data.example/manifest.json".to_vec()),
        results_url: Url::truncate_from(b"https://results.example".to_vec()),
        units: 100,
        price: MicroUsd(10_000),
        canary_root: None,
    }
}

fn eval(answers: &[u8]) -> Vec<u8> {
    answers.to_vec()
}

fn embed(prints: &[u32]) -> Vec<u8> {
    prints.iter().flat_map(|p| p.to_le_bytes()).collect()
}

// Spec "任务发布与取消": the three public kinds pass, training kinds are refused.
#[test]
fn public_kinds_pass_and_training_kinds_are_refused() {
    let cap = PublicParams::LIVE.price_cap;
    for kind in [JobKind::Eval, JobKind::DataClean, JobKind::Embed] {
        assert_eq!(check_spec(&spec(kind), cap), Ok(()));
    }
    for kind in [
        JobKind::Inference,
        JobKind::Pretrain,
        JobKind::Finetune,
        JobKind::Rl,
        JobKind::Storage,
    ] {
        let mut s = spec(JobKind::Eval);
        s.kind = kind;
        assert_eq!(check_spec(&s, cap), Err(SpecError::Kind));
    }
}

#[test]
fn spec_rules() {
    let cap = PublicParams::LIVE.price_cap;
    let mut s = spec(JobKind::DataClean);
    s.model = Some(ModelId([1; 32]));
    assert_eq!(check_spec(&s, cap), Err(SpecError::Model));
    let mut s = spec(JobKind::Eval);
    s.model = None;
    assert_eq!(check_spec(&s, cap), Err(SpecError::Model));
    let mut s = spec(JobKind::Eval);
    s.rules = 2;
    assert_eq!(check_spec(&s, cap), Err(SpecError::Rules));
    for units in [0, MAX_JOB_UNITS + 1] {
        let mut s = spec(JobKind::Eval);
        s.units = units;
        assert_eq!(check_spec(&s, cap), Err(SpecError::Units));
    }
    for price in [0, cap.0 + 1] {
        let mut s = spec(JobKind::Eval);
        s.price = MicroUsd(price);
        assert_eq!(check_spec(&s, cap), Err(SpecError::Price));
    }
    let mut s = spec(JobKind::Eval);
    s.price = cap;
    assert_eq!(check_spec(&s, cap), Ok(()));
    let mut s = spec(JobKind::Eval);
    s.results_url = Url::truncate_from(vec![0xff, 0xfe]);
    assert_eq!(check_spec(&s, cap), Err(SpecError::Url));
    let mut s = spec(JobKind::Eval);
    s.manifest_url = Url::default();
    assert_eq!(check_spec(&s, cap), Err(SpecError::Url));
}

#[test]
fn spec_round_trips() {
    let mut s = spec(JobKind::Embed);
    s.canary_root = Some([9; 32]);
    assert_eq!(JobSpec::decode(&mut &s.encode()[..]).unwrap(), s);
}

// Spec "参数与护栏" / "创世参数越界".
#[test]
fn params_guardrails() {
    assert_eq!(PublicParams::LIVE.check(), Ok(()));
    assert_eq!(PublicParams::DEV.check(), Ok(()));
    let p = PublicParams {
        units_per_round: 0,
        ..PublicParams::LIVE
    };
    assert_eq!(p.check(), Err(ParamsError::UnitsPerRound));
    let p = PublicParams {
        units_per_round: MAX_UNITS_PER_ROUND + 1,
        ..PublicParams::LIVE
    };
    assert_eq!(p.check(), Err(ParamsError::UnitsPerRound));
    let p = PublicParams {
        reveal_blocks: 0,
        ..PublicParams::LIVE
    };
    assert_eq!(p.check(), Err(ParamsError::ZeroBlocks));
    let p = PublicParams {
        challenge_epochs: 0,
        ..PublicParams::LIVE
    };
    assert_eq!(p.check(), Err(ParamsError::Challenge));
    let p = PublicParams {
        price_cap: MicroUsd(MAX_PRICE_CAP.0 + 1),
        ..PublicParams::LIVE
    };
    assert_eq!(p.check(), Err(ParamsError::PriceCap));
}

// Spec node/chain-spec "导出正式链公共任务参数".
#[test]
fn live_params_match_the_draft() {
    let p = PublicParams::LIVE;
    assert_eq!(
        (p.round_blocks, p.lock_blocks, p.price_cap),
        (600, 604_800, MicroUsd(1_000_000))
    );
}

#[test]
fn summary_formats() {
    let ok = |k, s: &[u8]| check_summary(k, RULES_V1, s);
    assert_eq!(ok(JobKind::Eval, &[0, 15, 3]), Ok(()));
    assert_eq!(ok(JobKind::Eval, &[]), Err(SummaryError::Length));
    assert_eq!(ok(JobKind::Eval, &[16]), Err(SummaryError::Choice));
    assert_eq!(ok(JobKind::Eval, &[0; 257]), Err(SummaryError::Length));
    assert_eq!(ok(JobKind::Embed, &[0; 8]), Ok(()));
    assert_eq!(ok(JobKind::Embed, &[0; 6]), Err(SummaryError::Length));
    assert_eq!(ok(JobKind::Embed, &[0; 1_028]), Err(SummaryError::Length));
    assert_eq!(ok(JobKind::DataClean, &[0; 32]), Ok(()));
    assert_eq!(ok(JobKind::DataClean, &[0; 31]), Err(SummaryError::Length));
    assert_eq!(ok(JobKind::Inference, &[0; 32]), Err(SummaryError::Kind));
    assert_eq!(
        check_summary(JobKind::DataClean, 2, &[0; 32]),
        Err(SummaryError::Kind)
    );
}

fn answers(n: usize, differ: usize) -> (Vec<u8>, Vec<u8>) {
    let a = vec![1u8; n];
    let mut b = a.clone();
    for x in b.iter_mut().take(differ) {
        *x = 2;
    }
    (eval(&a), eval(&b))
}

// Spec "摘要一致的判定" / "评测在容差内一致".
#[test]
fn eval_within_tolerance_agrees() {
    let (a, b) = answers(100, 2);
    assert!(agree(JobKind::Eval, RULES_V1, &a, &b));
}

// Spec "摘要一致的判定" / "评测超出容差".
#[test]
fn eval_beyond_tolerance_disagrees() {
    let (a, b) = answers(100, 3);
    assert!(!agree(JobKind::Eval, RULES_V1, &a, &b));
}

#[test]
fn small_units_tolerate_one_outlier_and_lengths_must_match() {
    let (a, b) = answers(10, 1);
    assert!(agree(JobKind::Eval, RULES_V1, &a, &b));
    let (a, b) = answers(10, 2);
    assert!(!agree(JobKind::Eval, RULES_V1, &a, &b));
    assert!(!agree(JobKind::Eval, RULES_V1, &[1, 1], &[1, 1, 1]));
}

// Spec "摘要一致的判定" / "嵌入指纹的汉明距离".
#[test]
fn embedding_fingerprints_tolerate_one_far_text_in_fifty() {
    let a: Vec<u32> = (0..50).map(|i| i * 0x0101_0101).collect();
    let mut b = a.clone();
    b[0] ^= 0b11111; // distance 5: one far text
    for x in b.iter_mut().skip(1) {
        *x ^= 0b111; // distance 3: within the tolerance
    }
    assert!(agree(JobKind::Embed, RULES_V1, &embed(&a), &embed(&b)));
    b[1] ^= 0b1000; // distance 4: a second far text
    assert!(!agree(JobKind::Embed, RULES_V1, &embed(&a), &embed(&b)));
}

// Spec "摘要一致的判定" / "数据清洗必须相等".
#[test]
fn data_cleaning_needs_equal_hashes() {
    let a = [5u8; 32];
    let mut b = a;
    b[31] ^= 1;
    assert!(agree(JobKind::DataClean, RULES_V1, &a, &a));
    assert!(!agree(JobKind::DataClean, RULES_V1, &a, &b));
}

fn settle(kind: JobKind, s: [Option<Vec<u8>>; 3]) -> Option<Majority> {
    let refs = [s[0].as_deref(), s[1].as_deref(), s[2].as_deref()];
    reference(kind, RULES_V1, &refs)
}

// Spec "单元结算" / "三人一致".
#[test]
fn three_agreeing_are_all_in_the_majority() {
    let s = Some(vec![1u8; 32]);
    let m = settle(JobKind::DataClean, [s.clone(), s.clone(), s]).unwrap();
    assert_eq!(m.reference, 0);
    assert_eq!(m.members, [true; 3]);
}

// Spec "单元结算" / "二对一".
#[test]
fn two_against_one() {
    let (a, b) = (Some(vec![1u8; 32]), Some(vec![2u8; 32]));
    let m = settle(JobKind::DataClean, [b, a.clone(), a]).unwrap();
    assert_eq!(m.reference, 1);
    assert_eq!(m.members, [false, true, true]);
}

// Spec "单元结算" / "无人一致则重开" and "超时关闭".
#[test]
fn no_two_agreeing_or_one_reveal_is_no_majority() {
    let s = |b: u8| Some(vec![b; 32]);
    assert_eq!(settle(JobKind::DataClean, [s(1), s(2), s(3)]), None);
    assert_eq!(settle(JobKind::DataClean, [None, s(1), None]), None);
    assert_eq!(settle(JobKind::DataClean, [None, None, None]), None);
}

#[test]
fn non_transitive_agreement_picks_the_middle() {
    // a ~ b and b ~ c but a ≁ c (100 items, tolerance 2).
    let a = vec![0u8; 100];
    let mut b = a.clone();
    b[0..2].fill(1);
    let mut c = a.clone();
    c[0..4].fill(1);
    let m = settle(JobKind::Eval, [Some(a), Some(b), Some(c)]).unwrap();
    assert_eq!(m.reference, 1);
    assert_eq!(m.members, [true, true, true]);
}

#[test]
fn commitment_binds_every_field() {
    let worker = [3u8; 32];
    let base = Reveal {
        job: 1,
        unit: 2,
        attempt: 1,
        worker: &worker,
        summary: &[1, 2, 3],
        result_hash: &[4; 32],
        salt: &[5; 32],
    };
    let c = commitment(&base);
    let other = [6u8; 32];
    let variants = [
        Reveal { job: 9, ..base },
        Reveal { unit: 9, ..base },
        Reveal { attempt: 2, ..base },
        Reveal {
            worker: &other,
            ..base
        },
        Reveal {
            summary: &[1, 2],
            ..base
        },
        Reveal {
            result_hash: &[0; 32],
            ..base
        },
        Reveal {
            salt: &[0; 32],
            ..base
        },
    ];
    for v in variants {
        assert_ne!(commitment(&v), c);
    }
    assert_eq!(commitment(&base), c);
}

fn leaves(n: u32) -> Vec<[u8; 32]> {
    (0..n)
        .map(|i| canary_leaf(0, i, &[i as u8; 32], &[7; 32]))
        .collect()
}

#[test]
fn canary_proofs_round_trip_for_every_tree_shape() {
    for n in 1..=17u32 {
        let l = leaves(n);
        let root = canary_root(&l).unwrap();
        for (i, leaf) in l.iter().enumerate() {
            let proof = canary_proof(&l, i).unwrap();
            assert!(verify_canary(&root, leaf, &proof), "n {n} i {i}");
        }
    }
    assert_eq!(canary_root(&[]), None);
    assert_eq!(canary_proof(&leaves(3), 3), None);
}

#[test]
fn tampered_canary_proofs_fail() {
    let l = leaves(5);
    let root = canary_root(&l).unwrap();
    let proof = canary_proof(&l, 2).unwrap();
    // Another leaf.
    assert!(!verify_canary(&root, &l[1], &proof));
    // A changed sibling.
    let mut bad = proof.clone();
    bad.siblings[0][0] ^= 1;
    assert!(!verify_canary(&root, &l[2], &bad));
    // A missing or extra sibling.
    let mut short = proof.clone();
    short.siblings.pop();
    assert!(!verify_canary(&root, &l[2], &short));
    let mut long = proof.clone();
    long.siblings.try_push([0; 32]).unwrap();
    assert!(!verify_canary(&root, &l[2], &long));
    // Another position or tree size.
    assert!(!verify_canary(
        &root,
        &l[2],
        &CanaryProof {
            index: 3,
            ..proof.clone()
        }
    ));
    assert!(!verify_canary(
        &root,
        &l[2],
        &CanaryProof {
            leaves: 2,
            ..proof.clone()
        }
    ));
    assert!(!verify_canary(
        &root,
        &l[2],
        &CanaryProof { index: 5, ..proof }
    ));
}

#[test]
fn assignment_respects_eligibility_and_is_deterministic() {
    let roster: Vec<u8> = (0..10).collect();
    let seed = H256([3; 32]);
    let a = assign_unit(&roster, &seed, (1, 2, 1), |w| *w % 2 == 0);
    assert_eq!(a.len(), 3);
    assert!(a.iter().all(|w| w % 2 == 0));
    assert_eq!(a, assign_unit(&roster, &seed, (1, 2, 1), |w| *w % 2 == 0));
    assert_ne!(
        assign_unit(&roster, &seed, (1, 2, 1), |_| true),
        assign_unit(&roster, &seed, (1, 2, 2), |_| true)
    );
    assert_eq!(assign_unit(&roster, &seed, (1, 2, 1), |w| *w < 2).len(), 2);
}

#[test]
fn per_worker_cap() {
    assert_eq!(max_per_worker(64, 1_000), 2);
    assert_eq!(max_per_worker(64, 10), 21);
    assert_eq!(max_per_worker(8, 0), 25);
}

#[test]
fn records_round_trip() {
    let u = UnitRecord::<[u8; 32], u32> {
        attempt: 2,
        opened_at: 10,
        commit_by: 30,
        reveal_by: 40,
        assigned: [[1; 32], [2; 32], [3; 32]],
        tried: BoundedVec::truncate_from(vec![[9; 32]]),
        commits: [Some(H256([4; 32])), None, None],
        reveals: [
            Some((Summary::truncate_from(vec![1, 2]), [5; 32])),
            None,
            None,
        ],
        state: UnitState::Accepted {
            reference: 0,
            majority: [true, false, true],
        },
        settled_at: Some(41),
        canary_revealed: false,
    };
    assert_eq!(UnitRecord::decode(&mut &u.encode()[..]).unwrap(), u);
}

fn summary_strategy() -> impl Strategy<Value = Vec<u8>> {
    prop::collection::vec(0u8..4, 20)
}

proptest! {
    #[test]
    fn agree_is_symmetric_and_reflexive(a in summary_strategy(), b in summary_strategy()) {
        prop_assert!(agree(JobKind::Eval, RULES_V1, &a, &a));
        prop_assert_eq!(
            agree(JobKind::Eval, RULES_V1, &a, &b),
            agree(JobKind::Eval, RULES_V1, &b, &a)
        );
    }

    #[test]
    fn reference_is_deterministic_and_in_its_majority(
        s in prop::collection::vec(prop::option::of(summary_strategy()), 3)
    ) {
        let refs = [s[0].as_deref(), s[1].as_deref(), s[2].as_deref()];
        let m = reference(JobKind::Eval, RULES_V1, &refs);
        prop_assert_eq!(m, reference(JobKind::Eval, RULES_V1, &refs));
        if let Some(m) = m {
            let r = usize::from(m.reference);
            prop_assert!(m.members[r]);
            prop_assert!(m.members.iter().filter(|x| **x).count() >= 2);
            for (j, member) in m.members.iter().enumerate() {
                if *member && j != r {
                    prop_assert!(agree(JobKind::Eval, RULES_V1, refs[r].unwrap(), refs[j].unwrap()));
                }
            }
        }
    }
}
