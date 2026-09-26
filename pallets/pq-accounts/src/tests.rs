//! Unit tests. Test names cite the spec Requirement / Scenario they cover
//! (chain/pq-transaction-auth, chain/pq-accounts).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable,
    clippy::indexing_slicing
)]

use ac_crypto::sig::{SecretSeed, SigningKey};
use ac_crypto::{PqPublicKey, PqSignature, SigAlg};
use frame_support::dispatch::GetDispatchInfo;
use frame_support::{assert_noop, assert_ok};
use parity_scale_codec::{Decode, Encode};
use sp_runtime::traits::DispatchTransaction;
use sp_runtime::transaction_validity::InvalidTransaction;
use sp_runtime::{AccountId32, DispatchError};

use crate::mock::{PqAccounts, RuntimeCall, RuntimeOrigin, System, Test, new_test_ext};
use crate::{
    Error, KEY_ROTATION_CONTEXT, Keys, PqAuth, PqAuthorize, TX_SIGNING_CONTEXT, derived_account,
    rotation_statement, signing_payload,
};

const EXT_VERSION: u8 = 0;

fn key(seed: u8, alg: SigAlg) -> SigningKey {
    SigningKey::from_seed(alg, &SecretSeed::new([seed; 32])).unwrap()
}

fn pk(k: &SigningKey) -> PqPublicKey {
    k.public_key().unwrap()
}

fn remark(n: u8) -> RuntimeCall {
    RuntimeCall::System(frame_system::Call::remark { remark: vec![n] })
}

fn sign_call(k: &SigningKey, call: &RuntimeCall, context: &[u8]) -> PqSignature {
    let payload = signing_payload(&(EXT_VERSION, call)).unwrap();
    k.sign_deterministic(&payload, context).unwrap()
}

fn auth(
    who: &AccountId32,
    k: &SigningKey,
    call: &RuntimeCall,
    carry_key: bool,
) -> PqAuthorize<Test> {
    PqAuthorize::new(PqAuth::Signed {
        who: who.clone(),
        signature: sign_call(k, call, TX_SIGNING_CONTEXT),
        public_key: carry_key.then(|| pk(k)),
    })
}

/// Validates and, if valid, dispatches `call` through the extension.
fn submit(
    ext: PqAuthorize<Test>,
    call: RuntimeCall,
) -> Result<Result<(), DispatchError>, InvalidTransaction> {
    let info = call.get_dispatch_info();
    match ext.dispatch_transaction(RuntimeOrigin::none(), call, &info, 0, EXT_VERSION) {
        Ok(res) => Ok(res.map(|_| ()).map_err(|e| e.error)),
        Err(sp_runtime::transaction_validity::TransactionValidityError::Invalid(e)) => Err(e),
        Err(other) => panic!("unexpected {other:?}"),
    }
}

fn register(k: &SigningKey) -> AccountId32 {
    let who = derived_account(&pk(k));
    let call = remark(0);
    assert_eq!(submit(auth(&who, k, &call, true), call), Ok(Ok(())));
    who
}

// ---------------------------------------------------------------- chain/pq-accounts: registry

// Requirement "公钥登记表" / Scenario "查询当前公钥".
#[test]
fn query_current_key_after_first_transaction() {
    new_test_ext().execute_with(|| {
        let k = key(1, SigAlg::MlDsa44);
        let who = register(&k);
        assert_eq!(PqAccounts::current_key(&who), Some((pk(&k), 0)));
    });
}

// Requirement "公钥登记表" / Scenario "查询未登记账户".
#[test]
fn query_unregistered_account() {
    new_test_ext().execute_with(|| {
        let who = derived_account(&pk(&key(2, SigAlg::MlDsa44)));
        assert_eq!(PqAccounts::current_key(&who), None);
    });
}

// chain/state-hashing Requirement "存储键哈希器的有界例外" / Scenario "自有模块的账户键".
#[test]
fn registry_storage_key_ends_with_account_id() {
    new_test_ext().execute_with(|| {
        let who = AccountId32::new([0xAB; 32]);
        let raw = Keys::<Test>::hashed_key_for(&who);
        assert_eq!(
            &raw[raw.len() - 32..],
            <AccountId32 as AsRef<[u8]>>::as_ref(&who)
        );
    });
}

