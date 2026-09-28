//! EVM contracts in the runtime (m4-evm tasks 2.5–2.8): specs evm/contracts and
//! chain/pq-transaction-auth. Contracts are hand-assembled EVM bytecode (no compiler needed).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

mod common;

use ac_primitives::evm::{EVM_CHAIN_ID, POLKAVM_MAGIC, REVIVE_CALL_INDEX, classify_call};
use ac_runtime::{
    ATC, AccountId, Balance, EXISTENTIAL_DEPOSIT, Emission, Revive, Runtime, RuntimeCall,
    RuntimeEvent, System, UncheckedExtrinsic,
};
use common::{Signer, apply, dev_ext, fees_paid, free, issuance, signed};
use frame_support::traits::{Contains, fungible::Inspect};
use frame_support::weights::Weight;
use pallet_revive::{AddressMapper, H160};
use parity_scale_codec::{Decode, Encode};
use sp_core::U256;
use sp_runtime::DispatchError;

// --- bytecode ------------------------------------------------------------------------------------

/// Init code returning `runtime` (CODECOPY + RETURN, 11-byte prefix).
fn init_code(runtime: &[u8]) -> Vec<u8> {
    let len = u8::try_from(runtime.len()).unwrap();
    let mut code = vec![
        0x60, len, 0x80, 0x60, 0x0b, 0x60, 0x00, 0x39, 0x60, 0x00, 0xf3,
    ];
    code.extend_from_slice(runtime);
    code
}

/// Stores `caller` in slot 0, `chainid` in slot 1 and `balance(caller)` in slot 2.
const PROBE: &[u8] = &[
    0x33, 0x60, 0x00, 0x55, // sstore(0, caller)
    0x46, 0x60, 0x01, 0x55, // sstore(1, chainid)
    0x33, 0x31, 0x60, 0x02, 0x55, // sstore(2, balance(caller))
    0x00,
];

/// Returns `caller`, `chainid` and `balance(caller)` as three words (for dry runs).
const PROBE_VIEW: &[u8] = &[
    0x33, 0x60, 0x00, 0x52, // mstore(0, caller)
    0x46, 0x60, 0x20, 0x52, // mstore(32, chainid)
    0x33, 0x31, 0x60, 0x40, 0x52, // mstore(64, balance(caller))
    0x60, 0x60, 0x60, 0x00, 0xf3, // return(0, 96)
];

/// Stores the first calldata word in slot 0, or clears slot 0 when called without data.
const STORE: &[u8] = &[
    0x36, 0x15, 0x60, 0x0c, 0x57, 0x60, 0x00, 0x35, 0x60, 0x00, 0x55, 0x00, 0x5b, 0x60, 0x00, 0x60,
    0x00, 0x55, 0x00,
];

/// CREATE2s the init code given as calldata (salt 1) and returns the new address.
const FACTORY: &[u8] = &[
    0x36, 0x60, 0x00, 0x60, 0x00, 0x37, 0x60, 0x01, 0x36, 0x60, 0x00, 0x60, 0x00, 0xf5, 0x60, 0x00,
    0x52, 0x60, 0x20, 0x60, 0x00, 0xf3,
];

/// Loops forever (JUMPDEST; PUSH1 0; JUMP).
const SPIN: &[u8] = &[0x5b, 0x60, 0x00, 0x56];

/// Reverts with `Error("no")`.
const REVERT_NO: &[u8] = &[
    // mstore(0, 0x08c379a0 << 224): Error(string) selector
    0x63, 0x08, 0xc3, 0x79, 0xa0, 0x60, 0xe0, 0x1b, 0x60, 0x00, 0x52, // offset word = 0x20
    0x60, 0x20, 0x60, 0x04, 0x52, // length = 2
    0x60, 0x02, 0x60, 0x24, 0x52, // "no" left-aligned
    0x61, 0x6e, 0x6f, 0x60, 0xf0, 0x1b, 0x60, 0x44, 0x52, // revert(0, 100)
    0x60, 0x64, 0x60, 0x00, 0xfd,
];

// --- helpers -------------------------------------------------------------------------------------

fn weight_limit() -> Weight {
    Weight::from_parts(50_000_000_000, 1024 * 1024)
}

