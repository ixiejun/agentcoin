//! Tests of the payer, the non-minting currency and the extension (m4-evm tasks 2.1–2.3).
//!
//! The contracts are hand-assembled EVM bytecode so that the tests need no compiler.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use crate::{
    EvmPayer, SetEvmPayer,
    extension::is_contract_transaction,
    mock::{
        ATC, AccountId, Balance, Balances, ED, EvmSupport, Revive, RuntimeCall, RuntimeEvent,
        RuntimeOrigin, System, Test, TestReviveCurrency, alice, bob, burned, gross_supply,
        new_test_ext,
    },
};
use frame_support::{
    assert_err, assert_ok,
    dispatch::DispatchInfo,
    pallet_prelude::{TransactionSource, Weight},
    traits::{
        Hooks, Imbalance as _,
        fungible::{Balanced, Inspect, InspectHold, Mutate, MutateHold},
        tokens::{Fortitude, Precision, Preservation},
    },
};
use pallet_revive::{AddressMapper, H160, U256};
use proptest::prelude::{ProptestConfig, any, prop_assert, prop_assert_eq, proptest};
use sp_runtime::{
    TokenError,
    traits::{Dispatchable, TransactionExtension, TxBaseImplication},
};

// --- EVM bytecode --------------------------------------------------------------------------------

/// Wraps runtime code in init code that returns it (CODECOPY + RETURN, 11-byte prefix).
fn init_code(runtime: &[u8]) -> Vec<u8> {
    let len = u8::try_from(runtime.len()).unwrap();
    let mut code = vec![
        0x60, len,  // PUSH1 len
        0x80, // DUP1
        0x60, 0x0b, // PUSH1 11 (offset of the runtime code)
        0x60, 0x00, // PUSH1 0
        0x39, // CODECOPY(0, 11, len)
        0x60, 0x00, // PUSH1 0
        0xf3, // RETURN(0, len)
    ];
    code.extend_from_slice(runtime);
    code
}

/// Stores the first calldata word in slot 0, or clears slot 0 when called without data.
const STORE: &[u8] = &[
    0x36, 0x15, 0x60, 0x0c, 0x57, // if calldatasize == 0 goto clear
    0x60, 0x00, 0x35, 0x60, 0x00, 0x55, 0x00, // sstore(0, calldataload(0)); stop
    0x5b, 0x60, 0x00, 0x60, 0x00, 0x55, 0x00, // clear: sstore(0, 0); stop
];

/// CREATE2s the init code given as calldata (salt 1) and returns the new address.
const FACTORY: &[u8] = &[
    0x36, 0x60, 0x00, 0x60, 0x00, 0x37, // calldatacopy(0, 0, calldatasize)
    0x60, 0x01, 0x36, 0x60, 0x00, 0x60, 0x00, 0xf5, // create2(0, 0, calldatasize, 1)
    0x60, 0x00, 0x52, // mstore(0, address)
    0x60, 0x20, 0x60, 0x00, 0xf3, // return(0, 32)
];

/// Init code that self-destructs in its constructor (created and destroyed in one transaction).
const SELF_DESTRUCT_INIT: &[u8] = &[0x33, 0xff];

/// Calls the builtin sha256 precompile (0x02) with no input.
const CALL_SHA256: &[u8] = &[
    0x60, 0x20, 0x60, 0x00, 0x60, 0x00, 0x60, 0x00, // ret 32 @0, args 0 @0
    0x60, 0x02, 0x5a, 0xfa, // staticcall(gas, 0x02, ...)
    0x00,
];

fn weight_limit() -> Weight {
    Weight::from_parts(1_000_000_000_000, 10 * 1024 * 1024)
}

const DEPOSIT_LIMIT: Balance = 100 * ATC;

