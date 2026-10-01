//! Audits in the real runtime (m6-audit-chain 4.1, 4.2): only the audit module reaches the
//! provider penalty interface, the treasury floor can fund the audit pot, and a confirmed
//! dispute slashes and jails a provider and voids its unsettled work.
// Test code: failures and overflows panic loudly, which is what a test wants.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

mod common;

use ac_crypto::{KemAlg, KemPublicKey, SigAlg};
use ac_primitives::market::audit::{AuditMetric, FailReason, VerdictOutcome, Vote};
use ac_primitives::market::model::QuantType;
use ac_primitives::market::receipt::{RECEIPT_CONTEXT, fee_for};
use ac_primitives::market::records::{ModelPrice, ProviderStatus, Tier};
use ac_primitives::market::runtime_decl_for_audit_api::AuditApiV1;
use ac_primitives::market::voucher::VOUCHER_CONTEXT;
use ac_primitives::market::work::JobKind;
use ac_primitives::market::{
    MicroUsd, ModelId, ModelManifest, PricePerMTok, ReceiptBody, ReportEntry, SignedReceipt,
    SignedVoucher, VoucherBody,
};
use ac_runtime::{
    ATC, AccountId, Audit, Balance, Providers, Runtime, RuntimeCall, RuntimeOrigin, System, Work,
};
use common::{Signer, apply, dev_ext, next_block, signed};
use frame_support::BoundedVec;
use frame_support::traits::fungible::{Inspect, Mutate};
use sp_core::H256;
use sp_runtime::traits::Dispatchable;

const PRICE: PricePerMTok = PricePerMTok {
    input: MicroUsd(100_000),
    output: MicroUsd(200_000),
};

fn ok(signer: &Signer, call: RuntimeCall) {
    let result = apply(signed(signer, call)).expect("valid transaction");
    assert!(result.is_ok(), "{result:?}");
}

fn free(who: &AccountId) -> Balance {
    <ac_runtime::Balances as Inspect<AccountId>>::balance(who)
}

fn register_model(signer: &Signer) -> ModelId {
    let m = ModelManifest::new(
        b"Qwen2.5-0.5B-Instruct",
        b"qwen2",
        QuantType::Bf16,
        vec![[1; 32]],
    )
    .unwrap();
    ok(
        signer,
        RuntimeCall::ModelRegistry(pallet_model_registry::Call::register {
            manifest: m.clone(),
            lineage: None,
            license_tag: BoundedVec::truncate_from(b"apache-2.0".to_vec()),
            royalty: None,
        }),
    );
    m.id().unwrap()
}

fn register_provider(signer: &Signer, model: ModelId) {
    ok(
        signer,
        RuntimeCall::Providers(pallet_providers::Call::register {
            registration: pallet_providers::Registration {
                tier: Tier::T2,
                endpoint: BoundedVec::truncate_from(b"https://provider.example".to_vec()),
                kem_pk: KemPublicKey::new(KemAlg::XWing, &[7; 1216]).unwrap(),
                models: BoundedVec::truncate_from(vec![ModelPrice {
                    model,
                    price: PRICE,
                }]),
                stake: 100 * ATC,
                attestation: None,
            },
        }),
    );
}

fn receipt(provider: &Signer, gateway: &Signer, model: ModelId, id: u8) -> SignedReceipt {
    let body = ReceiptBody {
        genesis: System::block_hash(0),
        gateway: gateway.account.clone(),
        provider: provider.account.clone(),
        kind: JobKind::Inference,
        model,
        request_id: [id; 32],
        in_tokens: 1_000,
        out_tokens: 2_000,
        fee: fee_for(&PRICE, 1_000, 2_000).unwrap(),
        toploc_commit: [9; 32],
        ttft_ms: 50,
        total_ms: 900,
    };
    let payload = body.payload().unwrap();
    SignedReceipt {
        provider_key: provider.public(),
        provider_sig: provider
            .key
            .sign_deterministic(&payload, RECEIPT_CONTEXT)
            .unwrap(),
        gateway_key: gateway.public(),
        gateway_sig: gateway
            .key
            .sign_deterministic(&payload, RECEIPT_CONTEXT)
            .unwrap(),
        body,
    }
}

