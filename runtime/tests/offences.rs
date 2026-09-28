//! Runtime tests of double-signing reports through the transaction pipeline
//! (chain/pq-transaction-auth "无账户来源的授权调用", consensus/offences). Tasks 4.1 and 4.4 of
//! m2-finality.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

mod common;

use ac_crypto::SigAlg;
use ac_crypto::sig::SigningKey;
use ac_primitives::aura_pq::{SEAL_CONTEXT, Slot, pre_digest, seal_digest};
use ac_primitives::offences::{ChainHeader, EncodedHeader, Evidence};
use ac_runtime::transaction::{
    assemble_unsigned, authorized_extensions, implicit_from, report_extrinsic,
};
use ac_runtime::{
    ATC, AccountId, Executive, Runtime, RuntimeCall, RuntimeEvent, System, UncheckedExtrinsic,
};
use common::{Signer, apply, context, dev_ext, free, immortal, issuance, signed, transfer};
use pallet_pq_accounts::PqAuthorize;
use parity_scale_codec::{Decode, Encode};
use sp_core::H256;
use sp_runtime::traits::Header as _;
use sp_runtime::transaction_validity::{
    InvalidTransaction, TransactionSource, TransactionValidityError,
};
use sp_runtime::{Digest, generic};

fn authority() -> SigningKey {
    SigningKey::from_seed(SigAlg::MlDsa65, &ac_crypto::dev_seed("alice").unwrap()).unwrap()
}

fn sealed(key: &SigningKey, root: u8) -> EncodedHeader {
    let mut digest = Digest::default();
    digest.push(pre_digest(Slot::from(0)));
    let mut header = ChainHeader::new(
        1,
        H256::repeat_byte(root),
        H256::zero(),
        H256::zero(),
        digest,
    );
    let sig = key
        .sign_deterministic(header.hash().as_ref(), SEAL_CONTEXT)
        .unwrap();
    header.digest_mut().push(seal_digest(&sig));
    EncodedHeader::try_from(header.encode()).unwrap()
}

fn evidence(key: &SigningKey) -> Evidence {
    Evidence::AuraEquivocation {
        offender: key.public_key().unwrap(),
        first: sealed(key, 1),
        second: sealed(key, 2),
    }
}

fn report(evidence: Evidence) -> UncheckedExtrinsic {
    assemble_unsigned(RuntimeCall::Offences(
        pallet_ac_offences::Call::report_equivocation { evidence },
    ))
}

fn dev_accounts() -> Vec<AccountId> {
    ["alice", "bob", "charlie", "dave"]
        .iter()
        .map(|n| Signer::dev(n).account)
        .collect()
}

// Scenario "证据举报无需签名": a report without any authorization is valid for the pool and
// executes without charging anyone or touching any nonce.
#[test]
fn report_without_signature_is_free() {
    dev_ext().execute_with(|| {
        let xt = report(evidence(&authority()));
        let validity = Executive::validate_transaction(
            TransactionSource::External,
            xt.clone(),
            System::parent_hash(),
        )
        .unwrap();
        assert!(!validity.provides.is_empty());
        let balances: Vec<_> = dev_accounts().iter().map(free).collect();
        let nonces: Vec<_> = dev_accounts().iter().map(System::account_nonce).collect();
        let supply = issuance();

        assert_eq!(apply(xt), Ok(Ok(())));
        let reported = System::events().into_iter().any(|e| {
            matches!(
                e.event,
                RuntimeEvent::Offences(pallet_ac_offences::Event::OffenceReported {
                    slashed: 0,
                    ..
                })
            )
        });
        assert!(reported);
        assert_eq!(
            dev_accounts().iter().map(free).collect::<Vec<_>>(),
            balances
        );
        assert_eq!(
            dev_accounts()
                .iter()
                .map(System::account_nonce)
                .collect::<Vec<_>>(),
            nonces
        );
        assert_eq!(issuance(), supply);
    });
}

// Scenario "无效证据被拒绝": the pool rejects a report whose evidence is forged.
#[test]
fn invalid_report_is_rejected_by_the_pool() {
    dev_ext().execute_with(|| {
        let key = authority();
        let other =
            SigningKey::from_seed(SigAlg::MlDsa65, &ac_crypto::dev_seed("bob").unwrap()).unwrap();
        let forged = Evidence::AuraEquivocation {
            offender: key.public_key().unwrap(),
            first: sealed(&key, 1),
            second: sealed(&other, 2),
        };
        let bad = Err(TransactionValidityError::Invalid(
            InvalidTransaction::BadProof,
        ));
        assert_eq!(
            Executive::validate_transaction(
                TransactionSource::External,
                report(forged.clone()),
                System::parent_hash()
            ),
            bad
        );
        assert_eq!(apply(report(forged)), bad.map(|_| Ok(())));
        // A duplicate of a recorded offence is stale.
        assert_eq!(apply(report(evidence(&key))), Ok(Ok(())));
        assert_eq!(
            apply(report(evidence(&key))),
            Err(TransactionValidityError::Invalid(InvalidTransaction::Stale))
        );
    });
}

