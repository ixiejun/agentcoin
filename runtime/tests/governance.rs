//! Runtime-level PoA administration (spec governance/poa-admin) and the holder-treasury lock
//! (economics/treasury "持币人国库锁定"). Tasks 5.3, 6.1 and 6.2 of m3-economics.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

mod common;

use ac_runtime::genesis_config_presets::dev_account;
use ac_runtime::{
    ATC, AccountId, Hash, Runtime, RuntimeCall, RuntimeEvent, RuntimeGenesisConfig, RuntimeOrigin,
    System, TreasuryDual,
};
use common::{GENESIS, free, issuance, preset_ext, preset_json, start_block};
use frame_support::dispatch::{DispatchResult, GetDispatchInfo};
use frame_support::genesis_builder_helper::build_state;
use parity_scale_codec::Encode;
use sp_core::traits::{Externalities, ReadRuntimeVersion, ReadRuntimeVersionExt};
use sp_runtime::DispatchError;
use sp_runtime::traits::{Dispatchable, Hash as _};

type Council = pallet_collective::Pallet<Runtime, pallet_collective::Instance1>;

fn who(name: &str) -> AccountId {
    dev_account(name).unwrap()
}

/// Local chain (alice, bob, charlie; threshold 2) positioned inside block 1.
fn local() -> sp_io::TestExternalities {
    let mut ext = preset_ext(sp_genesis_builder::LOCAL_TESTNET_RUNTIME_PRESET);
    ext.execute_with(|| start_block(1, GENESIS));
    ext
}

fn signed(name: &str, call: RuntimeCall) -> DispatchResult {
    call.dispatch(RuntimeOrigin::signed(who(name)))
        .map(|_| ())
        .map_err(|e| e.error)
}

fn propose(name: &str, call: &RuntimeCall) -> (Hash, u32) {
    let hash = <Runtime as frame_system::Config>::Hashing::hash_of(call);
    let index = pallet_collective::ProposalCount::<Runtime, pallet_collective::Instance1>::get();
    signed(
        name,
        RuntimeCall::PoaCouncil(pallet_collective::Call::propose {
            threshold: 2,
            proposal: Box::new(call.clone()),
            length_bound: u32::try_from(call.encoded_size()).unwrap(),
        }),
    )
    .unwrap();
    (hash, index)
}

fn vote(name: &str, hash: Hash, index: u32) -> DispatchResult {
    signed(
        name,
        RuntimeCall::PoaCouncil(pallet_collective::Call::vote {
            proposal: hash,
            index,
            approve: true,
        }),
    )
}

fn close(hash: Hash, index: u32, call: &RuntimeCall) -> DispatchResult {
    signed(
        "alice",
        RuntimeCall::PoaCouncil(pallet_collective::Call::close {
            proposal_hash: hash,
            index,
            proposal_weight_bound: call.get_dispatch_info().call_weight,
            length_bound: u32::try_from(call.encoded_size()).unwrap(),
        }),
    )
}

/// Runs `call` as a motion approved by alice and bob; returns the motion's execution result.
fn motion(call: RuntimeCall) -> DispatchResult {
    let (hash, index) = propose("alice", &call);
    vote("alice", hash, index).unwrap();
    vote("bob", hash, index).unwrap();
    close(hash, index, &call).unwrap();
    executed(hash).expect("motion executed")
}

fn executed(hash: Hash) -> Option<DispatchResult> {
    System::events().into_iter().find_map(|r| match r.event {
        RuntimeEvent::PoaCouncil(pallet_collective::Event::Executed {
            proposal_hash,
            result,
        }) if proposal_hash == hash => Some(result),
        _ => None,
    })
}

/// Result of the last `PoaAdmin::dispatch_as_root`.
fn last_root_result() -> Option<DispatchResult> {
    System::events()
        .into_iter()
        .rev()
        .find_map(|r| match r.event {
            RuntimeEvent::PoaAdmin(pallet_poa_admin::Event::DispatchedAsRoot { result }) => {
                Some(result)
            }
            _ => None,
        })
}

fn as_root(call: RuntimeCall) -> RuntimeCall {
    RuntimeCall::PoaAdmin(pallet_poa_admin::Call::dispatch_as_root {
        call: Box::new(call),
    })
}

fn spend(to: &AccountId, amount: u128) -> RuntimeCall {
    RuntimeCall::TreasuryDual(pallet_treasury_dual::Call::spend {
        to: to.clone(),
        amount,
    })
}