fn voucher(user: &Signer, gateway: &AccountId, cumulative: u128) -> SignedVoucher {
    let body = VoucherBody {
        genesis: System::block_hash(0),
        user: user.account.clone(),
        gateway: gateway.clone(),
        channel: 0,
        cumulative: MicroUsd(cumulative),
    };
    SignedVoucher {
        signature: user
            .key
            .sign_deterministic(&body.payload().unwrap(), VOUCHER_CONTEXT)
            .unwrap(),
        public_key: user.public(),
        body,
    }
}

// Spec market/audit "处罚" / "管理权限不能处罚" and market/providers "管理多签不能禁闭提供者":
// no runtime call slashes, jails or decides a dispute directly.
#[test]
fn no_call_punishes_directly() {
    use frame_support::traits::GetCallMetadata;
    for module in <RuntimeCall as GetCallMetadata>::get_module_names() {
        let calls = <RuntimeCall as GetCallMetadata>::get_call_names(module);
        if *module == "Audit" {
            assert_eq!(
                calls,
                [
                    "register",
                    "bond_extra",
                    "unbond",
                    "exit",
                    "withdraw_unbonded",
                    "submit_verdict",
                    "vote",
                    "close_dispute",
                    "set_params"
                ]
            );
        }
        for call in calls {
            let lower = call.to_lowercase();
            let punishes =
                (lower.contains("slash") || lower.contains("jail") || lower.contains("decide"))
                    && *module != "StakingPos";
            assert!(!punishes, "{module}::{call} could punish directly");
        }
    }
}

// Design D9: an "audit" spend of the treasury floor can fund the audit pot (a transfer).
#[test]
fn the_floor_funds_the_audit_pot() {
    dev_ext().execute_with(|| {
        let floor = pallet_treasury_dual::Pallet::<Runtime>::floor_account();
        ac_runtime::Balances::set_balance(&floor, 11 * ATC);
        pallet_treasury_dual::FloorMatured::<Runtime>::put(10 * ATC);
        let pot = Audit::pot();
        let issuance = ac_runtime::Balances::total_issuance();
        let call = RuntimeCall::TreasuryDual(pallet_treasury_dual::Call::spend_floor {
            purpose: pallet_treasury_dual::FloorPurpose::Audit,
            to: pot.clone(),
            amount: 10 * ATC,
        });
        assert!(call.dispatch(RuntimeOrigin::root()).is_ok());
        assert_eq!(free(&pot), 10 * ATC);
        assert_eq!(ac_runtime::Balances::total_issuance(), issuance);
        assert_eq!(<Runtime as AuditApiV1<_, _, _>>::pot(), (pot, 10 * ATC));
    });
}