/// Deploys `code` from `who` with `who` as the payer (what `SetEvmPayer` does).
fn deploy(who: &AccountId, code: Vec<u8>) -> H160 {
    EvmPayer::<Test>::put(who.clone());
    let result = Revive::instantiate_with_code(
        RuntimeOrigin::signed(who.clone()),
        0,
        weight_limit(),
        DEPOSIT_LIMIT,
        code,
        vec![],
        None,
    );
    EvmPayer::<Test>::kill();
    assert_ok!(result);
    last_instantiated()
}

fn last_instantiated() -> H160 {
    System::events()
        .into_iter()
        .rev()
        .find_map(|record| match record.event {
            RuntimeEvent::Revive(pallet_revive::Event::Instantiated { contract, .. }) => {
                Some(contract)
            }
            _ => None,
        })
        .unwrap()
}

fn last_new_contract_account() -> AccountId {
    System::events()
        .into_iter()
        .rev()
        .find_map(|record| match record.event {
            RuntimeEvent::System(frame_system::Event::NewAccount { account })
                if <Test as pallet_revive::Config>::AddressMapper::is_eth_derived(&account) =>
            {
                Some(account)
            }
            _ => None,
        })
        .unwrap()
}

fn call(who: &AccountId, dest: H160, data: Vec<u8>) {
    EvmPayer::<Test>::put(who.clone());
    let result = Revive::call(
        RuntimeOrigin::signed(who.clone()),
        dest,
        0,
        weight_limit(),
        DEPOSIT_LIMIT,
        data,
    );
    EvmPayer::<Test>::kill();
    assert_ok!(result);
}

fn account_of(address: H160) -> AccountId {
    <Test as pallet_revive::Config>::AddressMapper::to_account_id(&address)
}

// --- Gate (task 2.1): no path creates issuance ------------------------------------------------------

#[test]
fn genesis_mints_nothing() {
    new_test_ext().execute_with(|| {
        assert_eq!(gross_supply(), 2_000 * ATC);
        // Revive's attempt to mint an ED into its own account had no payer and failed; the
        // account exists through a provider reference with a zero balance instead.
        let revive = pallet_revive::Pallet::<Test>::account_id();
        assert!(System::account_exists(&revive));
        assert_eq!(Balances::total_balance(&revive), 0);
    });
}

// Spec evm/contracts, Scenario "部署不增发".
#[test]
fn deployment_is_paid_by_the_signer() {
    new_test_ext().execute_with(|| {
        let before = Balances::balance(&alice());
        let contract = deploy(&alice(), init_code(STORE));
        let contract_account = account_of(contract);
        assert_eq!(gross_supply(), 2_000 * ATC);
        assert_eq!(burned(), 0);
        assert!(Balances::total_balance(&contract_account) >= ED);
        let paid = before - Balances::balance(&alice());
        assert!(paid >= ED, "the signer paid at least the contract ED");
    });
}

// Spec evm/contracts, Scenario "签名者无力支付" (no payer at all is the extreme case).
#[test]
fn deployment_without_payer_fails() {
    new_test_ext().execute_with(|| {
        let result = Revive::instantiate_with_code(
            RuntimeOrigin::signed(alice()),
            0,
            weight_limit(),
            DEPOSIT_LIMIT,
            init_code(STORE),
            vec![],
            None,
        );
        assert!(result.is_err());
        assert_eq!(gross_supply(), 2_000 * ATC);
    });
}

// Spec evm/contracts, Scenario "押金冻结与退还".
#[test]
fn storage_deposit_is_held_and_refunded() {
    new_test_ext().execute_with(|| {
        let contract = deploy(&alice(), init_code(STORE));
        let before = Balances::balance(&bob());
        call(&bob(), contract, [7u8; 32].to_vec());
        let after_write = Balances::balance(&bob());
        assert!(after_write < before, "bob paid a storage deposit");
        call(&bob(), contract, vec![]);
        assert_eq!(Balances::balance(&bob()), before, "the deposit came back");
        assert_eq!(gross_supply(), 2_000 * ATC);
    });
}