const DEPOSIT_LIMIT: Balance = 10 * ATC;

fn deploy_call(code: Vec<u8>) -> RuntimeCall {
    RuntimeCall::Revive(pallet_revive::Call::instantiate_with_code {
        value: 0,
        weight_limit: weight_limit(),
        storage_deposit_limit: DEPOSIT_LIMIT,
        code,
        data: vec![],
        salt: None,
    })
}

fn contract_call(dest: H160, data: Vec<u8>) -> RuntimeCall {
    RuntimeCall::Revive(pallet_revive::Call::call {
        dest,
        value: 0,
        weight_limit: weight_limit(),
        storage_deposit_limit: DEPOSIT_LIMIT,
        data,
    })
}

fn last_instantiated() -> Option<H160> {
    System::events()
        .into_iter()
        .rev()
        .find_map(|r| match r.event {
            RuntimeEvent::Revive(pallet_revive::Event::Instantiated { contract, .. }) => {
                Some(contract)
            }
            _ => None,
        })
}

fn deploy(signer: &Signer, runtime: &[u8]) -> H160 {
    assert_eq!(
        apply(signed(signer, deploy_call(init_code(runtime)))),
        Ok(Ok(()))
    );
    last_instantiated().unwrap()
}

fn address_of(account: &AccountId) -> H160 {
    <Runtime as pallet_revive::Config>::AddressMapper::to_address(account)
}

fn account_of(address: H160) -> AccountId {
    <Runtime as pallet_revive::Config>::AddressMapper::to_account_id(&address)
}

fn slot(contract: H160, index: u8) -> Vec<u8> {
    let mut key = [0u8; 32];
    key[31] = index;
    Revive::get_storage(contract, key)
        .unwrap()
        .unwrap_or_default()
}

fn word(value: impl Into<U256>) -> Vec<u8> {
    value.into().to_big_endian().to_vec()
}

fn address_word(address: H160) -> Vec<u8> {
    let mut out = vec![0u8; 12];
    out.extend_from_slice(address.as_bytes());
    out
}

/// Everything minted since genesis minus everything burned: constant unless emission settles.
fn gross() -> Balance {
    issuance() + Emission::total_burned()
}

/// A dry run of `dest` from `from` (no signature, state discarded).
fn dry_call(from: &AccountId, dest: H160, data: Vec<u8>) -> pallet_revive::ExecReturnValue {
    frame_support::storage::with_transaction(|| {
        let result = ac_runtime::EvmSupport::with_payer(from.clone(), || {
            Revive::bare_call(
                ac_runtime::RuntimeOrigin::signed(from.clone()),
                dest,
                U256::zero(),
                pallet_revive::TransactionLimits::WeightAndDeposit {
                    weight_limit: weight_limit(),
                    deposit_limit: DEPOSIT_LIMIT,
                },
                data,
                &pallet_revive::ExecConfig::new_substrate_tx(),
            )
        });
        sp_runtime::TransactionOutcome::Rollback(Ok::<_, DispatchError>(result))
    })
    .unwrap()
    .result
    .unwrap()
}

fn is_filtered(call: &RuntimeCall) -> bool {
    !ac_runtime::RuntimeCallFilter::contains(call)
}

// --- evm/contracts: deployment and calls ---------------------------------------------------------

// Requirement "只接受 EVM 字节码" / Scenario "部署 EVM 合约".
#[test]
fn evm_contract_is_deployed() {
    dev_ext().execute_with(|| {
        let alice = Signer::dev("alice");
        let contract = deploy(&alice, STORE);
        assert_eq!(Revive::code(&contract), STORE.to_vec());
    });
}

// Requirement "合约交易由后量子签名授权" / Scenario "ML-DSA 签名的合约调用".
#[test]
fn signed_contract_call_runs_and_pays_fees() {
    dev_ext().execute_with(|| {
        let alice = Signer::dev("alice");
        let bob = Signer::dev("bob");
        let contract = deploy(&alice, STORE);
        let nonce = System::account_nonce(&bob.account);
        System::reset_events();
        assert_eq!(
            apply(signed(&bob, contract_call(contract, word(7u8)))),
            Ok(Ok(()))
        );
        assert_eq!(slot(contract, 0), word(7u8));
        assert_eq!(System::account_nonce(&bob.account), nonce + 1);
        assert!(fees_paid() > 0);
    });
}

