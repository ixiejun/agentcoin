//! Unit tests (spec consensus/staking).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use ac_primitives::staking::AccountStake;
use frame_support::traits::Get;
use frame_support::traits::fungible::{Inspect, InspectHold, Mutate};
use frame_support::traits::tokens::Preservation;
use frame_support::{assert_noop, assert_ok};
use proptest::prelude::*;

use crate::mock::{
    ACCOUNTS, Balances, ENDOWMENT, ISSUANCE, MIN_NOM, MIN_SELF, RuntimeHoldReason, RuntimeOrigin,
    StakingPos, System, Test, ext, key_and_proof, validator,
};
use crate::{Candidates, Error, HoldReason, KeyOwner, Ledger, Nominators, TotalActive};

fn origin(who: u64) -> RuntimeOrigin {
    RuntimeOrigin::signed(who)
}

fn register(
    who: u64,
    seed: u8,
    value: u128,
) -> sp_runtime::DispatchResultWithInfo<frame_support::dispatch::PostDispatchInfo> {
    let (key, proof) = key_and_proof(who, seed);
    StakingPos::register_candidate(origin(who), key, proof, value, 1_000)
}

fn held(who: u64) -> u128 {
    let reason: RuntimeHoldReason = HoldReason::Staking.into();
    Balances::balance_on_hold(&reason, &who)
}

fn run_to(n: u64) {
    System::set_block_number(n);
}

// Scenario "注册候选人".
#[test]
fn register_candidate() {
    ext().execute_with(|| {
        assert_eq!(Balances::total_issuance(), ISSUANCE);
        assert_ok!(register(1, 1, MIN_SELF));
        assert!(Candidates::<Test>::contains_key(1));
        assert_eq!(held(1), MIN_SELF);
        assert_eq!(StakingPos::active(&1), MIN_SELF);
        assert_eq!(TotalActive::<Test>::get(), MIN_SELF);
        // The self-stake cannot be transferred.
        assert!(
            <Balances as Mutate<u64>>::transfer(
                &1,
                &2,
                ENDOWMENT - MIN_SELF + 1,
                Preservation::Expendable
            )
            .is_err()
        );
        assert_eq!(Balances::total_issuance(), ISSUANCE);
    });
}

// Scenario "公钥被占用".
#[test]
fn key_in_use() {
    ext().execute_with(|| {
        assert_ok!(register(1, 1, MIN_SELF));
        assert_noop!(register(2, 1, MIN_SELF), Error::<Test>::KeyInUse);
        // A retired candidate's key stays taken.
        assert_ok!(StakingPos::retire(origin(1)));
        assert_noop!(register(3, 1, MIN_SELF), Error::<Test>::KeyInUse);
    });
}

// Scenario "缺少持有证明".
#[test]
fn missing_proof_of_possession() {
    ext().execute_with(|| {
        // Signed for another account.
        let (key, proof) = key_and_proof(2, 1);
        assert_noop!(
            StakingPos::register_candidate(origin(1), key.clone(), proof, MIN_SELF, 1_000),
            Error::<Test>::BadProof
        );
        // Signed by another key.
        let (_, other_proof) = key_and_proof(1, 2);
        assert_noop!(
            StakingPos::register_candidate(origin(1), key, other_proof, MIN_SELF, 1_000),
            Error::<Test>::BadProof
        );
    });
}

#[test]
fn validator_keys_must_be_ml_dsa_65() {
    ext().execute_with(|| {
        let key = ac_crypto::sig::SigningKey::from_seed(
            ac_crypto::SigAlg::MlDsa44,
            &ac_crypto::dev_seed("small").unwrap(),
        )
        .unwrap();
        let public = key.public_key().unwrap();
        let statement =
            ac_primitives::staking::pop_statement(&System::block_hash(0), &1u64, &public);
        let proof = key
            .sign_deterministic(&statement, ac_primitives::staking::VALIDATOR_POP_CONTEXT)
            .unwrap();
        assert_noop!(
            StakingPos::register_candidate(origin(1), public, proof, MIN_SELF, 1_000),
            Error::<Test>::NotMlDsa65
        );
    });
}

// Scenario "低于最低自质押额".
#[test]
fn below_minimum_self_stake() {
    ext().execute_with(|| {
        assert_noop!(register(1, 1, MIN_SELF - 1), Error::<Test>::BelowMinimum);
        assert_noop!(
            register(1, 1, ENDOWMENT + 1),
            Error::<Test>::InsufficientBalance
        );
    });
}

#[test]
fn candidate_changes_key_and_retires() {
    ext().execute_with(|| {
        assert_ok!(register(1, 1, MIN_SELF));
        let (key, proof) = key_and_proof(1, 2);
        assert_ok!(StakingPos::set_validator_key(origin(1), key.clone(), proof));
        assert_eq!(Candidates::<Test>::get(1).unwrap().key, key);
        // Both keys stay owned by account 1.
        let old = validator(1).public_key().unwrap();
        assert_eq!(
            KeyOwner::<Test>::get(ac_primitives::staking::validator_key_id(&old)),
            Some(1)
        );
        assert_ok!(StakingPos::bond_extra(origin(1), 500));
        assert_eq!(StakingPos::active(&1), MIN_SELF + 500);
        assert_ok!(StakingPos::retire(origin(1)));
        assert!(!Candidates::<Test>::contains_key(1));
        assert_eq!(
            StakingPos::stake_of(&1),
            AccountStake::new(0, MIN_SELF + 500, 0)
        );
        assert_eq!(TotalActive::<Test>::get(), 0);
        // Still unbonding: cannot take another role.
        assert_noop!(
            StakingPos::nominate(origin(1), MIN_NOM, vec![2]),
            Error::<Test>::AlreadyStaking
        );
    });
}