// Spec evm/contracts, Scenario "合约内创建合约".
#[test]
fn create2_child_is_paid_by_the_signer() {
    new_test_ext().execute_with(|| {
        let factory = deploy(&alice(), init_code(FACTORY));
        let before = Balances::balance(&bob());
        call(&bob(), factory, init_code(&[0x00]));
        // Revive emits `Instantiated` only for top-level deployments; find the child through
        // the account it created (contract accounts are "eth-derived": 0xEE-suffixed).
        let child = last_new_contract_account();
        assert_ne!(child, account_of(factory));
        assert!(Balances::total_balance(&child) >= ED);
        assert!(before - Balances::balance(&bob()) >= ED);
        assert_eq!(gross_supply(), 2_000 * ATC);
    });
}

#[test]
fn self_destruct_burns_are_counted() {
    new_test_ext().execute_with(|| {
        let factory = deploy(&alice(), init_code(FACTORY));
        call(&bob(), factory, SELF_DESTRUCT_INIT.to_vec());
        // Created and destroyed in one transaction: whatever revive burns is counted.
        assert_eq!(gross_supply(), 2_000 * ATC);
    });
}

#[test]
fn precompile_calls_do_not_mint() {
    new_test_ext().execute_with(|| {
        let caller = deploy(&alice(), init_code(CALL_SHA256));
        call(&bob(), caller, vec![]);
        assert_eq!(gross_supply(), 2_000 * ATC);
    });
}

#[test]
fn dry_run_with_payer_does_not_mint() {
    new_test_ext().execute_with(|| {
        let result = EvmSupport::with_payer(bob(), || {
            pallet_revive::Pallet::<Test>::bare_instantiate(
                RuntimeOrigin::signed(bob()),
                U256::zero(),
                pallet_revive::TransactionLimits::WeightAndDeposit {
                    weight_limit: weight_limit(),
                    deposit_limit: DEPOSIT_LIMIT,
                },
                pallet_revive::Code::Upload(init_code(STORE)),
                vec![],
                None,
                &pallet_revive::ExecConfig::new_substrate_tx(),
            )
        });
        assert_ok!(result.result);
        assert_eq!(gross_supply(), 2_000 * ATC);
        assert_eq!(EvmPayer::<Test>::get(), None);
    });
}

// --- ReviveCurrency (task 2.2) --------------------------------------------------------------------

#[test]
fn mint_into_transfers_from_the_payer() {
    new_test_ext().execute_with(|| {
        let target = AccountId::new([9; 32]);
        assert_err!(
            TestReviveCurrency::mint_into(&target, ED),
            TokenError::CannotCreate
        );
        EvmPayer::<Test>::put(alice());
        assert_ok!(TestReviveCurrency::mint_into(&target, ED));
        assert_eq!(Balances::balance(&target), ED);
        assert_eq!(Balances::balance(&alice()), 1_000 * ATC - ED);
        assert_eq!(gross_supply(), 2_000 * ATC);
    });
}

#[test]
fn issue_is_backed_by_the_payer() {
    new_test_ext().execute_with(|| {
        assert_eq!(TestReviveCurrency::issue(ATC).peek(), 0);
        EvmPayer::<Test>::put(bob());
        let credit = TestReviveCurrency::issue(ATC);
        assert_eq!(credit.peek(), ATC);
        assert_ok!(TestReviveCurrency::resolve(&alice(), credit));
        assert_eq!(Balances::balance(&alice()), 1_001 * ATC);
        assert_eq!(gross_supply(), 2_000 * ATC);
    });
}

#[test]
fn burns_are_counted() {
    new_test_ext().execute_with(|| {
        assert_ok!(TestReviveCurrency::burn_from(
            &alice(),
            ATC,
            Preservation::Preserve,
            Precision::Exact,
            Fortitude::Polite
        ));
        assert_eq!(burned(), ATC);
        assert_eq!(pallet_balances::TotalIssuance::<Test>::get(), 1_999 * ATC);
        // set_balance lowers by burning and never raises.
        assert_eq!(
            TestReviveCurrency::set_balance(&alice(), 500 * ATC),
            500 * ATC
        );
        assert_eq!(burned(), 500 * ATC);
        assert_eq!(
            TestReviveCurrency::set_balance(&alice(), 900 * ATC),
            500 * ATC
        );
        assert_eq!(gross_supply(), 2_000 * ATC);
    });
}