// Scenarios "合约看到调用者的 EVM 地址", "链 ID", "余额一致".
#[test]
fn contract_sees_caller_chain_id_and_balance() {
    dev_ext().execute_with(|| {
        let alice = Signer::dev("alice");
        let bob = Signer::dev("bob");
        let probe = deploy(&alice, PROBE);
        assert_eq!(
            apply(signed(&bob, contract_call(probe, vec![]))),
            Ok(Ok(()))
        );
        assert_eq!(slot(probe, 0), address_word(address_of(&bob.account)));
        assert_eq!(slot(probe, 1), word(EVM_CHAIN_ID));

        // `balance` in the EVM is the spendable native balance: free minus the existential
        // deposit (1 wei = 1 smallest ATC unit, no rounding).
        let view = deploy(&alice, PROBE_VIEW);
        let out = dry_call(&bob.account, view, vec![]).data;
        assert_eq!(out[..32], address_word(address_of(&bob.account))[..]);
        assert_eq!(out[32..64], word(EVM_CHAIN_ID)[..]);
        let spendable = free(&bob.account) - EXISTENTIAL_DEPOSIT;
        assert_eq!(out[64..96], word(spendable)[..]);
        assert_eq!(
            Revive::evm_balance(&address_of(&bob.account)),
            U256::from(spendable)
        );
    });
}

// Requirement "合约执行不产生新增发行量" / Scenario "部署不增发".
#[test]
fn deployment_creates_no_issuance() {
    dev_ext().execute_with(|| {
        let alice = Signer::dev("alice");
        let before = gross();
        let a0 = free(&alice.account);
        System::reset_events();
        let contract = deploy(&alice, STORE);
        assert_eq!(gross(), before);
        let contract_account = account_of(contract);
        assert!(
            pallet_balances::Pallet::<Runtime>::total_balance(&contract_account)
                >= EXISTENTIAL_DEPOSIT
        );
        // The deployer paid the fee, the storage deposits and the contract's existential deposit.
        assert!(a0 - free(&alice.account) >= fees_paid() + EXISTENTIAL_DEPOSIT);
    });
}

// Scenario "合约内创建合约".
#[test]
fn create2_child_creates_no_issuance() {
    dev_ext().execute_with(|| {
        let alice = Signer::dev("alice");
        let bob = Signer::dev("bob");
        let factory = deploy(&alice, FACTORY);
        let before = gross();
        let b0 = free(&bob.account);
        System::reset_events();
        assert_eq!(
            apply(signed(&bob, contract_call(factory, init_code(&[0x00])))),
            Ok(Ok(()))
        );
        assert_eq!(gross(), before);
        assert!(b0 - free(&bob.account) >= fees_paid() + EXISTENTIAL_DEPOSIT);
    });
}

// Scenario "签名者无力支付".
#[test]
fn signer_who_cannot_pay_fails_to_deploy() {
    dev_ext().execute_with(|| {
        let alice = Signer::dev("alice");
        let poor = Signer::fresh(42, ac_crypto::SigAlg::MlDsa44);
        // Enough for the fee of the transfer-in and a deployment fee, not for the deposits.
        assert_eq!(
            apply(signed(&alice, common::transfer(&poor.account, ATC / 100))),
            Ok(Ok(()))
        );
        let before = gross();
        System::reset_events();
        let result = apply(signed(&poor, deploy_call(init_code(STORE))));
        assert!(matches!(result, Ok(Err(_))), "{result:?}");
        assert!(last_instantiated().is_none());
        assert_eq!(gross(), before);
    });
}

