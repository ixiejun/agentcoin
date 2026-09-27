//! Unit tests (spec governance/poa-admin).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use frame_support::dispatch::GetDispatchInfo;
use frame_support::traits::EnsureOrigin;
use frame_support::{assert_noop, assert_ok};
use parity_scale_codec::Encode;
use sp_runtime::traits::Hash;
use sp_runtime::{BuildStorage, DispatchError};

use crate::mock::{
    ALICE, BOB, CHARLIE, Council, DAVE, MOTION, PoaAdmin, RuntimeCall, RuntimeEvent,
    RuntimeGenesisConfig, RuntimeOrigin, System, Test, ext,
};
use crate::{CouncilInstance, EnsureCouncilThreshold, Error, Event, MotionDuration, Threshold};

type CouncilOrigin = pallet_collective::RawOrigin<u64, CouncilInstance>;

fn members() -> Vec<u64> {
    pallet_collective::Members::<Test, CouncilInstance>::get()
}

fn members_origin(yes: u32) -> RuntimeOrigin {
    RuntimeOrigin::from(CouncilOrigin::Members(yes, 3))
}

/// Proposes `call` as `who` with motion threshold 2 and approves it; returns its hash and
/// index.
fn propose(who: u64, call: RuntimeCall) -> (<Test as frame_system::Config>::Hash, u32) {
    let len = u32::try_from(call.encoded_size()).unwrap();
    let hash = <Test as frame_system::Config>::Hashing::hash_of(&call);
    let index = pallet_collective::ProposalCount::<Test, CouncilInstance>::get();
    assert_ok!(Council::propose(
        RuntimeOrigin::signed(who),
        2,
        Box::new(call),
        len
    ));
    // The proposer approves explicitly; proposing is not a vote.
    assert_ok!(Council::vote(RuntimeOrigin::signed(who), hash, index, true));
    (hash, index)
}

fn close(
    hash: <Test as frame_system::Config>::Hash,
    index: u32,
    call: &RuntimeCall,
) -> frame_support::dispatch::DispatchResultWithPostInfo {
    Council::close(
        RuntimeOrigin::signed(ALICE),
        hash,
        index,
        call.get_dispatch_info().call_weight,
        u32::try_from(call.encoded_size()).unwrap(),
    )
}

fn set_threshold_call(t: u32) -> RuntimeCall {
    RuntimeCall::PoaAdmin(crate::Call::set_threshold { threshold: t })
}

fn admin_events() -> Vec<Event<Test>> {
    System::events()
        .into_iter()
        .filter_map(|e| match e.event {
            RuntimeEvent::PoaAdmin(ev) => Some(ev),
            _ => None,
        })
        .collect()
}

// The administration origin requires a motion with at least `Threshold` approvals.
#[test]
fn threshold_origin() {
    ext().execute_with(|| {
        assert_eq!(Threshold::<Test>::get(), 2);
        assert_eq!(MotionDuration::<Test>::get(), 20);
        assert!(EnsureCouncilThreshold::<Test>::try_origin(members_origin(2)).is_ok());
        assert!(EnsureCouncilThreshold::<Test>::try_origin(members_origin(3)).is_ok());
        assert!(EnsureCouncilThreshold::<Test>::try_origin(members_origin(1)).is_err());
        assert!(
            EnsureCouncilThreshold::<Test>::try_origin(RuntimeOrigin::from(CouncilOrigin::Member(
                ALICE
            )))
            .is_err()
        );
        assert!(EnsureCouncilThreshold::<Test>::try_origin(RuntimeOrigin::root()).is_err());
        assert!(EnsureCouncilThreshold::<Test>::try_origin(RuntimeOrigin::signed(ALICE)).is_err());
    });
}

// Scenario "达到门限后执行": a motion approved by two of three members runs once.
#[test]
fn motion_executes_at_threshold() {
    ext().execute_with(|| {
        let call = set_threshold_call(3);
        let (hash, index) = propose(ALICE, call.clone());
        assert_ok!(Council::vote(RuntimeOrigin::signed(BOB), hash, index, true));
        assert_ok!(close(hash, index, &call));
        assert_eq!(Threshold::<Test>::get(), 3);
        assert!(admin_events().contains(&Event::ThresholdSet { threshold: 3 }));
        // The motion is gone: closing again fails, so it ran exactly once.
        assert!(close(hash, index, &call).is_err());
    });
}

// Scenario "未达门限不执行": only the proposer approved; after the motion duration it closes as
// rejected (non-voters count as no) and nothing changes.
#[test]
fn motion_below_threshold_does_not_execute() {
    ext().execute_with(|| {
        let call = set_threshold_call(1);
        let (hash, index) = propose(ALICE, call.clone());
        assert!(close(hash, index, &call).is_err(), "too early to close");
        System::set_block_number(1 + MOTION + 1);
        assert_ok!(close(hash, index, &call));
        assert_eq!(Threshold::<Test>::get(), 2);
        assert!(admin_events().is_empty());
    });
}

// A member cannot get around the threshold by proposing with motion threshold 1: the call runs
// with one approval and the administration origin refuses it.
#[test]
fn single_member_motion_is_refused() {
    ext().execute_with(|| {
        let call = set_threshold_call(1);
        let len = u32::try_from(call.encoded_size()).unwrap();
        assert_ok!(Council::propose(
            RuntimeOrigin::signed(ALICE),
            1,
            Box::new(call),
            len
        ));
        assert_eq!(Threshold::<Test>::get(), 2);
    });
}

