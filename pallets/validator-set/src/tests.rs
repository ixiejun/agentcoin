//! Unit tests (consensus/validator-set; consensus/aura-pq "纪元边界后按新列表出块").
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use ac_crypto::sig::SigningKey;
use ac_crypto::{PqPublicKey, SigAlg, dev_seed};
use ac_primitives::ac_bft::{Authority, ConsensusLog};
use ac_primitives::aura_pq::{Slot, pre_digest};
use ac_primitives::validator_set::ValidatorSetInterface;
use frame_support::traits::Hooks;
use sp_runtime::{BuildStorage, Digest};

use crate::mock::{AuraPq, RuntimeEvent, RuntimeGenesisConfig, System, Test, ValidatorSet};
use crate::{CurrentSetId, Event, PendingRemovals, SwitchParams};

const NAMES: [&str; 4] = ["alice", "bob", "charlie", "dave"];

fn key(name: &str) -> PqPublicKey {
    SigningKey::from_seed(SigAlg::MlDsa65, &dev_seed(name).unwrap())
        .unwrap()
        .public_key()
        .unwrap()
}

fn keys(n: usize) -> Vec<PqPublicKey> {
    NAMES[..n].iter().map(|n| key(n)).collect()
}

/// Switch parameters of the tests: 10% of the issuance, 3 candidates, height 40, 20 blocks.
fn switch_params() -> SwitchParams {
    SwitchParams {
        stake_bps: 1_000,
        min_candidates: 3,
        min_height: 40,
        sustain_blocks: 20,
    }
}

/// M2-era set-up: `K` = 1 and one qualified candidate suffices, but nothing is staked.
fn ext(authorities: Vec<PqPublicKey>, epoch_length: u64) -> sp_io::TestExternalities {
    ext_params(
        authorities,
        epoch_length,
        1,
        SwitchParams {
            min_candidates: 1,
            ..switch_params()
        },
    )
}

fn ext_with(
    authorities: Vec<PqPublicKey>,
    epoch_length: u64,
    validator_count: u32,
) -> sp_io::TestExternalities {
    ext_params(authorities, epoch_length, validator_count, switch_params())
}

fn ext_params(
    authorities: Vec<PqPublicKey>,
    epoch_length: u64,
    validator_count: u32,
    transition: SwitchParams,
) -> sp_io::TestExternalities {
    crate::mock::reset_staking();
    let config = RuntimeGenesisConfig {
        aura_pq: pallet_aura_pq::GenesisConfig {
            authorities,
            ..Default::default()
        },
        validator_set: crate::GenesisConfig {
            epoch_length,
            transition,
            validator_count,
            ..Default::default()
        },
        ..Default::default()
    };
    config.build_storage().unwrap().into()
}

/// Runs blocks up to `n` (one per slot), each in runtime hook order.
fn run_to(n: u64) {
    let mut next = System::block_number() + 1;
    while next <= n {
        block(next);
        next += 1;
    }
}

/// Initializes block `n` in slot `n` and runs the hooks in runtime order.
fn block(n: u64) {
    let mut digest = Digest::default();
    digest.push(pre_digest(Slot::from(n)));
    System::reset_events();
    System::initialize(&n, &Default::default(), &digest);
    AuraPq::on_initialize(n);
    ValidatorSet::on_initialize(n);
    ValidatorSet::on_finalize(n);
}

fn change_digest() -> Option<ac_primitives::ac_bft::ScheduledChange> {
    ConsensusLog::find_change(System::digest().logs())
}

// Scenario "查询集合": genesis set id 0, members in genesis order, weight 1.
#[test]
fn genesis_set() {
    ext(keys(4), 20).execute_with(|| {
        let (id, set) = ValidatorSet::authority_set();
        assert_eq!(id, 0);
        assert_eq!(
            set,
            keys(4).into_iter().map(Authority::poa).collect::<Vec<_>>()
        );
        assert!(set.iter().all(|a| a.weight == 1));
        assert_eq!(ValidatorSet::epoch_length(), 20);
        assert_eq!(ValidatorSet::historical_set(0), Some(set));
    });
}