fn fund(account: &AccountId, amount: u128) {
    signed(
        "dave",
        RuntimeCall::Balances(pallet_balances::Call::transfer_keep_alive {
            dest: account.clone(),
            value: amount,
        }),
    )
    .unwrap();
}

// Scenario "达到门限后执行": alice proposes a treasury spend, bob approves; it runs once.
#[test]
fn spend_runs_at_threshold() {
    local().execute_with(|| {
        let community = TreasuryDual::community_account();
        fund(&community, 10 * ATC);
        let dave = who("dave");
        let before = free(&dave);
        let call = spend(&dave, 3 * ATC);
        let (hash, index) = propose("alice", &call);
        vote("alice", hash, index).unwrap();
        vote("bob", hash, index).unwrap();
        close(hash, index, &call).unwrap();
        assert_eq!(executed(hash), Some(Ok(())));
        assert_eq!(free(&dave), before + 3 * ATC);
        assert_eq!(free(&community), 7 * ATC);
        // The motion is gone: it cannot run twice.
        assert!(close(hash, index, &call).is_err());
        assert_eq!(free(&dave), before + 3 * ATC);
    });
}

// Scenario "未达门限不执行": only the proposer approved; the motion expires rejected.
#[test]
fn spend_below_threshold_does_not_run() {
    local().execute_with(|| {
        let community = TreasuryDual::community_account();
        fund(&community, 10 * ATC);
        let call = spend(&who("dave"), 3 * ATC);
        let (hash, index) = propose("alice", &call);
        vote("alice", hash, index).unwrap();
        assert!(close(hash, index, &call).is_err(), "too early");
        let expiry =
            System::block_number() + ac_runtime::genesis_config_presets::DEV_MOTION_DURATION + 1;
        System::set_block_number(expiry);
        close(hash, index, &call).unwrap();
        assert_eq!(executed(hash), None);
        assert_eq!(free(&community), 10 * ATC);
        // A single member cannot spend directly either.
        assert_eq!(
            signed("alice", spend(&who("alice"), ATC)),
            Err(DispatchError::BadOrigin)
        );
    });
}

// Scenario "非成员被拒绝".
#[test]
fn non_members_are_refused() {
    local().execute_with(|| {
        let call = spend(&who("dave"), ATC);
        let result = signed(
            "dave",
            RuntimeCall::PoaCouncil(pallet_collective::Call::propose {
                threshold: 2,
                proposal: Box::new(call.clone()),
                length_bound: u32::try_from(call.encoded_size()).unwrap(),
            }),
        );
        assert!(result.is_err());
        let (hash, index) = propose("alice", &call);
        assert!(vote("dave", hash, index).is_err());
    });
}

// Scenario "替换成员": alice, bob, charlie → alice, bob, dave with threshold 2.
#[test]
fn members_are_replaced() {
    local().execute_with(|| {
        let new = vec![who("alice"), who("bob"), who("dave")];
        let call = RuntimeCall::PoaAdmin(pallet_poa_admin::Call::set_members {
            members: new.clone(),
            threshold: 2,
        });
        assert_eq!(motion(call), Ok(()));
        let mut sorted = new;
        sorted.sort();
        assert_eq!(
            pallet_collective::Members::<Runtime, pallet_collective::Instance1>::get(),
            sorted
        );
        assert_eq!(pallet_poa_admin::Threshold::<Runtime>::get(), 2);
        let call = spend(&who("dave"), ATC);
        let charlie = signed(
            "charlie",
            RuntimeCall::PoaCouncil(pallet_collective::Call::propose {
                threshold: 2,
                proposal: Box::new(call.clone()),
                length_bound: u32::try_from(call.encoded_size()).unwrap(),
            }),
        );
        assert!(charlie.is_err());
        let (hash, index) = propose("dave", &call);
        assert_eq!(vote("dave", hash, index), Ok(()));
        // The council's own member call is closed.
        assert!(Council::set_members(RuntimeOrigin::root(), vec![who("dave")], None, 3).is_err());
    });
}

// Scenario "缺少成员的正式链创世": a genesis with a threshold but no members fails to build; the
// node additionally refuses live chains without members (ac-invariants `check_genesis`).
#[test]
fn genesis_without_members_fails() {
    let build = |json: &serde_json::Value| {
        let bytes = serde_json::to_vec(json).unwrap();
        sp_io::TestExternalities::default().execute_with(|| {
            std::panic::catch_unwind(|| build_state::<RuntimeGenesisConfig>(bytes))
        })
    };
    let mut json: serde_json::Value =
        serde_json::from_slice(&preset_json(sp_genesis_builder::DEV_RUNTIME_PRESET)).unwrap();
    assert!(matches!(build(&json), Ok(Ok(()))));
    json["poaCouncil"]["members"] = serde_json::json!([]);
    assert!(!matches!(build(&json), Ok(Ok(()))));
}