// ------------------------------------------------------- chain/pq-transaction-auth: PqAuthorize

// Requirement "首笔交易登记公钥" / Scenario "首笔交易完成登记".
#[test]
fn first_transaction_registers_key() {
    new_test_ext().execute_with(|| {
        let k = key(3, SigAlg::MlDsa44);
        let who = register(&k);
        assert!(Keys::<Test>::contains_key(&who));
        System::assert_has_event(crate::Event::<Test>::KeyRegistered { who, alg: 0x01 }.into());
    });
}

// Requirement "首笔交易登记公钥" / Scenario "公钥与账户不符".
#[test]
fn key_must_derive_the_account() {
    new_test_ext().execute_with(|| {
        let a = key(4, SigAlg::MlDsa44);
        let b = key(5, SigAlg::MlDsa44);
        let who_a = derived_account(&pk(&a));
        let call = remark(1);
        // Signature by B with B's key, claiming account A.
        assert_eq!(
            submit(auth(&who_a, &b, &call, true), call),
            Err(InvalidTransaction::BadSigner)
        );
        assert!(!Keys::<Test>::contains_key(&who_a));
    });
}

// Requirement "首笔交易登记公钥" / Scenario "已登记账户重复携带公钥".
#[test]
fn registered_account_must_not_carry_key() {
    new_test_ext().execute_with(|| {
        let k = key(6, SigAlg::MlDsa44);
        let who = register(&k);
        let call = remark(2);
        assert_eq!(
            submit(auth(&who, &k, &call, true), call),
            Err(InvalidTransaction::BadSigner)
        );
    });
}

// Requirement "首笔交易登记公钥": an unregistered account must carry its key.
#[test]
fn unregistered_account_must_carry_key() {
    new_test_ext().execute_with(|| {
        let k = key(7, SigAlg::MlDsa44);
        let who = derived_account(&pk(&k));
        let call = remark(3);
        assert_eq!(
            submit(auth(&who, &k, &call, false), call),
            Err(InvalidTransaction::BadSigner)
        );
    });
}

// Requirement "首笔交易登记公钥" / Scenario "后续交易不再携带公钥".
#[test]
fn later_transactions_verify_against_registry_and_are_shorter() {
    new_test_ext().execute_with(|| {
        let k = key(8, SigAlg::MlDsa44);
        let who = register(&k);
        let call = remark(4);
        let without = auth(&who, &k, &call, false);
        let with = auth(&who, &k, &call, true);
        assert!(with.encoded_size() - without.encoded_size() >= 1312);
        assert_eq!(submit(without, call), Ok(Ok(())));
    });
}

// Requirement "签名内容与域分离" / Scenario "篡改调用导致失败".
#[test]
fn tampered_call_fails() {
    new_test_ext().execute_with(|| {
        let k = key(9, SigAlg::MlDsa44);
        let who = register(&k);
        let ext = auth(&who, &k, &remark(5), false);
        assert_eq!(submit(ext, remark(6)), Err(InvalidTransaction::BadProof));
    });
}

// Requirement "签名内容与域分离" / Scenario "错误上下文的签名无效".
#[test]
fn wrong_context_fails() {
    new_test_ext().execute_with(|| {
        let k = key(10, SigAlg::MlDsa44);
        let who = register(&k);
        let call = remark(7);
        let ext = PqAuthorize::<Test>::new(PqAuth::Signed {
            who,
            signature: sign_call(&k, &call, b"agentcoin/aura-seal/v1"),
            public_key: None,
        });
        assert_eq!(submit(ext, call), Err(InvalidTransaction::BadProof));
    });
}

// Requirement "只接受 PQ 授权的交易" / Scenario "无签名的普通调用被拒绝".
#[test]
fn unauthorized_call_gets_no_account_origin() {
    new_test_ext().execute_with(|| {
        let call = RuntimeCall::PqAccounts(crate::Call::rotate_key {
            new_key: pk(&key(11, SigAlg::MlDsa44)),
            proof: sign_call(&key(11, SigAlg::MlDsa44), &remark(0), KEY_ROTATION_CONTEXT),
        });
        // The SDK refuses a general transaction whose origin no extension authorized, so the
        // transaction is rejected before it could reach the pool.
        assert_eq!(
            submit(PqAuthorize::new(PqAuth::None), call),
            Err(InvalidTransaction::UnknownOrigin)
        );
    });
}