// Scenario "纪元过短": 4 authorities need an epoch of at least 8 blocks.
#[test]
#[should_panic(expected = "epoch length must be at least 8")]
fn epoch_too_short() {
    let _ = ext(keys(4), 7);
}

// Scenario "变更摘要" and "下一纪元生效" (runtime part): a disabled validator leaves the set at
// the next boundary, which announces the new set; the next block uses the new list.
#[test]
fn boundary_enacts_removal() {
    ext(keys(4), 8).execute_with(|| {
        run_to(2);
        assert!(ValidatorSet::disable(&key("bob")));
        assert!(ValidatorSet::disable(&key("bob")), "idempotent");
        assert_eq!(PendingRemovals::<Test>::get().len(), 1);
        for n in 3..=8 {
            block(n);
            assert_eq!(change_digest(), None);
        }
        // Block 9 opens epoch 1.
        block(9);
        let change = change_digest().expect("set-change digest");
        let expected: Vec<Authority> = ["alice", "charlie", "dave"]
            .iter()
            .map(|n| Authority::poa(key(n)))
            .collect();
        assert_eq!(change.set_id, 1);
        assert_eq!(change.authorities.to_vec(), expected);
        assert_eq!(ValidatorSet::authority_set(), (1, expected.clone()));
        assert_eq!(
            AuraPq::authorities(),
            ac_primitives::ac_bft::keys(&expected)
        );
        let events: Vec<_> = System::events().into_iter().map(|e| e.event).collect();
        assert!(
            events.contains(&RuntimeEvent::ValidatorSet(Event::AuthorityDisabled {
                key: key("bob")
            }))
        );
        assert!(events.contains(&RuntimeEvent::ValidatorSet(Event::NewSet {
            set_id: 1,
            size: 3
        })));
        assert!(PendingRemovals::<Test>::get().is_empty());
    });
}

// Task 3.3 / Scenario "纪元边界后按新列表出块": the boundary block's author comes from the old
// list, the following block's from the new one.
#[test]
fn current_author_uses_list_in_force() {
    ext(keys(4), 8).execute_with(|| {
        run_to(1);
        ValidatorSet::disable(&key("alice"));
        // Boundary block 9, slot 9, under the old list [alice, bob, charlie, dave]: bob.
        run_to(9);
        assert_eq!(ValidatorSet::authority_set().0, 1);
        assert_eq!(
            pallet_aura_pq::CurrentAuthor::<Test>::get(),
            Some(key("bob"))
        );
        // Slot 12 tells the lists apart: bob under the new list [bob, charlie, dave]
        // (12 mod 3 = 0), alice under the old one (12 mod 4 = 0).
        run_to(12);
        assert_eq!(
            pallet_aura_pq::CurrentAuthor::<Test>::get(),
            Some(key("bob"))
        );
    });
}

// Scenario "无变更无摘要".
#[test]
fn boundary_without_offences_changes_nothing() {
    ext(keys(4), 8).execute_with(|| {
        for n in 1..=17 {
            block(n);
            assert_eq!(change_digest(), None);
        }
        assert_eq!(CurrentSetId::<Test>::get(), 0);
    });
}

// consensus/offences Scenario "不会清空集合" (runtime part).
#[test]
fn never_empties_the_set() {
    ext(keys(1), 2).execute_with(|| {
        run_to(1);
        assert!(ValidatorSet::disable(&key("alice")));
        run_to(3);
        assert_eq!(
            ValidatorSet::authority_set(),
            (0, vec![Authority::poa(key("alice"))])
        );
        assert_eq!(change_digest(), None);
    });
    // When every member offended, the first in set order stays.
    ext(keys(2), 4).execute_with(|| {
        run_to(1);
        ValidatorSet::disable(&key("bob"));
        ValidatorSet::disable(&key("alice"));
        run_to(5);
        assert_eq!(
            ValidatorSet::authority_set(),
            (1, vec![Authority::poa(key("alice"))])
        );
    });
}