// Requirement "存储押金与手续费" / Scenario "押金冻结与退还".
#[test]
fn storage_deposit_is_held_and_refunded() {
    dev_ext().execute_with(|| {
        let alice = Signer::dev("alice");
        let bob = Signer::dev("bob");
        let contract = deploy(&alice, STORE);
        let b0 = free(&bob.account);
        System::reset_events();
        assert_eq!(
            apply(signed(&bob, contract_call(contract, word(9u8)))),
            Ok(Ok(()))
        );
        let fee1 = fees_paid();
        let deposit = b0 - fee1 - free(&bob.account);
        assert!(deposit > 0, "a storage deposit was charged");
        let b1 = free(&bob.account);
        System::reset_events();
        assert_eq!(
            apply(signed(&bob, contract_call(contract, vec![]))),
            Ok(Ok(()))
        );
        let fee2 = fees_paid();
        assert_eq!(
            free(&bob.account),
            b1 - fee2 + deposit,
            "the deposit came back"
        );
    });
}

// Scenario "超出权重上限".
#[test]
fn out_of_weight_reverts_but_pays() {
    dev_ext().execute_with(|| {
        let alice = Signer::dev("alice");
        let bob = Signer::dev("bob");
        let spin = deploy(&alice, SPIN);
        let store = deploy(&alice, STORE);
        let b0 = free(&bob.account);
        System::reset_events();
        let call = RuntimeCall::Revive(pallet_revive::Call::call {
            dest: spin,
            value: 0,
            weight_limit: Weight::from_parts(100_000_000, 64 * 1024),
            storage_deposit_limit: DEPOSIT_LIMIT,
            data: vec![],
        });
        let result = apply(signed(&bob, call));
        assert!(matches!(result, Ok(Err(_))), "{result:?}");
        let fee = fees_paid();
        assert!(fee > 0);
        assert_eq!(free(&bob.account), b0 - fee);
        assert_eq!(slot(store, 0), Vec::<u8>::new());
    });
}

// Requirement "合约状态可只读查询" / Scenarios "模拟调用" and "回滚原因" (dry run level).
#[test]
fn dry_runs_report_results_and_revert_data() {
    dev_ext().execute_with(|| {
        let alice = Signer::dev("alice");
        let bob = Signer::dev("bob");
        let reverting = deploy(&alice, REVERT_NO);
        let nonce = System::account_nonce(&bob.account);
        let out = dry_call(&bob.account, reverting, vec![]);
        assert!(out.did_revert());
        assert_eq!(out.data[..4], [0x08, 0xc3, 0x79, 0xa0]);
        assert_eq!(out.data[68..70], *b"no");
        assert_eq!(System::account_nonce(&bob.account), nonce);
    });
}

// --- design D3: the call filter ------------------------------------------------------------------

// Requirement "只接受 EVM 字节码" / Scenario "拒绝 PolkaVM 程序".
#[test]
fn polkavm_code_is_refused() {
    dev_ext().execute_with(|| {
        let alice = Signer::dev("alice");
        let mut pvm = POLKAVM_MAGIC.to_vec();
        pvm.extend_from_slice(&[0; 64]);
        assert!(is_filtered(&deploy_call(pvm.clone())));
        let upload = RuntimeCall::Revive(pallet_revive::Call::upload_code {
            code: pvm.clone(),
            storage_deposit_limit: DEPOSIT_LIMIT,
        });
        assert!(is_filtered(&upload));
        let by_hash = RuntimeCall::Revive(pallet_revive::Call::instantiate {
            value: 0,
            weight_limit: weight_limit(),
            storage_deposit_limit: DEPOSIT_LIMIT,
            code_hash: sp_core::H256::zero(),
            data: vec![],
            salt: None,
        });
        assert!(is_filtered(&by_hash));
        System::reset_events();
        let result = apply(signed(&alice, deploy_call(pvm)));
        assert_eq!(
            result,
            Ok(Err(frame_system::Error::<Runtime>::CallFiltered.into()))
        );
        assert!(last_instantiated().is_none());
    });
}