// Requirement "验证失败绝不 panic" / Scenario "预留算法的签名".
#[test]
fn reserved_algorithm_signature_does_not_decode() {
    let k = key(12, SigAlg::MlDsa44);
    let who = derived_account(&pk(&k));
    let mut bytes = auth(&who, &k, &remark(0), false).encode();
    // Layout: variant (1) ‖ account (32) ‖ signature AlgId (1) ‖ …; switch to SLH-DSA (0x10).
    bytes[33] = 0x10;
    assert!(PqAuthorize::<Test>::decode(&mut &bytes[..]).is_err());
}

// ------------------------------------------------------------ chain/pq-accounts: rotate_key

fn rotation_call(who: &AccountId32, rotations: u32, new: &SigningKey) -> RuntimeCall {
    let genesis = System::block_hash(0);
    let statement = rotation_statement(&genesis, who, rotations, &pk(new));
    RuntimeCall::PqAccounts(crate::Call::rotate_key {
        new_key: pk(new),
        proof: new
            .sign_deterministic(&statement, KEY_ROTATION_CONTEXT)
            .unwrap(),
    })
}

// Requirement "密钥轮换" / Scenario "轮换后账户 ID 不变".
#[test]
fn rotation_keeps_account_id() {
    new_test_ext().execute_with(|| {
        let old = key(20, SigAlg::MlDsa44);
        let new = key(21, SigAlg::MlDsa65);
        let who = register(&old);
        let call = rotation_call(&who, 0, &new);
        assert_eq!(submit(auth(&who, &old, &call, false), call), Ok(Ok(())));
        assert_eq!(PqAccounts::current_key(&who), Some((pk(&new), 1)));
        System::assert_has_event(
            crate::Event::<Test>::KeyRotated {
                who,
                alg: 0x02,
                rotations: 1,
            }
            .into(),
        );
    });
}

// Requirement "密钥轮换" / Scenarios "轮换后旧密钥失效" and "轮换后新密钥可用".
#[test]
fn old_key_fails_and_new_key_works_after_rotation() {
    new_test_ext().execute_with(|| {
        let old = key(22, SigAlg::MlDsa44);
        let new = key(23, SigAlg::MlDsa65);
        let who = register(&old);
        let call = rotation_call(&who, 0, &new);
        assert_eq!(submit(auth(&who, &old, &call, false), call), Ok(Ok(())));

        let call = remark(8);
        assert_eq!(
            submit(auth(&who, &old, &call, false), call.clone()),
            Err(InvalidTransaction::BadProof)
        );
        assert_eq!(submit(auth(&who, &new, &call, false), call), Ok(Ok(())));
    });
}

// Requirement "密钥轮换" / Scenario "缺少或伪造持有证明".
#[test]
fn forged_proofs_are_rejected() {
    new_test_ext().execute_with(|| {
        let old = key(24, SigAlg::MlDsa44);
        let new = key(25, SigAlg::MlDsa44);
        let who = register(&old);
        let genesis = System::block_hash(0);

        // Proof signed by the old key instead of the new one.
        let statement = rotation_statement(&genesis, &who, 0, &pk(&new));
        let by_old = old
            .sign_deterministic(&statement, KEY_ROTATION_CONTEXT)
            .unwrap();
        assert_noop!(
            PqAccounts::rotate_key(RuntimeOrigin::signed(who.clone()), pk(&new), by_old),
            Error::<Test>::BadProof
        );

        // Proof over a statement with the wrong rotation count.
        let stale = rotation_statement(&genesis, &who, 1, &pk(&new));
        let wrong_count = new
            .sign_deterministic(&stale, KEY_ROTATION_CONTEXT)
            .unwrap();
        assert_noop!(
            PqAccounts::rotate_key(RuntimeOrigin::signed(who.clone()), pk(&new), wrong_count),
            Error::<Test>::BadProof
        );
        assert_eq!(PqAccounts::current_key(&who), Some((pk(&old), 0)));
    });
}