// Only members of the current set can be disabled.
#[test]
fn disabling_a_non_member_does_nothing() {
    ext(keys(2), 4).execute_with(|| {
        run_to(1);
        assert!(!ValidatorSet::disable(&key("dave")));
        assert!(PendingRemovals::<Test>::get().is_empty());
    });
}

// Historical sets stay available for evidence during the retention window (2 epochs here).
#[test]
fn history_window() {
    ext(keys(4), 8).execute_with(|| {
        run_to(1);
        ValidatorSet::disable(&key("dave"));
        run_to(9); // epoch 1: set 1 = [alice, bob, charlie]
        assert!(ValidatorSet::historical_set(0).is_some());
        assert!(ValidatorSet::is_recent_member(&key("dave")));
        run_to(17); // epoch 2
        assert!(
            ValidatorSet::historical_set(0).is_some(),
            "set 0 ended at epoch 1 >= window start 0"
        );
        run_to(25); // epoch 3: window starts at epoch 1, set 0 ended when epoch 1 began
        assert_eq!(ValidatorSet::historical_set(0), None);
        assert!(!ValidatorSet::is_recent_member(&key("dave")));
        assert!(ValidatorSet::is_recent_member(&key("alice")));
        assert!(ValidatorSet::historical_set(1).is_some());
        assert_eq!(<ValidatorSet as ValidatorSetInterface>::current_epoch(), 3);
    });
}

// The hook never panics, including on a default genesis without authorities (epoch length 0).
#[test]
fn hooks_never_panic() {
    ext(Vec::new(), 0).execute_with(|| {
        run_to(10);
        assert_eq!(ValidatorSet::authority_set(), (0, Vec::new()));
    });
}

// ---- m3-pos: PoA roster, switch to PoS, PoS sets ----

use crate::mock::{RuntimeOrigin, elections, set_elected, set_stake};
use crate::{Error, Phase, PoaAuthorities, QualifiedSince, SwitchedAt, ValidatorCount};
use ac_primitives::staking::ChainPhase;
use frame_support::{assert_noop, assert_ok};

fn extra(name: &str) -> PqPublicKey {
    key(name)
}

/// Qualified: 10% staked, 3 candidates.
fn qualify() {
    set_stake(100, 1_000, 3);
}

fn has_event(e: Event<Test>) -> bool {
    System::events()
        .iter()
        .any(|r| r.event == RuntimeEvent::ValidatorSet(e.clone()))
}

// Scenario "多签增加授权节点".
#[test]
fn admin_adds_poa_authority_at_next_boundary() {
    ext_with(keys(3), 8, 3).execute_with(|| {
        run_to(2);
        let new = extra("eve");
        assert_ok!(ValidatorSet::add_poa_authority(
            RuntimeOrigin::root(),
            new.clone()
        ));
        assert!(PoaAuthorities::<Test>::get().contains(&new));
        // Not yet in the set.
        assert!(!ValidatorSet::authority_set().1.iter().any(|a| a.key == new));
        run_to(9);
        let change = change_digest().unwrap();
        assert_eq!(change.set_id, 1);
        assert!(
            change
                .authorities
                .iter()
                .any(|a| a.key == new && a.weight == 1)
        );
        assert_eq!(AuraPq::authorities().len(), 4);
    });
}

