//! Runtime-level transaction tests (chain/pq-transaction-auth, chain/native-token,
//! chain/pq-accounts). Tasks 5.2 and 5.3 of m1-pq-chain.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::too_many_arguments
)]

mod common;

use ac_crypto::SigAlg;
use ac_runtime::transaction::ChainContext;
use ac_runtime::{ATC, EXISTENTIAL_DEPOSIT, Executive, RuntimeCall, UncheckedExtrinsic};
use common::{
    Signer, apply, build, context, dev_ext, fees_paid, free, immortal, issuance, next_block,
    signed, transfer,
};
use parity_scale_codec::{Decode, Encode};
use sp_core::H256;
use sp_runtime::DispatchError;
use sp_runtime::generic::Era;
use sp_runtime::transaction_validity::{
    InvalidTransaction, TransactionSource, TransactionValidityError,
};

fn invalid(e: InvalidTransaction) -> Result<sp_runtime::DispatchOutcome, TransactionValidityError> {
    Err(TransactionValidityError::Invalid(e))
}

// Requirement "转账" / Scenario "成功转账".
#[test]
fn successful_transfer() {
    dev_ext().execute_with(|| {
        let alice = Signer::dev("alice");
        let bob = Signer::dev("bob");
        let (a0, b0) = (free(&alice.account), free(&bob.account));
        assert_eq!(
            apply(signed(&alice, transfer(&bob.account, 10 * ATC))),
            Ok(Ok(()))
        );
        let fee = fees_paid();
        assert!(fee > 0);
        assert_eq!(free(&alice.account), a0 - 10 * ATC - fee);
        assert_eq!(free(&bob.account), b0 + 10 * ATC);
    });
}

// Requirement "转账" / Scenario "余额不足".
#[test]
fn insufficient_balance_fails_but_pays_fee() {
    dev_ext().execute_with(|| {
        let alice = Signer::dev("alice");
        let bob = Signer::dev("bob");
        let (a0, b0) = (free(&alice.account), free(&bob.account));
        let result = apply(signed(&alice, transfer(&bob.account, a0 + 1)));
        assert!(matches!(result, Ok(Err(_))), "{result:?}");
        assert_eq!(free(&alice.account), a0 - fees_paid());
        assert_eq!(free(&bob.account), b0);
    });
}

// Requirement "转账": zero amounts and transfers leaving the sender below the existential deposit
// (without explicitly allowing death) fail.
#[test]
fn zero_and_below_existential_deposit_transfers() {
    dev_ext().execute_with(|| {
        let alice = Signer::dev("alice");
        let fresh = Signer::fresh(1, SigAlg::MlDsa44);
        let before = free(&fresh.account);
        let below_ed = transfer(&fresh.account, EXISTENTIAL_DEPOSIT - 1);
        assert!(matches!(apply(signed(&alice, below_ed)), Ok(Err(_))));
        assert_eq!(free(&fresh.account), before);
        let keep_alive = RuntimeCall::Balances(pallet_balances::Call::transfer_keep_alive {
            dest: fresh.account.clone(),
            value: free(&alice.account),
        });
        assert!(matches!(apply(signed(&alice, keep_alive)), Ok(Err(_))));
    });
}

// Requirement "只接受 PQ 授权的交易" / Scenario "ML-DSA 签名的交易被接受", including a first
// transaction from an account that only received funds (chain/pq-accounts "向未登记账户转账").
#[test]
fn fresh_account_receives_then_sends() {
    dev_ext().execute_with(|| {
        let alice = Signer::dev("alice");
        let fresh = Signer::fresh(2, SigAlg::MlDsa44);
        assert_eq!(
            apply(signed(&alice, transfer(&fresh.account, 5 * ATC))),
            Ok(Ok(()))
        );
        assert_eq!(free(&fresh.account), 5 * ATC);
        assert!(pallet_pq_accounts::Keys::<ac_runtime::Runtime>::get(&fresh.account).is_none());
        assert_eq!(
            apply(signed(&fresh, transfer(&alice.account, ATC))),
            Ok(Ok(()))
        );
        assert!(pallet_pq_accounts::Keys::<ac_runtime::Runtime>::get(&fresh.account).is_some());
    });
}

// Requirement "只接受 PQ 授权的交易" / Scenario "旧式经典签名交易被拒绝".
#[test]
fn legacy_signed_extrinsics_do_not_decode() {
    let call = transfer(&Signer::dev("bob").account, 1).encode();
    // v4 signed extrinsic: 0x84 ‖ address (32) ‖ 64-byte "signature" ‖ empty extensions ‖ call.
    for signature in [vec![0u8; 64], vec![1u8; 65], vec![0x01; 1 + 64]] {
        let mut body = vec![0x84];
        body.extend([0x11; 32]);
        body.extend(&signature);
        body.extend(&call);
        let bytes = (
            parity_scale_codec::Compact(u32::try_from(body.len()).unwrap()),
            body,
        )
            .encode();
        assert!(UncheckedExtrinsic::decode(&mut &bytes[..]).is_err());
    }
}

