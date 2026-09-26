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
use crate::{CurrentSetId, Event, PendingRemovals};

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

fn ext(authorities: Vec<PqPublicKey>, epoch_length: u64) -> sp_io::TestExternalities {
    let config = RuntimeGenesisConfig {
        aura_pq: pallet_aura_pq::GenesisConfig {
            authorities,
            ..Default::default()
        },
        validator_set: crate::GenesisConfig {
            epoch_length,
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