// Scenario "不能移空" and other rejected roster changes.
#[test]
fn roster_changes_are_checked() {
    ext_with(keys(1), 6, 3).execute_with(|| {
        run_to(1);
        assert_noop!(
            ValidatorSet::remove_poa_authority(RuntimeOrigin::root(), key("alice")),
            Error::<Test>::WouldEmpty
        );
        assert_noop!(
            ValidatorSet::add_poa_authority(RuntimeOrigin::root(), key("alice")),
            Error::<Test>::AlreadyAuthority
        );
        assert_noop!(
            ValidatorSet::add_poa_authority(RuntimeOrigin::signed(1), key("bob")),
            sp_runtime::DispatchError::BadOrigin
        );
        let small = SigningKey::from_seed(SigAlg::MlDsa44, &dev_seed("bob").unwrap())
            .unwrap()
            .public_key()
            .unwrap();
        assert_noop!(
            ValidatorSet::add_poa_authority(RuntimeOrigin::root(), small),
            Error::<Test>::NotMlDsa65
        );
        // An epoch of 6 blocks fits 3 authorities, not 4.
        assert_ok!(ValidatorSet::add_poa_authority(
            RuntimeOrigin::root(),
            key("bob")
        ));
        assert_ok!(ValidatorSet::add_poa_authority(
            RuntimeOrigin::root(),
            key("charlie")
        ));
        assert_noop!(
            ValidatorSet::add_poa_authority(RuntimeOrigin::root(), key("dave")),
            Error::<Test>::TooMany
        );
        assert_ok!(ValidatorSet::remove_poa_authority(
            RuntimeOrigin::root(),
            key("alice")
        ));
        assert_noop!(
            ValidatorSet::remove_poa_authority(RuntimeOrigin::root(), key("alice")),
            Error::<Test>::NotAuthority
        );
    });
}

// Scenario "集合规模超过纪元长度一半".
#[test]
fn validator_count_is_bounded_by_half_the_epoch() {
    ext_with(keys(4), 20, 3).execute_with(|| {
        assert_noop!(
            ValidatorSet::set_validator_count(RuntimeOrigin::root(), 11),
            Error::<Test>::BadValidatorCount
        );
        // Below the switch's candidate count.
        assert_noop!(
            ValidatorSet::set_validator_count(RuntimeOrigin::root(), 2),
            Error::<Test>::BadValidatorCount
        );
        assert_ok!(ValidatorSet::set_validator_count(RuntimeOrigin::root(), 10));
        assert_eq!(ValidatorCount::<Test>::get(), 10);
    });
}

#[test]
#[should_panic(expected = "validator count")]
fn genesis_count_below_the_switch_candidates_fails() {
    let _ = ext_with(keys(4), 20, 2);
}

// Scenarios "质押不足", "时间未到": checkpoints fail, nothing is recorded.
#[test]
fn checkpoints_before_the_conditions_hold() {
    ext_with(keys(4), 10, 3).execute_with(|| {
        qualify();
        // Height below 40.
        run_to(31);
        assert_eq!(QualifiedSince::<Test>::get(), None);
        assert!(has_event(Event::TransitionCheckpoint {
            qualified: false,
            since: None
        }));
        // 9% staked at height 41.
        set_stake(90, 1_000, 30);
        run_to(41);
        assert_eq!(QualifiedSince::<Test>::get(), None);
        assert_eq!(Phase::<Test>::get(), ChainPhase::Poa);
    });
}

