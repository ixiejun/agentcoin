//! Work settlement in the real runtime (m5-work-settlement 5.2, 5.3): a full flow across epochs
//! with the node's issuance check on every block, the `WorkApi` against storage, the weight of a
//! full report, and no call that voids pending payments.
// Test code: failures and overflows panic loudly, which is what a test wants.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

mod common;

use ac_crypto::{KemAlg, KemPublicKey};
use ac_invariants::{GenesisParams, Ledger, check_block, check_genesis, read_ledger};
use ac_primitives::market::model::QuantType;
use ac_primitives::market::records::{ModelPrice, Tier};
use ac_primitives::market::runtime_decl_for_work_api::WorkApiV1;
use ac_primitives::market::voucher::VOUCHER_CONTEXT;
use ac_primitives::market::work::JobKind;
use ac_primitives::market::{
    MicroUsd, ModelId, ModelManifest, PricePerMTok, ReportEntry, SignedVoucher, VoucherBody,
};
use ac_runtime::{ATC, AccountId, Balance, Executive, Runtime, RuntimeCall, System, Work};
use common::{Signer, apply, dev_ext, signed, start_block};
use frame_support::BoundedVec;
use frame_support::traits::fungible::{Inspect, InspectHold};

fn ok(signer: &Signer, call: RuntimeCall) {
    let result = apply(signed(signer, call)).expect("valid transaction");
    assert!(result.is_ok(), "{result:?}");
}

fn free(who: &AccountId) -> Balance {
    <ac_runtime::Balances as Inspect<AccountId>>::balance(who)
}

fn held_pending(who: &AccountId) -> Balance {
    <ac_runtime::Balances as InspectHold<AccountId>>::balance_on_hold(
        &ac_runtime::RuntimeHoldReason::Work(pallet_work::HoldReason::Pending),
        who,
    )
}

fn get(k: &[u8]) -> Option<Vec<u8>> {
    sp_io::storage::get(k).map(|v| v.to_vec())
}

/// The node's genesis parameters, read from the current state as the node reads the genesis.
fn node_params() -> GenesisParams {
    let mut entries = Vec::new();
    let mut key = Vec::new();
    while let Some(next) = sp_io::storage::next_key(&key) {
        entries.push((next.clone(), get(&next).unwrap()));
        key = next;
    }
    check_genesis(
        entries.iter().map(|(k, v)| (k.as_slice(), v.as_slice())),
        false,
    )
    .unwrap()
}

/// Finalizes the current block, checks it with the node's per-block rule and starts the next.
fn step(params: &GenesisParams, prev: &mut Ledger) {
    let number = System::block_number();
    pallet_timestamp::Pallet::<Runtime>::set_timestamp(
        u64::from(number) * ac_runtime::MILLISECS_PER_BLOCK,
    );
    let header = Executive::finalize_block();
    let post = read_ledger(get).unwrap();
    check_block(params, u64::from(header.number), *prev, post)
        .unwrap_or_else(|v| panic!("block {}: {v}", header.number));
    *prev = post;
    start_block(header.number + 1, header.hash());
}

fn register_model(signer: &Signer) -> ModelId {
    let m = ModelManifest::new(
        b"Qwen2.5-0.5B-Instruct",
        b"qwen2",
        QuantType::Int4,
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
                    price: PricePerMTok {
                        input: MicroUsd(100_000),
                        output: MicroUsd(200_000),
                    },
                }]),
                stake: 100 * ATC,
                attestation: None,
            },
        }),
    );
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

fn entry(provider: &AccountId, model: ModelId, micro_usd: u128) -> ReportEntry<AccountId> {
    ReportEntry {
        kind: JobKind::Inference,
        provider: provider.clone(),
        model,
        usd: MicroUsd(micro_usd),
        unproven: MicroUsd(0),
        in_tokens: 1_000,
        out_tokens: 2_000,
    }
}