// Requirement "只接受 PQ 授权的交易" / Scenario "无签名的普通调用被拒绝".
#[test]
fn unauthorized_transfer_is_rejected() {
    dev_ext().execute_with(|| {
        let bob = Signer::dev("bob");
        // No account signature: `transfer` does not authorize itself, so it is rejected.
        let xt = ac_runtime::transaction::assemble_unsigned(transfer(&bob.account, ATC));
        assert_eq!(apply(xt), invalid(InvalidTransaction::UnknownOrigin));
    });
}

// Requirement "签名内容与域分离" / Scenario "篡改调用导致失败".
#[test]
fn tampered_amount_is_rejected() {
    dev_ext().execute_with(|| {
        let alice = Signer::dev("alice");
        let bob = Signer::dev("bob");
        let xt = signed(&alice, transfer(&bob.account, ATC));
        let mut bytes = xt.encode();
        let genuine = transfer(&bob.account, ATC).encode();
        let forged = transfer(&bob.account, 2 * ATC).encode();
        let at = bytes
            .windows(genuine.len())
            .position(|w| w == genuine.as_slice())
            .unwrap();
        bytes.splice(at..at + genuine.len(), forged.clone());
        assert_eq!(
            genuine.len(),
            forged.len(),
            "same-length amounts keep the framing valid"
        );
        let xt = UncheckedExtrinsic::decode(&mut &bytes[..]).unwrap();
        assert_eq!(apply(xt), invalid(InvalidTransaction::BadProof));
    });
}

// Requirement "签名内容与域分离" / Scenario "其他链的交易不能重放".
#[test]
fn transaction_for_another_genesis_is_rejected() {
    dev_ext().execute_with(|| {
        let alice = Signer::dev("alice");
        let bob = Signer::dev("bob");
        let other = ChainContext {
            genesis_hash: H256([0x99; 32]),
            ..context()
        };
        let params = ac_runtime::transaction::TxParams {
            era_birth_hash: other.genesis_hash,
            ..immortal(0)
        };
        let xt = build(
            &alice,
            &alice.key,
            transfer(&bob.account, ATC),
            true,
            params,
            other,
        );
        assert_eq!(apply(xt), invalid(InvalidTransaction::BadProof));
    });
}

// Requirement "防重放与有效期" / Scenario "重放同一交易".
#[test]
fn replayed_transaction_is_stale() {
    dev_ext().execute_with(|| {
        let alice = Signer::dev("alice");
        let bob = Signer::dev("bob");
        let xt = signed(&alice, transfer(&bob.account, ATC));
        assert_eq!(apply(xt.clone()), Ok(Ok(())));
        // The replay carries the public key again, which a registered account may not do; a
        // replay of a later (key-less) transaction fails on the nonce.
        let second = signed(&alice, transfer(&bob.account, ATC));
        assert_eq!(apply(second.clone()), Ok(Ok(())));
        assert_eq!(apply(second), invalid(InvalidTransaction::Stale));
        assert!(apply(xt).is_err());
    });
}

// Requirement "防重放与有效期" / Scenario "过期交易".
#[test]
fn expired_transaction_is_rejected() {
    dev_ext().execute_with(|| {
        let alice = Signer::dev("alice");
        let bob = Signer::dev("bob");
        for _ in 0..3 {
            next_block();
        }
        // Mortal for 4 blocks, born at block 2.
        let birth = frame_system::Pallet::<ac_runtime::Runtime>::block_hash(2);
        let params = ac_runtime::transaction::TxParams {
            nonce: 0,
            tip: 0,
            era: Era::mortal(4, 2),
            era_birth_hash: birth,
        };
        let xt = build(
            &alice,
            &alice.key,
            transfer(&bob.account, ATC),
            true,
            params,
            context(),
        );
        for _ in 0..6 {
            next_block();
        }
        // Once the era has passed, the chain resolves the birth block to a different hash than
        // the one that was signed, so the signature no longer matches.
        let result = apply(xt);
        assert!(
            matches!(
                result,
                Err(TransactionValidityError::Invalid(
                    InvalidTransaction::BadProof | InvalidTransaction::AncientBirthBlock
                ))
            ),
            "{result:?}"
        );
    });
}