#[test]
fn unused_minting_paths_fail() {
    new_test_ext().execute_with(|| {
        let reason = RuntimeHoldReasonForTest::get();
        assert_ok!(TestReviveCurrency::hold(&reason, &alice(), ATC));
        assert!(TestReviveCurrency::restore(&alice(), ATC).is_err());
        assert!(TestReviveCurrency::shelve(&alice(), ATC).is_err());
        assert!(
            TestReviveCurrency::burn_held(
                &reason,
                &alice(),
                ATC,
                Precision::Exact,
                Fortitude::Force
            )
            .is_err()
        );
        assert!(
            TestReviveCurrency::burn_all_held(
                &reason,
                &alice(),
                Precision::Exact,
                Fortitude::Force
            )
            .is_err()
        );
        assert_eq!(TestReviveCurrency::balance_on_hold(&reason, &alice()), ATC);
        assert_eq!(gross_supply(), 2_000 * ATC);
    });
}

/// A hold reason of the test runtime.
struct RuntimeHoldReasonForTest;

impl RuntimeHoldReasonForTest {
    fn get() -> crate::mock::RuntimeHoldReason {
        crate::mock::RuntimeHoldReason::Revive(pallet_revive::HoldReason::StorageDepositReserve)
    }
}

/// One operation of the random currency sequence.
#[derive(Debug, Clone)]
enum Op {
    Hold(u8),
    Release(u8),
    Transfer(u8),
    Burn(u8),
    Mint(u8, bool),
    Issue(u8, bool),
    SetBalance(u16),
}