// Task 5.3 with 5.2: deposit → voucher → report → challenge period → emission settlement →
// claims, every block passing the node's issuance check; the WorkApi reads what is stored.
#[test]
fn a_settlement_flow_across_epochs() {
    dev_ext().execute_with(|| {
        let params = node_params();
        let mut ledger = read_ledger(get).unwrap();
        let (alice, bob, charlie, dave) = (
            Signer::dev("alice"),
            Signer::dev("bob"),
            Signer::dev("charlie"),
            Signer::dev("dave"),
        );
        let model = register_model(&alice);
        register_provider(&bob, model);
        register_provider(&dave, model);
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
        step(&params, &mut ledger);

        // $0.80 of receipts: bob $0.60, dave $0.20 (1 ATC = 1 USD on the dev chain).
        let burned_before = ac_runtime::Emission::total_burned();
        ok(
            &charlie,
            RuntimeCall::Work(pallet_work::Call::submit_report {
                root: [0xab; 32],
                receipt_count: 2,
                entries: BoundedVec::truncate_from(vec![
                    entry(&bob.account, model, 600_000),
                    entry(&dave.account, model, 200_000),
                ]),
                vouchers: BoundedVec::truncate_from(vec![voucher(
                    &alice,
                    &charlie.account,
                    800_000,
                )]),
            }),
        );
        let g = 8 * ATC / 10;
        let burn = g / 5;
        let fee = g * 3 / 100;
        let pool = g - burn - fee;
        let (bob_share, dave_share) = (pool * 3 / 4, pool / 4);
        // The report's burn (the transaction fee is burned too, so the counter grows by more).
        assert_eq!(Work::report(0).unwrap().burned, burn);
        assert!(ac_runtime::Emission::total_burned() - burned_before >= burn);
        assert_eq!(held_pending(&charlie.account), fee + bob_share + dave_share);

        // WorkApi against storage.
        let report = <Runtime as WorkApiV1<_, _, _>>::report(0).unwrap();
        assert_eq!(
            Some(report.clone()),
            pallet_work::Reports::<Runtime>::get(0)
        );
        assert_eq!(
            (report.settled, report.submitted, report.matures),
            (g, 0, 2)
        );
        assert_eq!(
            <Runtime as WorkApiV1<_, _, _>>::params(),
            ac_primitives::market::WorkParams {
                retention_epochs: 20,
                ..ac_primitives::market::WorkParams::LIVE
            }
        );
        let held = <Runtime as WorkApiV1<_, _, _>>::held(bob.account.clone());
        assert_eq!(held, Work::held(&bob.account));
        assert_eq!(held[0].2.shares, bob_share);
        let work = <Runtime as WorkApiV1<_, _, _>>::work(bob.account.clone());
        assert_eq!(work[0].1.work, g * 3 / 4 / 2);
        assert_eq!(
            <Runtime as WorkApiV1<_, _, _>>::epoch_work(2).verified,
            g / 2
        );
        assert_eq!(
            <Runtime as WorkApiV1<_, _, _>>::lifetime(bob.account.clone()).pending,
            g * 3 / 8
        );

        // During the challenge period nothing can be claimed.
        let bob_free = free(&bob.account);
        let items = |g: &AccountId| BoundedVec::truncate_from(vec![(2u64, g.clone())]);
        ok(
            &alice,
            RuntimeCall::Work(pallet_work::Call::claim {
                who: bob.account.clone(),
                items: items(&charlie.account),
            }),
        );
        assert!(
            free(&bob.account) <= bob_free,
            "nothing paid before maturity"
        );

        // Epoch 2 is blocks 21..=30 of the dev chain (10-block epochs); block 31 settles it.
        while System::block_number() < 32 {
            step(&params, &mut ledger);
        }
        let market = Work::epoch_work(2).market.unwrap();
        // Work 0.4 ATC is below 50% × avail: all of it is minted; the pot keeps one deposit.
        let ed = <ac_runtime::Balances as Inspect<AccountId>>::minimum_balance();
        assert_eq!(market, g / 2 - ed);
        assert_eq!(free(&Work::pot()), g / 2);

        for (who, share, emission) in [
            (&bob, bob_share, market * 3 / 4),
            (&dave, dave_share, market / 4),
        ] {
            let before = free(&who.account);
            ok(
                &alice,
                RuntimeCall::Work(pallet_work::Call::claim {
                    who: who.account.clone(),
                    items: items(&charlie.account),
                }),
            );
            assert_eq!(free(&who.account) - before, share + emission);
        }
        let before = free(&charlie.account);
        ok(
            &alice,
            RuntimeCall::Work(pallet_work::Call::claim {
                who: charlie.account.clone(),
                items: items(&charlie.account),
            }),
        );
        // The claim fee is paid by alice; charlie gets its fee back.
        assert_eq!(free(&charlie.account) - before, fee);
        assert_eq!(held_pending(&charlie.account), 0);
        assert_eq!(free(&Work::pot()), ed);
        step(&params, &mut ledger);
    });
}

// Task 5.3: a full report (16 ML-DSA-87 vouchers, 128 entries) stays under half the weight a
// normal transaction may use.
#[test]
fn a_full_report_fits_comfortably_in_a_block() {
    use frame_support::dispatch::DispatchClass;
    use pallet_work::WeightInfo;
    let limit = <Runtime as frame_system::Config>::BlockWeights::get()
        .get(DispatchClass::Normal)
        .max_extrinsic
        .expect("normal transactions are limited");
    let full = pallet_work::weights::SubstrateWeight::<Runtime>::submit_report(
        ac_primitives::market::work::MAX_REPORT_VOUCHERS,
        ac_primitives::market::work::MAX_REPORT_ENTRIES,
    );
    assert!(
        full.ref_time() * 2 <= limit.ref_time(),
        "{full:?} vs half of {limit:?}"
    );
    assert!(full.proof_size() * 2 <= limit.proof_size());
}

// Scenario "无人能在本阶段触发": no runtime call voids pending payments or jails a provider;
// settlement's calls are exactly submitting and claiming.
#[test]
fn no_call_voids_pending_payments() {
    use frame_support::traits::GetCallMetadata;
    for module in <RuntimeCall as GetCallMetadata>::get_module_names() {
        let calls = <RuntimeCall as GetCallMetadata>::get_call_names(module);
        if *module == "Work" {
            assert_eq!(calls, ["submit_report", "claim"]);
        }
        for call in calls {
            let lower = call.to_lowercase();
            let voids = lower.contains("void");
            let jails = lower.contains("jail") && *module != "StakingPos";
            assert!(
                !voids && !jails,
                "{module}::{call} could void pending payments"
            );
        }
    }
}