// Scenario "提名多个候选人".
#[test]
fn nominate_several_candidates() {
    ext().execute_with(|| {
        for (who, seed) in [(1, 1), (2, 2), (3, 3)] {
            assert_ok!(register(who, seed, MIN_SELF));
        }
        assert_ok!(StakingPos::nominate(origin(10), 5_000, vec![1, 2, 3]));
        assert_eq!(Nominators::<Test>::get(10).unwrap().to_vec(), vec![1, 2, 3]);
        assert_eq!(held(10), 5_000);
        let info = StakingPos::candidate_info(&2).unwrap();
        assert_eq!(info.nominators, vec![(10, 5_000)]);
        assert_eq!(TotalActive::<Test>::get(), 3 * MIN_SELF + 5_000);
        // A candidate cannot also nominate, a nominator cannot also register.
        assert_noop!(
            StakingPos::nominate(origin(1), MIN_NOM, vec![2]),
            Error::<Test>::AlreadyStaking
        );
        assert_noop!(register(10, 9, MIN_SELF), Error::<Test>::AlreadyStaking);
    });
}

// Scenario "提名过多".
#[test]
fn too_many_or_bad_targets() {
    ext().execute_with(|| {
        let mut params = crate::mock::params();
        params.max_candidates = 20;
        crate::Params::<Test>::put(params);
        for who in 1..=17u64 {
            assert_ok!(register(who, who as u8, MIN_SELF));
        }
        let seventeen: Vec<u64> = (1..=17).collect();
        assert_noop!(
            StakingPos::nominate(origin(30), MIN_NOM, seventeen),
            Error::<Test>::BadTargets
        );
        assert_noop!(
            StakingPos::nominate(origin(30), MIN_NOM, vec![]),
            Error::<Test>::BadTargets
        );
        assert_noop!(
            StakingPos::nominate(origin(30), MIN_NOM, vec![1, 1]),
            Error::<Test>::BadTargets
        );
        assert_noop!(
            StakingPos::nominate(origin(30), MIN_NOM, vec![31]),
            Error::<Test>::BadTargets
        );
        assert_noop!(
            StakingPos::nominate(origin(30), MIN_NOM - 1, vec![1]),
            Error::<Test>::BelowMinimum
        );
        let sixteen: Vec<u64> = (1..=16).collect();
        assert_ok!(StakingPos::nominate(origin(30), MIN_NOM, sixteen));
    });
}

// Scenario "更换提名对象".
#[test]
fn change_targets_without_unbonding() {
    ext().execute_with(|| {
        assert_ok!(register(1, 1, MIN_SELF));
        assert_ok!(register(2, 2, MIN_SELF));
        assert_ok!(StakingPos::nominate(origin(10), 1_000, vec![1]));
        assert_ok!(StakingPos::set_nominations(origin(10), vec![2]));
        assert_eq!(Nominators::<Test>::get(10).unwrap().to_vec(), vec![2]);
        assert_eq!(StakingPos::stake_of(&10), AccountStake::new(1_000, 0, 0));
        assert!(
            StakingPos::candidate_info(&1)
                .unwrap()
                .nominators
                .is_empty()
        );
        assert_eq!(
            StakingPos::candidate_info(&2).unwrap().nominators,
            vec![(10, 1_000)]
        );
    });
}

// Scenario "名单已满时挤出最小者" (nominators, then candidates).
#[test]
fn full_lists_evict_the_smallest() {
    ext().execute_with(|| {
        assert_ok!(register(1, 1, MIN_SELF));
        for (who, value) in [(10, 500), (11, 300), (12, 400), (13, 600), (14, 700)] {
            assert_ok!(StakingPos::nominate(origin(who), value, vec![1]));
        }
        assert_eq!(Nominators::<Test>::count(), 5);
        assert_noop!(
            StakingPos::nominate(origin(15), 300, vec![1]),
            Error::<Test>::ListFull
        );
        assert_ok!(StakingPos::nominate(origin(15), 301, vec![1]));
        assert_eq!(Nominators::<Test>::count(), 5);
        assert!(!Nominators::<Test>::contains_key(11));
        assert_eq!(StakingPos::stake_of(&11).unlocking, 300);

        for (who, seed, extra) in [(2, 2, 5), (3, 3, 1), (4, 4, 3), (5, 5, 4)] {
            assert_ok!(register(who, seed, MIN_SELF + extra));
        }
        assert_noop!(register(6, 6, MIN_SELF), Error::<Test>::ListFull);
        assert_ok!(register(6, 6, MIN_SELF + 2));
        assert_eq!(Candidates::<Test>::count(), 5);
        // Candidate 1 (exactly the minimum) was the smallest.
        assert!(!Candidates::<Test>::contains_key(1));
        assert_eq!(StakingPos::stake_of(&1).unlocking, MIN_SELF);
    });
}

// Scenario "发行量增长后低于最低额": the stake stays held and can be topped up.
#[test]
fn issuance_growth_raises_the_minimum() {
    ext().execute_with(|| {
        assert_ok!(register(1, 1, MIN_SELF));
        assert!(StakingPos::is_qualified(&1));
        // Issuance grows by 10%: the minimum becomes 11,000.
        let _ = <Balances as Mutate<u64>>::mint_into(&40, ISSUANCE / 10);
        assert!(!StakingPos::is_qualified(&1));
        assert_eq!(held(1), MIN_SELF);
        assert_ok!(StakingPos::bond_extra(origin(1), 1_000));
        assert!(StakingPos::is_qualified(&1));
    });
}