fn op() -> impl proptest::strategy::Strategy<Value = Op> {
    use proptest::prelude::{Strategy, prop_oneof};
    prop_oneof![
        any::<u8>().prop_map(Op::Hold),
        any::<u8>().prop_map(Op::Release),
        any::<u8>().prop_map(Op::Transfer),
        any::<u8>().prop_map(Op::Burn),
        (any::<u8>(), any::<bool>()).prop_map(|(a, p)| Op::Mint(a, p)),
        (any::<u8>(), any::<bool>()).prop_map(|(a, p)| Op::Issue(a, p)),
        any::<u16>().prop_map(Op::SetBalance),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    // Issuance only ever goes down, and exactly by what is counted as burned.
    #[test]
    fn issuance_never_grows(ops in proptest::collection::vec(op(), 1..40)) {
        new_test_ext().execute_with(|| {
            let reason = RuntimeHoldReasonForTest::get();
            let unit = ATC / 10;
            for op in ops {
                let issuance_before = pallet_balances::TotalIssuance::<Test>::get();
                let burned_before = burned();
                match op {
                    Op::Hold(a) => { let _ = TestReviveCurrency::hold(&reason, &alice(), unit * Balance::from(a)); }
                    Op::Release(a) => { let _ = TestReviveCurrency::release(&reason, &alice(), unit * Balance::from(a), Precision::BestEffort); }
                    Op::Transfer(a) => { let _ = TestReviveCurrency::transfer(&alice(), &bob(), unit * Balance::from(a), Preservation::Preserve); }
                    Op::Burn(a) => { let _ = TestReviveCurrency::burn_from(&bob(), unit * Balance::from(a), Preservation::Preserve, Precision::BestEffort, Fortitude::Polite); }
                    Op::Mint(a, payer) => {
                        if payer { EvmPayer::<Test>::put(bob()) } else { EvmPayer::<Test>::kill() }
                        let _ = TestReviveCurrency::mint_into(&alice(), unit * Balance::from(a));
                    }
                    Op::Issue(a, payer) => {
                        if payer { EvmPayer::<Test>::put(bob()) } else { EvmPayer::<Test>::kill() }
                        let credit = TestReviveCurrency::issue(unit * Balance::from(a));
                        let _ = TestReviveCurrency::resolve(&alice(), credit);
                    }
                    Op::SetBalance(a) => { let _ = TestReviveCurrency::set_balance(&alice(), unit * Balance::from(a)); }
                }
                let issuance_after = pallet_balances::TotalIssuance::<Test>::get();
                let burned_after = burned();
                prop_assert!(issuance_after <= issuance_before);
                prop_assert_eq!(issuance_before - issuance_after, burned_after - burned_before);
            }
            prop_assert_eq!(gross_supply(), 2_000 * ATC);
            Ok(())
        })?;
    }
}

// --- SetEvmPayer (task 2.3) -----------------------------------------------------------------------

fn contract_call() -> RuntimeCall {
    RuntimeCall::Revive(pallet_revive::Call::call {
        dest: H160::zero(),
        value: 0,
        weight_limit: weight_limit(),
        storage_deposit_limit: 0,
        data: vec![],
    })
}

fn transfer_call() -> RuntimeCall {
    RuntimeCall::Balances(pallet_balances::Call::transfer_allow_death {
        dest: bob(),
        value: ED,
    })
}

/// Runs the extension's validate → prepare → (dispatch) → post_dispatch for `call`, returning
/// the payer seen during dispatch.
fn run_extension(call: &RuntimeCall, fail: bool) -> Option<AccountId> {
    let ext = SetEvmPayer::<Test>::new();
    let info = DispatchInfo::default();
    let origin = RuntimeOrigin::signed(alice());
    let (_, val, origin) = ext
        .validate(
            origin,
            call,
            &info,
            0,
            (),
            &TxBaseImplication(()),
            TransactionSource::External,
        )
        .unwrap();
    let pre = ext.prepare(val, &origin, call, &info, 0).unwrap();
    let seen = EvmPayer::<Test>::get();
    let result = if fail {
        Err(sp_runtime::DispatchError::Other("fail"))
    } else {
        Ok(())
    };
    SetEvmPayer::<Test>::post_dispatch_details(pre, &info, &Default::default(), 0, &result)
        .unwrap();
    seen
}

#[test]
fn payer_is_set_only_for_contract_transactions() {
    new_test_ext().execute_with(|| {
        assert!(is_contract_transaction::<Test>(&contract_call()));
        assert!(!is_contract_transaction::<Test>(&transfer_call()));
        let wrapped = RuntimeCall::Revive(pallet_revive::Call::dispatch_as_fallback_account {
            call: Box::new(contract_call()),
        });
        assert!(is_contract_transaction::<Test>(&wrapped));

        assert_eq!(run_extension(&contract_call(), false), Some(alice()));
        assert_eq!(EvmPayer::<Test>::get(), None, "cleared after dispatch");
        assert_eq!(run_extension(&contract_call(), true), Some(alice()));
        assert_eq!(
            EvmPayer::<Test>::get(),
            None,
            "cleared after a failed dispatch"
        );
        assert_eq!(run_extension(&transfer_call(), false), None);
        assert!(!sp_io::storage::exists(&EvmPayer::<Test>::hashed_key()));

        let ext = SetEvmPayer::<Test>::new();
        assert_eq!(ext.weight(&transfer_call()), Weight::zero());
        assert!(ext.weight(&contract_call()).ref_time() > 0);
        // No explicit or implicit data.
        assert!(parity_scale_codec::Encode::encode(&ext).is_empty());
    });
}

#[test]
fn payer_is_cleared_at_the_end_of_the_block() {
    new_test_ext().execute_with(|| {
        EvmPayer::<Test>::put(alice());
        EvmSupport::on_finalize(1);
        assert_eq!(EvmPayer::<Test>::get(), None);
    });
}

#[test]
fn dispatch_uses_the_runtime_call() {
    new_test_ext().execute_with(|| {
        // A plain transfer still dispatches normally (no interference from the extension).
        assert_ok!(transfer_call().dispatch(RuntimeOrigin::signed(alice())));
    });
}