// Task 4.2: verdicts on real receipts, a dispute decided by real votes, then
// market/providers "审计确认触达接口" and market/work-settlement "审计确认使待领取款项作废".
#[test]
fn a_confirmed_dispute_jails_and_voids_unsettled_work() {
    dev_ext().execute_with(|| {
        let (alice, bob, charlie) = (
            Signer::dev("alice"),
            Signer::dev("bob"),
            Signer::dev("charlie"),
        );
        let model = register_model(&alice);
        register_provider(&bob, model);
        ok(
            &charlie,
            RuntimeCall::Gateways(pallet_gateways::Call::register {
                endpoint: BoundedVec::truncate_from(b"https://gateway.example".to_vec()),
                fee_bps: 300,
                stake: 1_000 * ATC,
            }),
        );
        ok(
            &alice,
            RuntimeCall::Credits(pallet_credits::Call::deposit {
                gateway: charlie.account.clone(),
                amount: 10 * ATC,
            }),
        );
        // A report for $0.60 of bob's work, maturing in emission epoch 2 (blocks 21..=30).
        ok(
            &charlie,
            RuntimeCall::Work(pallet_work::Call::submit_report {
                root: [0xab; 32],
                receipt_count: 1,
                entries: BoundedVec::truncate_from(vec![ReportEntry {
                    kind: JobKind::Inference,
                    provider: bob.account.clone(),
                    model,
                    usd: MicroUsd(600_000),
                    in_tokens: 1_000,
                    out_tokens: 2_000,
                }]),
                vouchers: BoundedVec::truncate_from(vec![voucher(
                    &alice,
                    &charlie.account,
                    600_000,
                )]),
            }),
        );
        assert!(Work::epoch_work(2).verified > 0);

        // Seven funded auditors, a funded pot, and randomness for the round seeds.
        let auditors: Vec<Signer> = (1..=7u8)
            .map(|s| Signer::fresh(s, SigAlg::MlDsa44))
            .collect();
        for a in &auditors {
            ac_runtime::Balances::set_balance(&a.account, 10_000 * ATC);
            ok(
                a,
                RuntimeCall::Audit(pallet_audit::Call::register { stake: 1_000 * ATC }),
            );
        }
        ac_runtime::Balances::set_balance(&Audit::pot(), 100 * ATC);
        pallet_randomness_cr::Latest::<Runtime>::put((0, H256([5; 32])));

        // Round 1 of the dev chain starts at block 21.
        while System::block_number() < 21 {
            next_block();
        }
        let (round, start, _) = <Runtime as AuditApiV1<_, _, _>>::round().unwrap();
        assert_eq!((round, start), (1, 21));
        assert_eq!(<Runtime as AuditApiV1<_, _, _>>::roster(1).len(), 7);
        let assigned = <Runtime as AuditApiV1<_, _, _>>::assignment(1, bob.account.clone());
        assert_eq!(assigned.len(), 2);
        let signer = |who: &AccountId| auditors.iter().find(|a| a.account == *who).unwrap();
        let fail = VerdictOutcome::Fail(FailReason::Threshold {
            chunk: 1,
            metric: AuditMetric::MantissaMean,
        });
        for (i, who) in assigned.iter().enumerate() {
            let verdict = pallet_audit::VerdictSubmission {
                provider: bob.account.clone(),
                round: 1,
                outcome: fail,
                thresholds_version: 2,
                evidence: Some([i as u8; 32]),
                receipt: receipt(&bob, &charlie, model, 10 + i as u8),
            };
            ok(
                signer(who),
                RuntimeCall::Audit(pallet_audit::Call::submit_verdict {
                    verdict: Box::new(verdict),
                }),
            );
        }
        let assigned_to = <Runtime as AuditApiV1<_, _, _>>::assigned_to(1, assigned[0].clone());
        assert_eq!(assigned_to, vec![(bob.account.clone(), true)]);
        let id = <Runtime as AuditApiV1<_, _, _>>::open_dispute(bob.account.clone()).unwrap();
        let dispute = <Runtime as AuditApiV1<_, _, _>>::dispute(id).unwrap();
        assert_eq!(dispute.reviewers.len(), 3);

        let stake = Providers::provider(&bob.account).unwrap().stake;
        let burned = ac_runtime::Emission::total_burned();
        for (reviewer, _) in dispute.reviewers.iter().take(2) {
            ok(
                signer(reviewer),
                RuntimeCall::Audit(pallet_audit::Call::vote {
                    provider: bob.account.clone(),
                    id,
                    vote: Vote::Confirm,
                }),
            );
        }
        let p = Providers::provider(&bob.account).unwrap();
        assert_eq!(p.status, ProviderStatus::Jailed);
        assert_eq!(p.stake, stake - stake / 10);
        assert!(ac_runtime::Emission::total_burned() - burned >= stake / 10);
        assert!(!Providers::is_serviceable(&bob.account));
        // The report has not matured: bob's work leaves epoch 2.
        assert_eq!(Work::epoch_work(2).verified, 0);
        assert_eq!(
            <Runtime as AuditApiV1<_, _, _>>::provider_stats(bob.account.clone()).confirmed,
            1
        );
    });
}