// Requirement "密钥轮换" / Scenario "持有证明不能跨账户重放".
#[test]
fn proof_cannot_be_replayed_for_another_account() {
    new_test_ext().execute_with(|| {
        let a = key(26, SigAlg::MlDsa44);
        let b = key(27, SigAlg::MlDsa44);
        let new = key(28, SigAlg::MlDsa44);
        let who_a = register(&a);
        let who_b = register(&b);
        let genesis = System::block_hash(0);
        let statement_a = rotation_statement(&genesis, &who_a, 0, &pk(&new));
        let proof_a = new
            .sign_deterministic(&statement_a, KEY_ROTATION_CONTEXT)
            .unwrap();
        assert_noop!(
            PqAccounts::rotate_key(RuntimeOrigin::signed(who_b), pk(&new), proof_a),
            Error::<Test>::BadProof
        );
    });
}

// Requirement "公钥不可被他人占用" / Scenario "轮换到他人的公钥".
#[test]
fn cannot_rotate_to_another_accounts_key() {
    new_test_ext().execute_with(|| {
        let a = key(29, SigAlg::MlDsa44);
        let b = key(30, SigAlg::MlDsa44);
        let who_a = register(&a);
        register(&b);
        let genesis = System::block_hash(0);
        let statement = rotation_statement(&genesis, &who_a, 0, &pk(&b));
        let proof = b
            .sign_deterministic(&statement, KEY_ROTATION_CONTEXT)
            .unwrap();
        assert_noop!(
            PqAccounts::rotate_key(RuntimeOrigin::signed(who_a), pk(&b), proof),
            Error::<Test>::KeyInUse
        );
    });
}

// Requirement "公钥不可被他人占用": a key whose derived account already exists is refused.
#[test]
fn cannot_rotate_to_key_of_existing_unregistered_account() {
    new_test_ext().execute_with(|| {
        let a = key(31, SigAlg::MlDsa44);
        let c = key(32, SigAlg::MlDsa44);
        let who_a = register(&a);
        let who_c = derived_account(&pk(&c));
        System::inc_providers(&who_c);
        let genesis = System::block_hash(0);
        let statement = rotation_statement(&genesis, &who_a, 0, &pk(&c));
        let proof = c
            .sign_deterministic(&statement, KEY_ROTATION_CONTEXT)
            .unwrap();
        assert_noop!(
            PqAccounts::rotate_key(RuntimeOrigin::signed(who_a), pk(&c), proof),
            Error::<Test>::KeyInUse
        );
    });
}

// Requirement "预留算法的迁移路径" / Scenario "轮换到未实现的算法".
#[test]
fn rotation_to_reserved_algorithm_does_not_decode() {
    new_test_ext().execute_with(|| {
        let a = key(33, SigAlg::MlDsa44);
        let who = register(&a);
        let mut bytes = rotation_call(&who, 0, &key(34, SigAlg::MlDsa44)).encode();
        // Layout: pallet index (1) ‖ call index (1) ‖ new key AlgId (1) ‖ …; switch to FN-DSA.
        bytes[2] = 0x20;
        assert!(RuntimeCall::decode(&mut &bytes[..]).is_err());
        assert_eq!(PqAccounts::current_key(&who), Some((pk(&a), 0)));
    });
}

// rotate_key requires a registered account.
#[test]
fn rotation_requires_registration() {
    new_test_ext().execute_with(|| {
        let a = key(35, SigAlg::MlDsa44);
        let who = derived_account(&pk(&a));
        let call = rotation_call(&who, 0, &key(36, SigAlg::MlDsa44));
        let RuntimeCall::PqAccounts(crate::Call::rotate_key { new_key, proof }) = call else {
            unreachable!()
        };
        assert_noop!(
            PqAccounts::rotate_key(RuntimeOrigin::signed(who), new_key, proof),
            Error::<Test>::NotRegistered
        );
    });
}

// Rotating back to the account's original key is allowed.
#[test]
fn rotating_back_to_the_original_key_is_allowed() {
    new_test_ext().execute_with(|| {
        let first = key(37, SigAlg::MlDsa44);
        let second = key(38, SigAlg::MlDsa65);
        let who = register(&first);
        let call = rotation_call(&who, 0, &second);
        assert_ok!(submit(auth(&who, &first, &call, false), call).unwrap());
        let call = rotation_call(&who, 1, &first);
        assert_ok!(submit(auth(&who, &second, &call, false), call).unwrap());
        assert_eq!(PqAccounts::current_key(&who), Some((pk(&first), 2)));
    });
}
