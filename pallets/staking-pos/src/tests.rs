//! Unit tests (spec consensus/staking).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use ac_primitives::staking::AccountStake;
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