// Requirement "合约交易由后量子签名授权" / Scenario "以太坊原生交易入口被关闭", and
// chain/pq-transaction-auth Scenario "以太坊入口调用被过滤".
#[test]
fn ethereum_entry_points_are_filtered() {
    dev_ext().execute_with(|| {
        let alice = Signer::dev("alice");
        let eth_calls = [
            RuntimeCall::Revive(pallet_revive::Call::eth_transact {
                payload: vec![0xf8, 0x00],
            }),
            RuntimeCall::Revive(pallet_revive::Call::eth_call {
                dest: H160::zero(),
                value: U256::zero(),
                weight_limit: weight_limit(),
                eth_gas_limit: U256::from(1_000_000u32),
                data: vec![],
                transaction_encoded: vec![],
                effective_gas_price: U256::one(),
                encoded_len: 0,
            }),
            RuntimeCall::Revive(pallet_revive::Call::eth_instantiate_with_code {
                value: U256::zero(),
                weight_limit: weight_limit(),
                eth_gas_limit: U256::from(1_000_000u32),
                code: init_code(STORE),
                data: vec![],
                transaction_encoded: vec![],
                effective_gas_price: U256::one(),
                encoded_len: 0,
            }),
        ];
        for call in eth_calls {
            assert!(is_filtered(&call), "{call:?}");
            let wrapped = RuntimeCall::Revive(pallet_revive::Call::dispatch_as_fallback_account {
                call: Box::new(call.clone()),
            });
            assert!(is_filtered(&wrapped));
            let as_root = RuntimeCall::PoaAdmin(pallet_poa_admin::Call::dispatch_as_root {
                call: Box::new(call.clone()),
            });
            assert!(is_filtered(&as_root));
            // `eth_transact` declares `Weight::MAX`, so it is already invalid (never enters the
            // pool or a block); the others reach dispatch and are filtered there.
            let result = apply(signed(&alice, call));
            let filtered = Ok(Err(frame_system::Error::<Runtime>::CallFiltered.into()));
            let invalid = Err(
                sp_runtime::transaction_validity::TransactionValidityError::Invalid(
                    sp_runtime::transaction_validity::InvalidTransaction::ExhaustsResources,
                ),
            );
            assert!(result == filtered || result == invalid, "{result:?}");
        }
        // What stays open: EVM calls and deployments, also wrapped for the fallback account.
        assert!(!is_filtered(&deploy_call(init_code(STORE))));
        assert!(!is_filtered(&contract_call(H160::zero(), vec![])));
        assert!(!is_filtered(&RuntimeCall::Revive(
            pallet_revive::Call::dispatch_as_fallback_account {
                call: Box::new(contract_call(H160::zero(), vec![])),
            }
        )));
        // A wrapped non-Revive call is refused (the fallback account is for contract calls).
        assert!(is_filtered(&RuntimeCall::Revive(
            pallet_revive::Call::dispatch_as_fallback_account {
                call: Box::new(common::transfer(&alice.account, 1)),
            }
        )));
    });
}

// Requirement "账户与 EVM 地址的映射" / Scenario "映射不可解除".
#[test]
fn mapping_cannot_be_removed() {
    dev_ext().execute_with(|| {
        let alice = Signer::dev("alice");
        let original = address_of(&alice.account);
        assert_eq!(account_of(original), alice.account);
        for call in [
            RuntimeCall::Revive(pallet_revive::Call::unmap_account {}),
            RuntimeCall::Revive(pallet_revive::Call::map_account {}),
        ] {
            assert!(is_filtered(&call));
            assert_eq!(
                apply(signed(&alice, call)),
                Ok(Err(frame_system::Error::<Runtime>::CallFiltered.into()))
            );
        }
        assert_eq!(account_of(original), alice.account);
    });
}

// --- chain/pq-transaction-auth: Ethereum transactions never decode --------------------------------

// Scenario "交易池拒绝以太坊交易": legacy (the EIP-155 example), EIP-2930, EIP-1559, EIP-4844 and
// EIP-7702 transactions are not AgentCoin extrinsics; the pool rejects them at decoding, so they
// never reach validation or a block.
#[test]
fn ethereum_transactions_do_not_decode() {
    let legacy = hex::decode(concat!(
        "f86c098504a817c800825208943535353535353535353535353535353535353535880de0b6b3a764000080",
        "25a028ef61340bd939bc2195fe537567866003e1a15d3c71ff63e1590620aa636276a067cbe9d8997f761a",
        "ecb703304b3800ccf555c9f3dc64214b297fb1966a3b6d83"
    ))
    .unwrap();
    // Typed transactions: type byte, then an RLP list (signature values are placeholders; the
    // shape alone is what the pool sees).
    let typed = |ty: u8| {
        let mut tx = vec![ty, 0xf8, 0x50];
        tx.extend_from_slice(&[0x82, 0x11, 0x33]); // chain id 4403
        tx.extend(std::iter::repeat_n(0x80, 0x4d));
        tx
    };
    let candidates = [legacy, typed(0x01), typed(0x02), typed(0x03), typed(0x04)];
    for bytes in candidates {
        assert!(
            UncheckedExtrinsic::decode(&mut &bytes[..]).is_err(),
            "{bytes:02x?}"
        );
        // Wrapped as the node's RPC does (a SCALE byte vector) it does not decode either.
        let wrapped = bytes.encode();
        assert!(UncheckedExtrinsic::decode(&mut &wrapped[..]).is_err());
    }
}