// Scenario "管理权限无法动用持币人国库": motions of the administration try every forced way
// out of the holder treasury; all fail and its balance stays.
#[test]
fn holder_treasury_cannot_be_spent() {
    local().execute_with(|| {
        let holder = TreasuryDual::holder_account();
        fund(&holder, 10 * ATC);
        let dave = who("dave");
        let supply = issuance();
        let attempts = [
            RuntimeCall::Balances(pallet_balances::Call::force_transfer {
                source: holder.clone(),
                dest: dave.clone(),
                value: ATC,
            }),
            RuntimeCall::Balances(pallet_balances::Call::force_set_balance {
                who: holder.clone(),
                new_free: 0,
            }),
            RuntimeCall::Balances(pallet_balances::Call::force_unreserve {
                who: holder.clone(),
                amount: ATC,
            }),
            RuntimeCall::Balances(pallet_balances::Call::transfer_all {
                dest: dave.clone(),
                keep_alive: false,
            }),
            RuntimeCall::System(frame_system::Call::set_storage {
                items: vec![(
                    frame_system::Account::<Runtime>::hashed_key_for(&holder),
                    Vec::new(),
                )],
            }),
            RuntimeCall::System(frame_system::Call::kill_storage {
                keys: vec![frame_system::Account::<Runtime>::hashed_key_for(&holder)],
            }),
            RuntimeCall::System(frame_system::Call::kill_prefix {
                prefix: frame_support::storage::storage_prefix(b"System", b"Account").to_vec(),
                subkeys: 16,
            }),
        ];
        for call in attempts {
            // Directly as a motion (council origin) and as Root, also nested.
            for wrapped in [call.clone(), as_root(call.clone()), as_root(as_root(call))] {
                let result = motion(wrapped.clone());
                let root = last_root_result();
                assert!(
                    result.is_err() || matches!(root, Some(Err(_))),
                    "{wrapped:?} passed"
                );
                assert_eq!(free(&holder), 10 * ATC, "{wrapped:?}");
            }
        }
        assert_eq!(issuance(), supply);
        // Community grants stay spendable through the same path.
        fund(&TreasuryDual::community_account(), ATC);
        assert_eq!(motion(as_root(spend(&dave, ATC / 2))), Ok(()));
        assert_eq!(last_root_result(), Some(Ok(())));
    });
}

/// Reports the current runtime version with a higher `spec_version`, as a newer build of the
/// same runtime would.
struct NewerVersion;

impl ReadRuntimeVersion for NewerVersion {
    fn read_runtime_version(
        &self,
        _wasm_code: &[u8],
        _ext: &mut dyn Externalities,
    ) -> Result<Vec<u8>, String> {
        let mut version = ac_runtime::VERSION;
        version.spec_version += 1;
        Ok(version.encode())
    }
}

// Scenario "多签升级" (runtime part): a motion runs `System::set_code` as Root and the code is
// replaced; a signed account or a single member cannot. The version change of a running node is
// checked end to end (m3-economics 8.2).
#[test]
fn upgrade_through_the_multisig() {
    let mut ext = local();
    ext.register_extension(ReadRuntimeVersionExt::new(NewerVersion));
    ext.execute_with(|| {
        let code = b"\0asm new runtime".to_vec();
        let set_code = RuntimeCall::System(frame_system::Call::set_code { code: code.clone() });
        assert_eq!(
            signed("alice", set_code.clone()),
            Err(DispatchError::BadOrigin)
        );
        assert_eq!(
            signed("alice", as_root(set_code.clone())),
            Err(DispatchError::BadOrigin)
        );
        assert_eq!(motion(as_root(set_code)), Ok(()));
        assert_eq!(last_root_result(), Some(Ok(())));
        assert_eq!(
            sp_io::storage::get(sp_core::storage::well_known_keys::CODE).map(|c| c.to_vec()),
            Some(code)
        );
        assert!(System::events().iter().any(|r| matches!(
            r.event,
            RuntimeEvent::System(frame_system::Event::CodeUpdated { .. })
        )));
    });
}