// Task 4.4: the runtime API builds a ready-to-submit report for valid, new evidence only.
#[test]
fn report_extrinsic_api() {
    dev_ext().execute_with(|| {
        let key = authority();
        let xt = report_extrinsic(evidence(&key)).unwrap();
        assert!(
            Executive::validate_transaction(
                TransactionSource::Local,
                xt.clone(),
                System::parent_hash()
            )
            .is_ok()
        );
        assert_eq!(apply(xt), Ok(Ok(())));
        assert!(report_extrinsic(evidence(&key)).is_none());
        assert_eq!(ac_runtime::Offences::offences(0).len(), 1);
    });
}

// Scenarios "同一集合内的另一类违规" and "第二类违规不重复处置" through the runtime: a vote
// offence and then a seal offence of alice are both recorded; a second one of either kind is
// refused by the pool; issuance and balances do not change.
#[test]
fn both_kinds_are_recorded_once_each() {
    dev_ext().execute_with(|| {
        let before = (
            issuance(),
            dev_accounts().iter().map(free).collect::<Vec<_>>(),
        );
        assert_eq!(
            apply(common::report(common::vote_evidence("alice", 0, 1, false))),
            Ok(Ok(()))
        );
        assert_eq!(apply(report(evidence(&authority()))), Ok(Ok(())));
        assert_eq!(
            apply(common::report(common::vote_evidence("alice", 0, 2, false))),
            Err(TransactionValidityError::Invalid(InvalidTransaction::Stale))
        );
        assert!(report_extrinsic(common::seal_evidence("alice", 5, false)).is_none());
        let kinds: Vec<_> = ac_runtime::Offences::offences(0)
            .iter()
            .map(|(_, k)| k.kind())
            .collect();
        assert_eq!(
            kinds,
            vec![
                ac_primitives::offences::OffenceKind::BftEquivocation,
                ac_primitives::offences::OffenceKind::AuraEquivocation
            ]
        );
        assert_eq!(
            (
                issuance(),
                dev_accounts().iter().map(free).collect::<Vec<_>>()
            ),
            before
        );
    });
}

/// The M1 extension tuple, without `AuthorizeCall`.
type M1Extension = (
    PqAuthorize<Runtime>,
    frame_system::CheckNonZeroSender<Runtime>,
    frame_system::CheckSpecVersion<Runtime>,
    frame_system::CheckTxVersion<Runtime>,
    frame_system::CheckGenesis<Runtime>,
    frame_system::CheckMortality<Runtime>,
    frame_system::CheckNonce<Runtime>,
    frame_system::CheckWeight<Runtime>,
    pallet_transaction_payment::ChargeTransactionPayment<Runtime>,
    frame_system::WeightReclaim<Runtime>,
);
type M1Extrinsic = generic::UncheckedExtrinsic<
    AccountId,
    RuntimeCall,
    ac_primitives::NoClassicSignature,
    M1Extension,
>;

// Scenario "已有账户交易不受影响": account transactions keep their M1 encoding and signed
// payload; the new extension adds no bytes.
#[test]
fn account_transactions_keep_their_encoding() {
    dev_ext().execute_with(|| {
        let alice = Signer::dev("alice");
        let bob = Signer::dev("bob");
        let xt = signed(&alice, transfer(&bob.account, ATC));
        let bytes = xt.encode();
        // Decodes as the M1 transaction type and re-encodes to the same bytes.
        let m1 = M1Extrinsic::decode(&mut &bytes[..]).unwrap();
        assert_eq!(m1.encode(), bytes);
        // The explicit and implicit data covered by the signature encode as in M1:
        // `AuthorizeCall` (M2) and `SetEvmPayer` (M4) carry no data.
        let params = immortal(0);
        let (_authorize, a, b, c, d, e, f, g, h, _set_evm_payer, i) =
            authorized_extensions(&params);
        assert_eq!(
            authorized_extensions(&params).encode(),
            (a, b, c, d, e, f, g, h, i).encode()
        );
        let (_authorize, a, b, c, d, e, f, g, h, _set_evm_payer, i) =
            implicit_from(&context(), &params);
        assert_eq!(
            implicit_from(&context(), &params).encode(),
            (a, b, c, d, e, f, g, h, i).encode()
        );
        assert_eq!(apply(xt), Ok(Ok(())));
    });
}