// --- task 2.8: address projection vectors ---------------------------------------------------------

// Requirement "账户与 EVM 地址的映射" / Scenario "地址投影固定": address = keccak256(account)[12..].
#[test]
fn address_projection_vectors() {
    let vectors: [([u8; 32], &str); 2] = [
        ([0x00; 32], "88386fc84ba6bc95484008f6362f93160ef3e563"),
        ([0xff; 32], "ab758a3376d22aedc6a55823d1b3ecbee81b8fb9"),
    ];
    for (account, expected) in vectors {
        let address = address_of(&AccountId::new(account));
        assert_eq!(hex::encode(address.as_bytes()), expected);
    }
    // A real ML-DSA-derived account (development key "alice").
    let alice = Signer::dev("alice");
    let raw: [u8; 32] = *alice.account.as_ref();
    let expected = &sp_io::hashing::keccak_256(&raw)[12..];
    assert_eq!(address_of(&alice.account).as_bytes(), expected);
    assert_eq!(
        hex::encode(address_of(&alice.account).as_bytes()),
        ALICE_EVM_ADDRESS
    );
}

/// EVM address of the development account "alice" (regression vector).
const ALICE_EVM_ADDRESS: &str = "32362c314aa26f1401e81985da5b510d3e957a3d";

// --- ac_primitives::evm::classify_call matches the runtime's encoding -----------------------------

#[test]
fn classifier_matches_runtime_encoding() {
    let call = contract_call(H160::repeat_byte(7), vec![1, 2, 3]);
    let encoded = call.encode();
    assert_eq!(encoded[1], REVIVE_CALL_INDEX);
    match classify_call(&encoded).unwrap() {
        ac_primitives::evm::ContractTransaction::Call(c) => {
            assert_eq!(c.dest, [7; 20]);
            assert_eq!(c.data, vec![1, 2, 3]);
            assert_eq!(c.storage_deposit_limit, DEPOSIT_LIMIT);
            assert_eq!(c.encode_call(), encoded);
        }
        other => panic!("{other:?}"),
    }
    let deploy = deploy_call(init_code(STORE));
    let encoded = deploy.encode();
    match classify_call(&encoded).unwrap() {
        ac_primitives::evm::ContractTransaction::Deploy(d) => {
            assert_eq!(d.code, init_code(STORE));
            assert_eq!(d.encode_call(), encoded);
        }
        other => panic!("{other:?}"),
    }
    assert!(classify_call(&common::transfer(&AccountId::new([1; 32]), 1).encode()).is_err());
}

// --- task 2.9: ReviveApi --------------------------------------------------------------------------

/// Stores the first calldata word in slot 0, or returns slot 0 when called without data (a
/// minimal "setter/getter", standing in for an ERC-20 `balanceOf`).
const REGISTER: &[u8] = &[
    0x36, 0x15, 0x60, 0x0c, 0x57, 0x60, 0x00, 0x35, 0x60, 0x00, 0x55, 0x00, // set
    0x5b, 0x60, 0x00, 0x54, 0x60, 0x00, 0x52, 0x60, 0x20, 0x60, 0x00, 0xf3, // get
];

/// Runs a runtime API call the way the node does: its state changes are discarded. Returns the
/// result and whether the storage root was unchanged afterwards.
fn api<R>(f: impl FnOnce() -> R) -> (R, bool) {
    let before = sp_io::storage::root(sp_runtime::StateVersion::V1);
    let result = frame_support::storage::with_transaction(|| {
        sp_runtime::TransactionOutcome::Rollback(Ok::<_, DispatchError>(f()))
    })
    .unwrap();
    let after = sp_io::storage::root(sp_runtime::StateVersion::V1);
    (result, before == after)
}