// Scenario "非成员被拒绝".
#[test]
fn non_members_cannot_propose_or_vote() {
    ext().execute_with(|| {
        let call = set_threshold_call(1);
        let len = u32::try_from(call.encoded_size()).unwrap();
        assert!(
            Council::propose(RuntimeOrigin::signed(DAVE), 2, Box::new(call.clone()), len).is_err()
        );
        let (hash, index) = propose(ALICE, call);
        assert!(Council::vote(RuntimeOrigin::signed(DAVE), hash, index, true).is_err());
    });
}

// Scenario "替换成员": after replacing charlie by dave (threshold 2), charlie can no longer
// propose and dave can.
#[test]
fn members_are_replaced_by_the_administration() {
    ext().execute_with(|| {
        assert_noop!(
            PoaAdmin::set_members(RuntimeOrigin::signed(ALICE), vec![ALICE, BOB, DAVE], 2),
            DispatchError::BadOrigin
        );
        assert_ok!(PoaAdmin::set_members(
            members_origin(2),
            vec![DAVE, BOB, ALICE],
            2
        ));
        assert_eq!(members(), vec![ALICE, BOB, DAVE]);
        assert_eq!(Threshold::<Test>::get(), 2);
        let call = set_threshold_call(3);
        let len = u32::try_from(call.encoded_size()).unwrap();
        assert!(
            Council::propose(
                RuntimeOrigin::signed(CHARLIE),
                2,
                Box::new(call.clone()),
                len
            )
            .is_err()
        );
        let (hash, index) = propose(DAVE, call.clone());
        assert_ok!(Council::vote(
            RuntimeOrigin::signed(ALICE),
            hash,
            index,
            true
        ));
        assert_ok!(close(hash, index, &call));
        assert_eq!(Threshold::<Test>::get(), 3);
    });
}

// Members and threshold always satisfy 1 ≤ t ≤ members; member lists are bounded and unique.
#[test]
fn member_and_threshold_checks() {
    ext().execute_with(|| {
        let admin = || members_origin(2);
        assert_noop!(
            PoaAdmin::set_threshold(admin(), 0),
            Error::<Test>::InvalidThreshold
        );
        assert_noop!(
            PoaAdmin::set_threshold(admin(), 4),
            Error::<Test>::InvalidThreshold
        );
        assert_noop!(
            PoaAdmin::set_members(admin(), vec![], 1),
            Error::<Test>::InvalidThreshold
        );
        assert_noop!(
            PoaAdmin::set_members(admin(), vec![ALICE, BOB], 3),
            Error::<Test>::InvalidThreshold
        );
        assert_noop!(
            PoaAdmin::set_members(admin(), vec![ALICE, ALICE], 1),
            Error::<Test>::DuplicateMember
        );
        assert_noop!(
            PoaAdmin::set_members(admin(), (1..=6).collect(), 1),
            Error::<Test>::TooManyMembers
        );
        // The council's own member call is closed: members change only here.
        assert!(Council::set_members(RuntimeOrigin::root(), vec![DAVE], None, 3).is_err());
        assert_ok!(PoaAdmin::set_members(admin(), vec![BOB], 1));
        assert_eq!((members(), Threshold::<Test>::get()), (vec![BOB], 1));
    });
}

// `dispatch_as_root` runs the call as Root once, reports its result, and refuses calls outside
// the Root call filter.
#[test]
fn dispatch_as_root() {
    ext().execute_with(|| {
        let remark = RuntimeCall::System(frame_system::Call::remark {
            remark: b"hi".to_vec(),
        });
        assert_noop!(
            PoaAdmin::dispatch_as_root(members_origin(1), Box::new(remark.clone())),
            DispatchError::BadOrigin
        );
        assert_ok!(PoaAdmin::dispatch_as_root(
            members_origin(2),
            Box::new(remark)
        ));
        assert_eq!(
            admin_events(),
            vec![Event::DispatchedAsRoot { result: Ok(()) }]
        );
        // A Root-only call runs, and its failure is reported, not hidden.
        let bad = RuntimeCall::PoaAdmin(crate::Call::set_threshold { threshold: 9 });
        assert_ok!(PoaAdmin::dispatch_as_root(members_origin(2), Box::new(bad)));
        assert!(matches!(
            admin_events().last(),
            Some(Event::DispatchedAsRoot { result: Err(_) })
        ));
        let filtered = RuntimeCall::System(frame_system::Call::set_storage { items: vec![] });
        assert_noop!(
            PoaAdmin::dispatch_as_root(members_origin(2), Box::new(filtered)),
            Error::<Test>::CallFiltered
        );
    });
}

// Genesis: a threshold above the members fails, as does a threshold without members; neither
// members nor threshold leaves the administration unset.
#[test]
fn genesis_checks() {
    let build = |members: Vec<u64>, threshold: u32| {
        RuntimeGenesisConfig {
            system: Default::default(),
            council: pallet_collective::GenesisConfig {
                members,
                ..Default::default()
            },
            poa_admin: crate::GenesisConfig {
                threshold,
                ..Default::default()
            },
        }
        .build_storage()
    };
    assert!(build(vec![], 0).is_ok());
    assert!(build(vec![ALICE], 1).is_ok());
    for (members, threshold) in [(vec![], 1), (vec![ALICE, BOB], 3), (vec![ALICE], 0)] {
        let result = std::panic::catch_unwind(|| build(members.clone(), threshold));
        assert!(result.is_err(), "{members:?} / {threshold}");
    }
}