// Scenario "自质押解绑".
#[test]
fn self_stake_unbonds_after_the_fixed_period() {
    ext().execute_with(|| {
        assert_ok!(register(1, 1, MIN_SELF + 3_000));
        assert_noop!(
            StakingPos::unbond(origin(1), 3_001),
            Error::<Test>::BelowMinimum
        );
        assert_ok!(StakingPos::unbond(origin(1), 3_000));
        assert_eq!(StakingPos::active(&1), MIN_SELF);
        assert_eq!(TotalActive::<Test>::get(), MIN_SELF);
        run_to(1 + 27);
        assert_ok!(StakingPos::withdraw_unbonded(origin(1)));
        assert_eq!(held(1), MIN_SELF + 3_000);
        run_to(1 + 28);
        assert_ok!(StakingPos::withdraw_unbonded(origin(1)));
        assert_eq!(held(1), MIN_SELF);
    });
}

// Scenario "少量提名退出".
#[test]
fn small_nomination_exit_waits_the_minimum() {
    ext().execute_with(|| {
        assert_ok!(register(1, 1, 200_000));
        assert_ok!(StakingPos::nominate(origin(10), 1_000, vec![1]));
        assert_ok!(StakingPos::unbond(origin(10), 100));
        let ledger = Ledger::<Test>::get(10).unwrap();
        assert_eq!(ledger.unlocking[0].unlock_at, 1 + 2);
        // Stops counting at once.
        assert_eq!(TotalActive::<Test>::get(), 200_000 + 900);
        assert_noop!(
            StakingPos::unbond(origin(10), 850),
            Error::<Test>::BelowMinimum
        );
    });
}

// Scenario "大量提名同时退出".
#[test]
fn mass_nomination_exit_waits_longer() {
    ext().execute_with(|| {
        assert_ok!(register(1, 1, MIN_SELF));
        for who in 10..15u64 {
            assert_ok!(StakingPos::nominate(origin(who), 200_000, vec![1]));
        }
        let mut last = 0;
        for who in 10..15u64 {
            assert_ok!(StakingPos::unnominate(origin(who)));
            let unlock = Ledger::<Test>::get(who).unwrap().unlocking[0].unlock_at;
            assert!(unlock >= last);
            assert!(unlock <= 1 + 28);
            last = unlock;
        }
        assert!(last > 1 + 2);
        assert!(!Nominators::<Test>::contains_key(10));
    });
}

// Scenario "等待期内不能转账".
#[test]
fn unbonding_stake_cannot_be_transferred() {
    ext().execute_with(|| {
        assert_ok!(register(1, 1, MIN_SELF));
        assert_ok!(StakingPos::nominate(origin(10), ENDOWMENT - 10, vec![1]));
        assert_ok!(StakingPos::unnominate(origin(10)));
        assert!(
            <Balances as Mutate<u64>>::transfer(&10, &2, 100, Preservation::Expendable).is_err()
        );
        run_to(1 + 28);
        assert_ok!(StakingPos::withdraw_unbonded(origin(10)));
        assert!(!Ledger::<Test>::contains_key(10));
        assert_ok!(<Balances as Mutate<u64>>::transfer(
            &10,
            &2,
            100,
            Preservation::Expendable
        ));
    });
}

// Scenario "佣金低于下限".
#[test]
fn commission_out_of_range() {
    ext().execute_with(|| {
        let (key, proof) = key_and_proof(1, 1);
        assert_noop!(
            StakingPos::register_candidate(origin(1), key, proof, MIN_SELF, 400),
            Error::<Test>::CommissionOutOfRange
        );
        assert_ok!(register(1, 1, MIN_SELF));
        assert_noop!(
            StakingPos::set_commission(origin(1), 400),
            Error::<Test>::CommissionOutOfRange
        );
        assert_noop!(
            StakingPos::set_commission(origin(1), 10_001),
            Error::<Test>::CommissionOutOfRange
        );
        assert_ok!(StakingPos::set_commission(origin(1), 10_000));
    });
}

// Scenario "调高佣金延迟生效"; decreases take effect at the next epoch.
#[test]
fn commission_changes_take_effect_later() {
    ext().execute_with(|| {
        assert_ok!(register(1, 1, MIN_SELF));
        assert_ok!(StakingPos::set_commission(origin(1), 5_000));
        let info = StakingPos::candidate_info(&1).unwrap();
        assert_eq!(info.commission_bps, 1_000);
        assert_eq!(info.pending_commission, Some((5_000, 1 + 7)));
        run_to(7);
        assert_eq!(StakingPos::commission(&1), Some(1_000));
        run_to(8);
        assert_eq!(StakingPos::commission(&1), Some(5_000));
        // A decrease at block 13 takes effect at the next epoch boundary, block 21.
        run_to(13);
        assert_ok!(StakingPos::set_commission(origin(1), 800));
        run_to(20);
        assert_eq!(StakingPos::commission(&1), Some(5_000));
        run_to(21);
        assert_eq!(StakingPos::commission(&1), Some(800));
    });
}

#[test]
fn chill_and_validate() {
    ext().execute_with(|| {
        assert_ok!(register(1, 1, MIN_SELF));
        assert_noop!(StakingPos::validate(origin(1)), Error::<Test>::NotChilled);
        assert_ok!(StakingPos::chill(origin(1)));
        assert!(!StakingPos::is_qualified(&1));
        assert!(Candidates::<Test>::get(1).unwrap().chilled);
        assert_ok!(StakingPos::validate(origin(1)));
        assert!(StakingPos::is_qualified(&1));
        assert_noop!(StakingPos::chill(origin(2)), Error::<Test>::NotCandidate);
    });
}