// Requirement "合约状态可只读查询" / Scenario "模拟调用": any caller, no signature, no state change.
#[test]
fn revive_api_dry_call_from_any_address() {
    use pallet_revive::runtime_decl_for_revive_api::ReviveApiV1;
    dev_ext().execute_with(|| {
        let alice = Signer::dev("alice");
        let register = deploy(&alice, REGISTER);
        assert_eq!(
            apply(signed(&alice, contract_call(register, word(42u8)))),
            Ok(Ok(()))
        );
        // A caller that has never existed on chain.
        let stranger = AccountId::new([0x5a; 32]);
        let (result, unchanged) = api(|| {
            <Runtime as ReviveApiV1<_, _, _, _, _, _>>::call(
                stranger.clone(),
                register,
                0,
                None,
                None,
                vec![],
            )
        });
        assert_eq!(result.result.unwrap().data, word(42u8));
        assert!(unchanged, "the dry run leaves no trace");
        assert!(!System::account_exists(&stranger));
        let store = register;
        assert_eq!(
            <Runtime as ReviveApiV1<_, _, _, _, _, _>>::code(store),
            REGISTER.to_vec()
        );
        assert_eq!(
            <Runtime as ReviveApiV1<_, _, _, _, _, _>>::nonce(address_of(&alice.account)),
            System::account_nonce(&alice.account)
        );
        assert_eq!(
            <Runtime as ReviveApiV1<_, _, _, _, _, _>>::account_id(address_of(&alice.account)),
            alice.account
        );
    });
}

// Scenario "回滚原因".
#[test]
fn revive_api_reports_revert_data() {
    use pallet_revive::runtime_decl_for_revive_api::ReviveApiV1;
    dev_ext().execute_with(|| {
        let alice = Signer::dev("alice");
        let reverting = deploy(&alice, REVERT_NO);
        let (result, unchanged) = api(|| {
            <Runtime as ReviveApiV1<_, _, _, _, _, _>>::call(
                alice.account.clone(),
                reverting,
                0,
                None,
                None,
                vec![],
            )
        });
        assert!(unchanged);
        let out = result.result.unwrap();
        assert!(out.did_revert());
        assert_eq!(out.data[68..70], *b"no");
    });
}

// Dry-run deployments accept EVM init code only; Ethereum payloads and code uploads are refused.
#[test]
fn revive_api_refuses_polkavm_and_ethereum_paths() {
    use pallet_revive::runtime_decl_for_revive_api::ReviveApiV1;
    dev_ext().execute_with(|| {
        let alice = Signer::dev("alice");
        let (evm, unchanged) = api(|| {
            <Runtime as ReviveApiV1<_, _, _, _, _, _>>::instantiate(
                alice.account.clone(),
                0,
                None,
                None,
                pallet_revive::Code::Upload(init_code(STORE)),
                vec![],
                None,
            )
        });
        assert!(unchanged);
        assert!(evm.result.is_ok());
        assert!(evm.storage_deposit.charge_or_zero() > 0);
        let mut pvm = POLKAVM_MAGIC.to_vec();
        pvm.extend_from_slice(&[0; 16]);
        for code in [
            pallet_revive::Code::Upload(pvm.clone()),
            pallet_revive::Code::Existing(sp_core::H256::zero()),
        ] {
            let (refused, _) = api(|| {
                <Runtime as ReviveApiV1<_, _, _, _, _, _>>::instantiate(
                    alice.account.clone(),
                    0,
                    None,
                    None,
                    code,
                    vec![],
                    None,
                )
            });
            assert_eq!(
                refused.result.err(),
                Some(pallet_revive::Error::<Runtime>::CodeRejected.into())
            );
        }
        assert!(
            <Runtime as ReviveApiV1<_, _, _, _, _, _>>::upload_code(
                alice.account.clone(),
                pvm,
                None
            )
            .is_err()
        );
        assert!(
            <Runtime as ReviveApiV1<_, _, _, _, _, _>>::eth_pre_dispatch_weight(vec![0xf8])
                .is_err()
        );
    });
}