// Requirement "首笔交易登记公钥" / Scenario "后续交易不再携带公钥", and "长度计费".
#[test]
fn later_transactions_are_shorter_and_cheaper() {
    dev_ext().execute_with(|| {
        let alice = Signer::dev("alice");
        let bob = Signer::dev("bob");
        let first = signed(&alice, transfer(&bob.account, ATC));
        let first_len = first.encoded_size();
        let first_fee = ac_runtime::TransactionPayment::query_info(
            first.clone(),
            u32::try_from(first_len).unwrap(),
        )
        .partial_fee;
        assert_eq!(apply(first), Ok(Ok(())));
        let later = signed(&alice, transfer(&bob.account, ATC));
        let later_len = later.encoded_size();
        let later_fee =
            ac_runtime::TransactionPayment::query_info(later, u32::try_from(later_len).unwrap())
                .partial_fee;
        assert!(first_len - later_len >= 1312, "{first_len} vs {later_len}");
        assert!(first_fee > later_fee);
    });
}

// Pool validation of a well-formed transaction succeeds without changing state.
#[test]
fn pool_validation_does_not_register() {
    dev_ext().execute_with(|| {
        let fresh = Signer::fresh(3, SigAlg::MlDsa44);
        let alice = Signer::dev("alice");
        assert_eq!(
            apply(signed(&alice, transfer(&fresh.account, ATC))),
            Ok(Ok(()))
        );
        let xt = signed(&fresh, transfer(&alice.account, 1));
        let block = frame_system::Pallet::<ac_runtime::Runtime>::parent_hash();
        assert!(Executive::validate_transaction(TransactionSource::External, xt, block).is_ok());
        assert!(pallet_pq_accounts::Keys::<ac_runtime::Runtime>::get(&fresh.account).is_none());
    });
}

// chain/pq-accounts Requirement "密钥轮换" through full transactions: rotate, old key fails,
// new key works, account ID and balance unchanged.
#[test]
fn rotation_through_runtime() {
    dev_ext().execute_with(|| {
        let alice = Signer::dev("alice");
        let bob = Signer::dev("bob");
        assert_eq!(
            apply(signed(&alice, transfer(&bob.account, ATC))),
            Ok(Ok(()))
        );
        let new = Signer::fresh(9, SigAlg::MlDsa65);
        let genesis = frame_system::Pallet::<ac_runtime::Runtime>::block_hash(0);
        let statement =
            pallet_pq_accounts::rotation_statement(&genesis, &alice.account, 0, &new.public());
        let proof = new
            .key
            .sign_deterministic(&statement, pallet_pq_accounts::KEY_ROTATION_CONTEXT)
            .unwrap();
        let rotate = RuntimeCall::PqAccounts(pallet_pq_accounts::Call::rotate_key {
            new_key: new.public(),
            proof,
        });
        assert_eq!(apply(signed(&alice, rotate)), Ok(Ok(())));

        let nonce = frame_system::Pallet::<ac_runtime::Runtime>::account_nonce(&alice.account);
        let old = build(
            &alice,
            &alice.key,
            transfer(&bob.account, 1),
            false,
            immortal(nonce),
            context(),
        );
        assert_eq!(apply(old), invalid(InvalidTransaction::BadProof));
        let with_new = build(
            &alice,
            &new.key,
            transfer(&bob.account, 1),
            false,
            immortal(nonce),
            context(),
        );
        assert_eq!(apply(with_new), Ok(Ok(())));
        let (key, rotations) = ac_runtime::PqAccounts::current_key(&alice.account).unwrap();
        assert_eq!((key, rotations), (new.public(), 1));
    });
}

// Requirement "手续费与销毁": a block without an author (no pre-runtime digest) burns the whole
// fee; the 80/20 split with an author is tested in `economics.rs`.
#[test]
fn fees_are_burned() {
    dev_ext().execute_with(|| {
        let alice = Signer::dev("alice");
        let bob = Signer::dev("bob");
        let before = issuance();
        for _ in 0..3 {
            assert_eq!(
                apply(signed(&alice, transfer(&bob.account, ATC))),
                Ok(Ok(()))
            );
        }
        assert_eq!(before - issuance(), fees_paid());
    });
}

// Wrong dispatch origin of rotate_key never panics the runtime.
#[test]
fn dispatch_errors_are_reported() {
    dev_ext().execute_with(|| {
        let alice = Signer::dev("alice");
        let bogus = RuntimeCall::PqAccounts(pallet_pq_accounts::Call::rotate_key {
            new_key: alice.public(),
            proof: alice
                .key
                .sign_deterministic(b"x", pallet_pq_accounts::KEY_ROTATION_CONTEXT)
                .unwrap(),
        });
        let result = apply(signed(&alice, bogus));
        assert!(
            matches!(result, Ok(Err(DispatchError::Module(_)))),
            "{result:?}"
        );
    });
}