// Scenarios "中途跌破重新计时", "连续保持后切换", "预演不影响 PoA", "切换后质押下降".
#[test]
fn sustained_conditions_switch_to_pos_for_good() {
    ext_with(keys(4), 10, 3).execute_with(|| {
        let elected: Vec<(PqPublicKey, u128)> = vec![
            (key("eve"), 3_000_000_000_000_000_000_000_000),
            (key("ferdie"), 1_000_000_000_000_000_000_000_000),
            (key("grace"), 1_000_000_000_000_000_000_000_000),
        ];
        set_elected(elected.clone());
        qualify();
        run_to(41);
        assert_eq!(QualifiedSince::<Test>::get(), Some(41));
        // Buffer: the last block of the epoch runs a preview; the PoA set stays.
        run_to(50);
        assert_eq!(elections(), vec![(3, true)]);
        run_to(51);
        assert_eq!(Phase::<Test>::get(), ChainPhase::Poa);
        assert_eq!(ValidatorSet::authority_set().1.len(), 4);
        // Drops below the threshold at block 61: the run restarts.
        set_stake(90, 1_000, 3);
        run_to(61);
        assert_eq!(QualifiedSince::<Test>::get(), None);
        qualify();
        run_to(71);
        assert_eq!(QualifiedSince::<Test>::get(), Some(71));
        run_to(81);
        assert_eq!(Phase::<Test>::get(), ChainPhase::Poa);
        // 71 + 20 = 91: switch.
        run_to(91);
        assert_eq!(Phase::<Test>::get(), ChainPhase::Pos);
        assert_eq!(SwitchedAt::<Test>::get(), Some(91));
        assert!(PoaAuthorities::<Test>::get().is_empty());
        assert!(has_event(Event::SwitchedToPos { block: 91 }));
        // The new set is exactly the elected validators, weighted by backing (3 : 1 : 1).
        let change = change_digest().unwrap();
        let set: Vec<(PqPublicKey, u64)> = change
            .authorities
            .iter()
            .map(|a| (a.key.clone(), a.weight))
            .collect();
        assert_eq!(set.len(), 3);
        assert_eq!(set[0].0, key("eve"));
        assert_eq!(set[0].1, 3 * set[1].1);
        assert_eq!(set[1].1, set[2].1);
        assert_eq!(
            AuraPq::authorities(),
            vec![key("eve"), key("ferdie"), key("grace")]
        );
        // Stake falls to 5%: still PoS, no checkpoints any more.
        set_stake(50, 1_000, 0);
        run_to(121);
        assert_eq!(Phase::<Test>::get(), ChainPhase::Pos);
        // Roster changes are refused in PoS (spec "PoS 阶段增删被拒绝").
        assert_noop!(
            ValidatorSet::add_poa_authority(RuntimeOrigin::root(), key("alice")),
            Error::<Test>::NotPoa
        );
        assert_noop!(
            ValidatorSet::remove_poa_authority(RuntimeOrigin::root(), key("eve")),
            Error::<Test>::NotPoa
        );
    });
}

// Scenarios "选举结果变化", "无变更无摘要", "PoS 阶段的权重".
#[test]
fn pos_boundaries_install_elections() {
    ext_with(keys(4), 10, 3).execute_with(|| {
        set_elected(vec![
            (key("eve"), 2_000_000_000_000_000_000),
            (key("ferdie"), 2_000_000_000_000_000_000),
            (key("grace"), 1_000_000_000_000_000_000),
        ]);
        qualify();
        run_to(61);
        assert_eq!(Phase::<Test>::get(), ChainPhase::Pos);
        let id = CurrentSetId::<Test>::get();
        // Same election: no digest, same id; the election ran in the last block (non-preview).
        run_to(71);
        assert!(change_digest().is_none());
        assert_eq!(CurrentSetId::<Test>::get(), id);
        assert_eq!(elections().last(), Some(&(3, false)));
        // Different weights: a new set.
        set_elected(vec![
            (key("eve"), 2_000_000_000_000_000_000),
            (key("ferdie"), 4_000_000_000_000_000_000),
            (key("grace"), 1_000_000_000_000_000_000),
        ]);
        run_to(81);
        let change = change_digest().unwrap();
        assert_eq!(change.set_id, id + 1);
        let weights: Vec<u64> = change.authorities.iter().map(|a| a.weight).collect();
        assert_eq!(weights, vec![2_000_000, 4_000_000, 1_000_000]);
        // A disabled validator is left out even if elected.
        assert!(ValidatorSet::disable(&key("grace")));
        run_to(91);
        let change = change_digest().unwrap();
        assert_eq!(change.authorities.len(), 2);
        assert!(has_event(Event::AuthorityDisabled { key: key("grace") }));
    });
}

// Scenario "查询切换进度".
#[test]
fn transition_progress_query() {
    ext_with(keys(4), 10, 3).execute_with(|| {
        set_stake(80, 1_000, 2);
        run_to(41);
        let p = ValidatorSet::transition_progress();
        assert_eq!(p.phase, ChainPhase::Poa);
        assert_eq!(p.total_active, 80);
        assert_eq!(p.stake_needed, 100);
        assert_eq!(p.qualified_candidates, 2);
        assert_eq!(p.params.min_candidates, 3);
        assert_eq!(p.params.min_height, 40);
        assert_eq!(p.height, 41);
        assert_eq!(p.qualified_since, None);
        assert_eq!(p.switched_at, None);
    });
}