#[test]
fn queries_and_minimums() {
    ext().execute_with(|| {
        assert_eq!(StakingPos::minimums(), (MIN_SELF, MIN_NOM));
        assert_eq!(StakingPos::stake_of(&1), AccountStake::default());
        assert!(StakingPos::candidate_info(&1).is_none());
    });
}

/// One random staking action.
#[derive(Clone, Debug)]
enum Action {
    Register(u64, u128),
    Nominate(u64, u128, u64),
    Retarget(u64, u64),
    BondExtra(u64, u128),
    Unbond(u64, u128),
    Unnominate(u64),
    Retire(u64),
    Withdraw(u64),
    Advance(u64),
}

fn action() -> impl Strategy<Value = Action> {
    let who = 1..=ACCOUNTS;
    prop_oneof![
        (who.clone(), 0u128..30_000).prop_map(|(w, v)| Action::Register(w, v)),
        (who.clone(), 0u128..30_000, 1u64..=8).prop_map(|(w, v, t)| Action::Nominate(w, v, t)),
        (who.clone(), 1u64..=8).prop_map(|(w, t)| Action::Retarget(w, t)),
        (who.clone(), 0u128..5_000).prop_map(|(w, v)| Action::BondExtra(w, v)),
        (who.clone(), 0u128..5_000).prop_map(|(w, v)| Action::Unbond(w, v)),
        who.clone().prop_map(Action::Unnominate),
        who.clone().prop_map(Action::Retire),
        who.prop_map(Action::Withdraw),
        (1u64..10).prop_map(Action::Advance),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    // Scenario "质押不改变发行量": after every step the issuance is unchanged, balances add up
    // to it, and holds equal the ledgers.
    #[test]
    fn staking_conserves_issuance(actions in proptest::collection::vec(action(), 1..60)) {
        ext().execute_with(|| {
            for action in actions {
                let _ = match action {
                    Action::Register(w, v) => register(w, w as u8, v).map(|_| ()).map_err(|e| e.error),
                    Action::Nominate(w, v, t) => StakingPos::nominate(origin(w), v, vec![t]).map(|_| ()).map_err(|e| e.error),
                    Action::Retarget(w, t) => StakingPos::set_nominations(origin(w), vec![t]),
                    Action::BondExtra(w, v) => StakingPos::bond_extra(origin(w), v),
                    Action::Unbond(w, v) => StakingPos::unbond(origin(w), v),
                    Action::Unnominate(w) => StakingPos::unnominate(origin(w)),
                    Action::Retire(w) => StakingPos::retire(origin(w)),
                    Action::Withdraw(w) => StakingPos::withdraw_unbonded(origin(w)),
                    Action::Advance(n) => {
                        run_to(System::block_number() + n);
                        Ok(())
                    }
                };
                prop_assert_eq!(Balances::total_issuance(), ISSUANCE);
                let sum: u128 = (1..=ACCOUNTS).map(|a| Balances::total_balance(&a)).sum();
                prop_assert_eq!(sum, ISSUANCE);
                let mut active = 0u128;
                for who in 1..=ACCOUNTS {
                    let ledger_total = Ledger::<Test>::get(who).map_or(0, |l| {
                        active += l.active;
                        l.total()
                    });
                    prop_assert_eq!(held(who), ledger_total);
                }
                prop_assert_eq!(TotalActive::<Test>::get(), active);
                prop_assert!(Candidates::<Test>::count() <= 5 && Nominators::<Test>::count() <= 5);
            }
            Ok(())
        })?;
    }
}

#[test]
fn genesis_writes_the_parameters() {
    ext().execute_with(|| {
        assert_eq!(crate::Params::<Test>::get(), crate::mock::params());
    });
}

#[test]
#[should_panic(expected = "invalid staking genesis parameters")]
fn invalid_genesis_parameters_fail() {
    let mut params = crate::mock::params();
    params.max_candidates = 501;
    let _ = crate::mock::ext_with(params);
}

#[test]
#[should_panic(expected = "invalid staking genesis parameters")]
fn inverted_unbonding_bounds_fail() {
    let mut params = crate::mock::params();
    params.nomination_unbond_min = 30;
    let _ = crate::mock::ext_with(params);
}

// Task 2.7: calls are charged the benchmarked weights (worst case before refunds).
#[test]
fn calls_use_benchmarked_weights() {
    use crate::WeightInfo;
    use frame_support::dispatch::GetDispatchInfo;
    ext().execute_with(|| {
        let (key, proof) = key_and_proof(1, 1);
        let call = crate::Call::<Test>::register_candidate {
            key,
            proof,
            value: MIN_SELF,
            commission_bps: 1_000,
        };
        let weight = call.get_dispatch_info().call_weight;
        assert_eq!(weight, <() as WeightInfo>::register_candidate(500));
        // Verifying an ML-DSA proof costs far more than the database accesses alone.
        assert!(weight.ref_time() > 500_000_000);
        let nominate = crate::Call::<Test>::nominate {
            value: MIN_NOM,
            targets: vec![1],
        };
        assert_eq!(
            nominate.get_dispatch_info().call_weight,
            <() as WeightInfo>::nominate(2_000)
        );
        for w in [
            <() as WeightInfo>::bond_extra(),
            <() as WeightInfo>::unbond(),
            <() as WeightInfo>::withdraw_unbonded(),
            <() as WeightInfo>::set_commission(),
        ] {
            assert!(w.ref_time() > 0 && w.proof_size() > 0);
        }
    });
}

// ---- Election (spec consensus/npos-election) ----

fn many_candidates(count: u64) {
    let mut params = crate::mock::params();
    params.max_candidates = 50;
    params.max_nominators = 50;
    crate::Params::<Test>::put(params);
    for who in 1..=count {
        assert_ok!(register(who, who as u8, MIN_SELF + u128::from(who)));
    }
}

// Scenario "选出 K 个验证人".
#[test]
fn elects_k_validators() {
    ext().execute_with(|| {
        many_candidates(30);
        let elected = StakingPos::elect(21, false);
        assert_eq!(elected.len(), 21);
        let info = StakingPos::last_election().unwrap();
        assert!(!info.preview);
        assert_eq!(info.elected.len(), 21);
        for v in &info.elected {
            assert!(Candidates::<Test>::contains_key(v.who));
        }
        // The 21 largest self-stakes win when nobody nominates.
        let mut winners: Vec<u64> = info.elected.iter().map(|v| v.who).collect();
        winners.sort();
        assert_eq!(winners, (10..=30).collect::<Vec<_>>());
    });
}

// Scenario "合格候选人不足 K 个".
#[test]
fn fewer_candidates_than_seats() {
    ext().execute_with(|| {
        many_candidates(15);
        assert_eq!(StakingPos::elect(21, false).len(), 15);
    });
}

// Scenario "暂停参选的候选人落选", and candidates below the minimum stay out.
#[test]
fn chilled_and_underbonded_candidates_are_not_elected() {
    ext().execute_with(|| {
        many_candidates(3);
        assert_ok!(StakingPos::bond_extra(origin(3), 100_000));
        assert_ok!(StakingPos::chill(origin(3)));
        let info_before = StakingPos::elect(3, false);
        assert_eq!(info_before.len(), 2);
        assert!(
            StakingPos::last_election()
                .unwrap()
                .elected
                .iter()
                .all(|v| v.who != 3)
        );
        // Issuance growth pushes candidate 1 below the minimum.
        let _ = <Balances as Mutate<u64>>::mint_into(&40, ISSUANCE / 10);
        assert_eq!(StakingPos::qualified_candidates(), 0);
        assert!(StakingPos::elect(3, false).is_empty());
    });
}

// Scenario "提名在多个当选者之间分摊".
#[test]
fn nominations_balance_backing() {
    ext().execute_with(|| {
        many_candidates(2);
        // Candidate 1 also has a large nominator of its own.
        assert_ok!(StakingPos::nominate(origin(20), 50_000, vec![1]));
        assert_ok!(StakingPos::nominate(origin(21), 1_000, vec![1, 2]));
        let info = {
            StakingPos::elect(2, false);
            StakingPos::last_election().unwrap()
        };
        let part = |validator: u64| {
            info.elected
                .iter()
                .find(|v| v.who == validator)
                .unwrap()
                .exposure
                .iter()
                .find(|(n, _)| *n == 21)
                .map_or(0, |(_, v)| *v)
        };
        assert!(part(2) > part(1));
        assert_eq!(part(1) + part(2), 1_000);
        for v in &info.elected {
            let sum: u128 = v.exposure.iter().map(|(_, x)| x).sum();
            assert_eq!(v.backing, sum);
            // Self-stake is part of the backing.
            assert!(v.exposure.contains(&(v.who, StakingPos::active(&v.who))));
        }
        let total: u128 = info.elected.iter().map(|v| v.backing).sum();
        assert_eq!(total, TotalActive::<Test>::get());
    });
}

// Unbonding stake and nominations below the minimum do not count.
#[test]
fn election_uses_active_stake_only() {
    ext().execute_with(|| {
        many_candidates(1);
        assert_ok!(StakingPos::nominate(origin(20), 5_000, vec![1]));
        assert_ok!(StakingPos::unbond(origin(20), 4_000));
        StakingPos::elect(1, false);
        let v = &StakingPos::last_election().unwrap().elected[0];
        assert_eq!(v.backing, MIN_SELF + 1 + 1_000);
        // Issuance growth by 50% raises the minimum nomination to 150 > nothing here, but a
        // nomination of 120 falls out.
        assert_ok!(StakingPos::nominate(origin(21), 120, vec![1]));
        let _ = <Balances as Mutate<u64>>::mint_into(&40, ISSUANCE / 2);
        assert_ok!(StakingPos::bond_extra(origin(1), 10_000));
        StakingPos::elect(1, false);
        let v = &StakingPos::last_election().unwrap().elected[0];
        assert!(v.exposure.iter().all(|(n, _)| *n != 21));
    });
}

// Deterministic, and the same winners as the algorithm run directly.
#[test]
fn election_is_deterministic_and_matches_the_reference() {
    ext().execute_with(|| {
        many_candidates(12);
        for (n, targets) in [
            (20u64, vec![1u64, 5, 9]),
            (21, vec![2, 3]),
            (22, vec![12, 1]),
        ] {
            assert_ok!(StakingPos::nominate(origin(n), 4_000 + n as u128, targets));
        }
        let first = StakingPos::elect(6, false);
        let second = StakingPos::elect(6, false);
        assert_eq!(first, second);

        let candidates: Vec<u64> = (1..=12).collect();
        let mut voters: Vec<(u64, u64, Vec<u64>)> = candidates
            .iter()
            .map(|c| (*c, (MIN_SELF + u128::from(*c)) as u64, vec![*c]))
            .collect();
        voters.push((20, 4_020, vec![1, 5, 9]));
        voters.push((21, 4_021, vec![2, 3]));
        voters.push((22, 4_022, vec![12, 1]));
        let reference = sp_npos_elections::seq_phragmen::<u64, sp_runtime::Perbill>(
            6,
            candidates,
            voters,
            Some(sp_npos_elections::BalancingConfig {
                iterations: 10,
                tolerance: 0,
            }),
        )
        .unwrap();
        let ours: Vec<u64> = StakingPos::last_election()
            .unwrap()
            .elected
            .iter()
            .map(|v| v.who)
            .collect();
        let theirs: Vec<u64> = reference.winners.iter().map(|(w, _)| *w).collect();
        assert_eq!(ours, theirs);
    });
}

// Scenario "调整 K 不需要迁移": the same storage holds up to 1,000 winners.
#[test]
fn seats_are_a_parameter() {
    ext().execute_with(|| {
        many_candidates(30);
        assert_eq!(StakingPos::elect(100, false).len(), 30);
        assert_eq!(StakingPos::elect(200, true).len(), 30);
        assert!(StakingPos::last_election().unwrap().preview);
        let bound: u32 = <crate::mock::Test as crate::Config>::MaxWinners::get();
        assert_eq!(bound, 1_000);
    });
}

// Scenario "查询选举结果" / "查询预演结果".
#[test]
fn election_queries() {
    ext().execute_with(|| {
        assert!(StakingPos::last_election().is_none());
        many_candidates(2);
        assert_ok!(StakingPos::nominate(origin(20), 3_000, vec![2]));
        StakingPos::elect(2, true);
        let info = StakingPos::last_election().unwrap();
        assert!(info.preview);
        assert_eq!(info.block, 1);
        let two = info.elected.iter().find(|v| v.who == 2).unwrap();
        assert_eq!(two.backing, MIN_SELF + 2 + 3_000);
        assert_eq!(two.key, validator(2).public_key().unwrap());
    });
}

/// Writes `candidates` candidates and `nominators` nominators with 16 targets each directly to
/// storage (one shared key: the election only reads it).
fn populate(candidates: u64, nominators: u64) {
    use crate::{StakingLedger, Targets};
    let key = validator(1).public_key().unwrap();
    let mut total = 0u128;
    for who in 1..=candidates {
        let active = MIN_SELF + u128::from(who) * 7;
        Ledger::<Test>::insert(
            who,
            StakingLedger {
                active,
                ..Default::default()
            },
        );
        total += active;
        Candidates::<Test>::insert(
            who,
            ac_primitives::staking::CandidateRecord {
                key: key.clone(),
                commission_bps: 1_000,
                pending_commission: None,
                chilled: false,
            },
        );
    }
    for n in 0..nominators {
        let who = 100_000 + n;
        let active = MIN_NOM + u128::from(n) * 13;
        Ledger::<Test>::insert(
            who,
            StakingLedger {
                active,
                ..Default::default()
            },
        );
        total += active;
        let targets: Vec<u64> = (0..16)
            .map(|i| 1 + (n * 31 + i * 17) % candidates)
            .collect();
        let mut unique = targets.clone();
        unique.sort();
        unique.dedup();
        Nominators::<Test>::insert(who, Targets::<Test>::truncate_from(unique));
    }
    TotalActive::<Test>::put(total);
}

// Task 3.4: a reduced-scale election (100 candidates, 400 nominators × 16 edges) completes and
// never assigns more than the stake that took part.
#[test]
fn reduced_scale_election() {
    ext().execute_with(|| {
        populate(100, 400);
        let started = std::time::Instant::now();
        let elected = StakingPos::elect(50, false);
        let elapsed = started.elapsed();
        assert_eq!(elected.len(), 50);
        let backing: u128 = elected.iter().map(|(_, b)| b).sum();
        assert!(backing <= TotalActive::<Test>::get());
        // Every nominator's assigned parts add up to at most its stake.
        let info = StakingPos::last_election().unwrap();
        for n in 0..400u64 {
            let who = 100_000 + n;
            let assigned: u128 = info
                .elected
                .iter()
                .flat_map(|v| v.exposure.iter())
                .filter(|(x, _)| *x == who)
                .map(|(_, v)| v)
                .sum();
            assert!(assigned <= StakingPos::active(&who));
        }
        println!("reduced-scale election took {elapsed:?}");
    });
}

// ---- Rewards (spec economics/validator-rewards, economics/emission) ----

use crate::mock::{burned, set_author, set_missed, set_phase};
use ac_primitives::emission::{Phase, SecurityBudget};
use ac_primitives::offences::OffenceKind;
use ac_primitives::staking::{ChainPhase, validator_key_id};
use ac_primitives::validator_set::SlashHandler;
use frame_support::traits::Hooks;

fn key_id(seed: u8) -> [u8; 32] {
    validator_key_id(&validator(seed).public_key().unwrap())
}

/// Runs `blocks` blocks authored by validator `seed` (on_initialize only).
fn author_blocks(seed: u8, blocks: u64) {
    set_author(Some(validator(seed).public_key().unwrap()));
    for _ in 0..blocks {
        let n = System::block_number() + 1;
        System::set_block_number(n);
        StakingPos::on_initialize(n);
    }
    set_author(None);
}

/// Moves to the next epoch boundary and runs its hook with `missed` reveals.
fn next_boundary(missed: Vec<[u8; 32]>) {
    let n = System::block_number();
    let next = (n.saturating_sub(1) / crate::mock::EPOCH + 1) * crate::mock::EPOCH + 1;
    System::set_block_number(next);
    set_missed(missed);
    StakingPos::on_initialize(next);
    set_missed(Vec::new());
}

/// Two candidates (seeds 1, 2) elected, PoS phase.
fn pos_with_two_validators() {
    assert_ok!(register(1, 1, 50_000));
    assert_ok!(register(2, 2, 10_000));
    StakingPos::elect(2, false);
    set_phase(ChainPhase::Pos);
}

fn allotted_to(validator: u64) -> u128 {
    let mut total = 0;
    for i in crate::PayoutHead::<Test>::get()..crate::PayoutTail::<Test>::get() {
        if let Some((who, amount)) = crate::Payouts::<Test>::get(i)
            && who == validator
        {
            total += amount;
        }
    }
    total
}

// Scenario "支撑额不同、工作量相同".
#[test]
fn equal_work_equal_share() {
    ext().execute_with(|| {
        pos_with_two_validators();
        author_blocks(1, 5);
        author_blocks(2, 5);
        next_boundary(vec![]);
        let paid = <StakingPos as SecurityBudget<u64>>::recipients(1_000);
        assert_eq!(paid, vec![(StakingPos::reward_pot(), 1_000)]);
        assert_eq!(allotted_to(1), 500);
        assert_eq!(allotted_to(2), 500);
    });
}

// Scenario "漏块少分".
#[test]
fn missed_blocks_earn_less() {
    ext().execute_with(|| {
        pos_with_two_validators();
        author_blocks(1, 6);
        author_blocks(2, 3);
        next_boundary(vec![]);
        let _ = <StakingPos as SecurityBudget<u64>>::recipients(900);
        assert_eq!(allotted_to(1), 600);
        assert_eq!(allotted_to(2), 300);
    });
}

// Scenario "未揭示的纪元不计分" and chain/randomness "PoS 阶段未揭示扣分".
#[test]
fn missed_reveal_zeroes_the_epoch() {
    ext().execute_with(|| {
        pos_with_two_validators();
        author_blocks(1, 4);
        author_blocks(2, 4);
        next_boundary(vec![key_id(1)]);
        assert_eq!(crate::PendingPoints::<Test>::get(1), 0);
        assert_eq!(crate::PendingPoints::<Test>::get(2), 4);
        let _ = <StakingPos as SecurityBudget<u64>>::recipients(800);
        assert_eq!(allotted_to(1), 0);
        assert_eq!(allotted_to(2), 800);
    });
}

// Scenario "佣金与按比例分配".
#[test]
fn commission_then_backing() {
    ext().execute_with(|| {
        assert_ok!(register(1, 1, 10_000));
        assert_ok!(StakingPos::nominate(origin(20), 20_000, vec![1]));
        StakingPos::elect(1, false);
        set_phase(ChainPhase::Pos);
        author_blocks(1, 3);
        next_boundary(vec![]);
        let _ = <StakingPos as SecurityBudget<u64>>::recipients(100_000);
        // 10% commission, then 1/3 to the validator's stake and 2/3 to the nominator.
        assert_eq!(allotted_to(1), 10_000 + 30_000);
        assert_eq!(allotted_to(20), 60_000);
    });
}

// Scenarios "自动发放", "奖励总额守恒" and economics/emission "切换后开始发放".
#[test]
fn rewards_are_paid_automatically() {
    ext().execute_with(|| {
        assert_ok!(register(1, 1, 10_000));
        for n in 20..24u64 {
            assert_ok!(StakingPos::nominate(origin(n), 5_000, vec![1]));
        }
        StakingPos::elect(1, false);
        set_phase(ChainPhase::Pos);
        author_blocks(1, 2);
        next_boundary(vec![]);
        let budget = 30_000u128;
        let paid = <StakingPos as SecurityBudget<u64>>::recipients(budget);
        let (pot, allotted) = paid[0];
        assert!(allotted <= budget);
        // Emission mints the allotment into the pot.
        let _ = <Balances as Mutate<u64>>::mint_into(&pot, allotted);
        let issuance = Balances::total_issuance();
        let before: Vec<u128> = (20..24u64).map(Balances::free_balance).collect();
        // Five payouts, two per block: done within three blocks, without transactions.
        for _ in 0..3 {
            let n = System::block_number() + 1;
            System::set_block_number(n);
            StakingPos::on_initialize(n);
            assert_eq!(Balances::total_issuance(), issuance);
        }
        assert_eq!(
            crate::PayoutHead::<Test>::get(),
            crate::PayoutTail::<Test>::get()
        );
        for (i, n) in (20..24u64).enumerate() {
            assert!(Balances::free_balance(n) > before[i]);
        }
        assert_eq!(Balances::free_balance(pot), 0);
    });
}

// Scenario "无工作分时滚存" and PoA: nothing is paid.
#[test]
fn nothing_paid_without_points_or_in_poa() {
    ext().execute_with(|| {
        pos_with_two_validators();
        assert!(<StakingPos as SecurityBudget<u64>>::recipients(1_000).is_empty());
        set_phase(ChainPhase::Poa);
        assert_eq!(<StakingPos as SecurityBudget<u64>>::phase(), Phase::Poa);
        author_blocks(1, 5);
        next_boundary(vec![]);
        assert!(<StakingPos as SecurityBudget<u64>>::recipients(1_000).is_empty());
        assert_eq!(crate::PendingPoints::<Test>::get(1), 0);
    });
}

// Scenario "连续 3 次不揭示" and "PoA 阶段不处罚".
#[test]
fn three_missed_reveals_chill() {
    ext().execute_with(|| {
        pos_with_two_validators();
        // PoA: missing reveals changes nothing.
        set_phase(ChainPhase::Poa);
        for _ in 0..5 {
            next_boundary(vec![key_id(1)]);
        }
        assert!(!Candidates::<Test>::get(1).unwrap().chilled);
        set_phase(ChainPhase::Pos);
        next_boundary(vec![key_id(1)]);
        next_boundary(vec![key_id(1)]);
        assert!(!Candidates::<Test>::get(1).unwrap().chilled);
        // A revealed epoch in between resets the streak.
        author_blocks(1, 1);
        next_boundary(vec![]);
        next_boundary(vec![key_id(1)]);
        next_boundary(vec![key_id(1)]);
        assert!(!Candidates::<Test>::get(1).unwrap().chilled);
        next_boundary(vec![key_id(1)]);
        assert!(Candidates::<Test>::get(1).unwrap().chilled);
        // Not elected until it asks to validate again.
        let elected = StakingPos::elect(2, false);
        assert_eq!(elected.len(), 1);
        assert_ok!(StakingPos::validate(origin(1)));
        assert_eq!(StakingPos::elect(2, false).len(), 2);
    });
}

// ---- Slashing (spec consensus/offences "罚没验证人自质押并销毁") ----

fn offender() -> ac_crypto::PqPublicKey {
    validator(1).public_key().unwrap()
}

// Scenario "投票双签罚没全部自质押".
#[test]
fn vote_double_signing_burns_all_self_stake() {
    ext().execute_with(|| {
        assert_ok!(register(1, 1, 12_000));
        assert_ok!(StakingPos::unbond(origin(1), 2_000));
        assert_ok!(StakingPos::nominate(origin(20), 3_000, vec![1]));
        let issuance = Balances::total_issuance();
        let slashed = <StakingPos as SlashHandler>::on_offence(
            &offender(),
            OffenceKind::BftEquivocation,
            &[],
        );
        assert_eq!(slashed, 12_000);
        assert_eq!(burned(), 12_000);
        assert_eq!(Balances::total_issuance(), issuance - 12_000);
        assert_eq!(held(1), 0);
        assert!(!Ledger::<Test>::contains_key(1));
        // Nominators keep everything.
        assert_eq!(held(20), 3_000);
        assert_eq!(TotalActive::<Test>::get(), 3_000);
        assert!(Candidates::<Test>::get(1).unwrap().chilled);
    });
}

// Scenario "出块双签罚没 10%".
#[test]
fn seal_double_signing_burns_ten_percent() {
    ext().execute_with(|| {
        assert_ok!(register(1, 1, 10_000));
        let slashed = <StakingPos as SlashHandler>::on_offence(
            &offender(),
            OffenceKind::AuraEquivocation,
            &[],
        );
        assert_eq!(slashed, 1_000);
        assert_eq!(StakingPos::active(&1), 9_000);
    });
}

// Scenario "同一集合内先出块双签后投票双签".
#[test]
fn most_severe_kind_without_adding_up() {
    ext().execute_with(|| {
        assert_ok!(register(1, 1, 10_000));
        let first = <StakingPos as SlashHandler>::on_offence(
            &offender(),
            OffenceKind::AuraEquivocation,
            &[],
        );
        let second = <StakingPos as SlashHandler>::on_offence(
            &offender(),
            OffenceKind::BftEquivocation,
            &[OffenceKind::AuraEquivocation],
        );
        assert_eq!((first, second), (1_000, 9_000));
        // A repeated record of the same kind takes nothing more.
        let third = <StakingPos as SlashHandler>::on_offence(
            &offender(),
            OffenceKind::AuraEquivocation,
            &[OffenceKind::AuraEquivocation, OffenceKind::BftEquivocation],
        );
        assert_eq!(third, 0);
        assert_eq!(burned(), 10_000);
    });
}

// Scenario "PoA 授权节点违规": no stake, nothing slashed.
#[test]
fn poa_authority_without_stake_loses_nothing() {
    ext().execute_with(|| {
        let issuance = Balances::total_issuance();
        let slashed = <StakingPos as SlashHandler>::on_offence(
            &validator(9).public_key().unwrap(),
            OffenceKind::BftEquivocation,
            &[],
        );
        assert_eq!(slashed, 0);
        assert_eq!(Balances::total_issuance(), issuance);
    });
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    // Slashing conservation: issuance decrease = burned = self-stake decrease.
    #[test]
    fn slashing_conserves(stake in MIN_SELF..200_000u128, unbond in 0u128..100_000, aura_first in any::<bool>()) {
        ext().execute_with(|| {
            register(1, 1, stake).unwrap();
            let unbond = unbond.min(stake - MIN_SELF);
            if unbond > 0 {
                StakingPos::unbond(origin(1), unbond).unwrap();
            }
            let issuance = Balances::total_issuance();
            let before = held(1);
            let kinds = if aura_first {
                vec![OffenceKind::AuraEquivocation, OffenceKind::BftEquivocation]
            } else {
                vec![OffenceKind::BftEquivocation]
            };
            let mut prior = Vec::new();
            let mut total = 0;
            for kind in kinds {
                total += <StakingPos as SlashHandler>::on_offence(&offender(), kind, &prior);
                prior.push(kind);
            }
            prop_assert_eq!(total, stake);
            prop_assert_eq!(burned(), total);
            prop_assert_eq!(issuance - Balances::total_issuance(), total);
            prop_assert_eq!(before - held(1), total);
            Ok(())
        })?;
    }
}
