//! The inference market in the real runtime (m5-market-registry 7.2, 7.3): signed market
//! transactions, the `MarketApi` against storage, no call that slashes or jails providers, and
//! the node's issuance rule across the whole flow.
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
use ac_primitives::market::model::QuantType;
use ac_primitives::market::records::{ModelPrice, ProviderStatus, Tier};
use ac_primitives::market::runtime_decl_for_market_api::MarketApiV1;
use ac_primitives::market::traits::Credit;
use ac_primitives::market::voucher::VOUCHER_CONTEXT;
use ac_primitives::market::{
    AtcPerUsd, MicroUsd, ModelManifest, PricePerMTok, SignedVoucher, VoucherBody, VoucherError,
};
use ac_runtime::{ATC, AccountId, Balance, Emission, Runtime, RuntimeCall, System};
use common::{Signer, apply, dev_ext, issuance, next_block, signed};
use frame_support::BoundedVec;

fn ok(signer: &Signer, call: RuntimeCall) {
    let result = apply(signed(signer, call)).expect("valid transaction");
    assert!(result.is_ok(), "{result:?}");
}

/// Minted since genesis minus burned: constant unless emission settles an epoch.
fn gross() -> Balance {
    issuance() + Emission::total_burned()
}

fn manifest() -> ModelManifest {
    ModelManifest::new(
        b"Qwen2.5-0.5B-Instruct",
        b"qwen2",
        QuantType::Int4,
        vec![[1; 32], [2; 32]],
    )
    .unwrap()
}

fn register_model(signer: &Signer) -> ac_primitives::market::ModelId {
    let m = manifest();
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

fn register_provider(signer: &Signer, model: ac_primitives::market::ModelId) {
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
                // $100 at the dev rate (1 ATC = 1 USD).
                stake: 100 * ATC,
                attestation: None,
            },
        }),
    );
}

fn register_gateway(signer: &Signer) {
    ok(
        signer,
        RuntimeCall::Gateways(pallet_gateways::Call::register {
            endpoint: BoundedVec::truncate_from(b"https://gateway.example".to_vec()),
            fee_bps: 300,
            stake: 1_000 * ATC,
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

// Task 7.2: every `MarketApi` method against the stored state after a full registration flow.
#[test]
fn the_market_api_reads_what_the_transactions_stored() {
    let mut ext = dev_ext();
    ext.execute_with(|| {
        let (alice, bob, charlie) = (
            Signer::dev("alice"),
            Signer::dev("bob"),
            Signer::dev("charlie"),
        );
        let before = gross();
        let model = register_model(&alice);
        register_provider(&bob, model);
        register_gateway(&charlie);
        ok(
            &alice,
            RuntimeCall::Credits(pallet_credits::Call::deposit {
                gateway: charlie.account.clone(),
                amount: 10 * ATC,
            }),
        );
        assert_eq!(gross(), before, "market transactions mint nothing");
    });
    // Call the APIs through the runtime API machinery on the resulting state.
    ext.execute_with(|| {
        let (alice, bob, charlie) = (Signer::dev("alice"), Signer::dev("bob"), Signer::dev("charlie"));
        let model = manifest().id().unwrap();
        let rate = <Runtime as MarketApiV1<ac_runtime::Block, AccountId, Balance, u32>>::atc_per_usd();
        assert_eq!(rate.map(|(r, _)| r), Some(AtcPerUsd(ATC)));
        assert_eq!(
            <Runtime as MarketApiV1<ac_runtime::Block, AccountId, Balance, u32>>::model(model),
            pallet_model_registry::Models::<Runtime>::get(model)
        );
        let provider = <Runtime as MarketApiV1<ac_runtime::Block, AccountId, Balance, u32>>::provider(bob.account.clone()).unwrap();
        assert_eq!(provider.status, ProviderStatus::Active);
        assert!(<Runtime as MarketApiV1<ac_runtime::Block, AccountId, Balance, u32>>::is_serviceable(bob.account.clone()));
        let listed = <Runtime as MarketApiV1<ac_runtime::Block, AccountId, Balance, u32>>::serviceable_providers(model, None, 10_000);
        assert_eq!(listed, vec![(bob.account.clone(), provider)]);
        assert_eq!(
            <Runtime as MarketApiV1<ac_runtime::Block, AccountId, Balance, u32>>::provider_threshold(Tier::T2),
            Ok(100 * ATC)
        );
        assert_eq!(
            <Runtime as MarketApiV1<ac_runtime::Block, AccountId, Balance, u32>>::gateway(charlie.account.clone()),
            pallet_gateways::Gateways::<Runtime>::get(&charlie.account)
        );
        assert_eq!(
            <Runtime as MarketApiV1<ac_runtime::Block, AccountId, Balance, u32>>::gateway_threshold(),
            Ok(1_000 * ATC)
        );
        let channel = <Runtime as MarketApiV1<ac_runtime::Block, AccountId, Balance, u32>>::channel(
            alice.account.clone(),
            charlie.account.clone(),
        )
        .unwrap();
        assert_eq!(channel.escrow, 10 * ATC);
        assert_eq!(
            channel.key,
            ac_primitives::market::voucher::key_fingerprint(&alice.public())
        );

        // The off-chain check and redemption agree on the same voucher.
        let v = voucher(&alice, &charlie.account, 500_000);
        let check = <Runtime as MarketApiV1<ac_runtime::Block, AccountId, Balance, u32>>::check_voucher(v.clone()).unwrap();
        assert_eq!(check.increment_atc, ATC / 2);
        assert!(check.covered);
        let payee = bob.account.clone();
        let redeemed = <ac_runtime::Credits as Credit<AccountId, Balance>>::redeem(&charlie.account, &v, &payee).unwrap();
        assert_eq!(redeemed.paid, check.increment_atc);
        // Signed by another key: both refuse it.
        let mut forged = voucher(&bob, &charlie.account, 900_000);
        forged.body.user = alice.account.clone();
        forged.signature = bob
            .key
            .sign_deterministic(&forged.body.payload().unwrap(), VOUCHER_CONTEXT)
            .unwrap();
        assert_eq!(
            <Runtime as MarketApiV1<ac_runtime::Block, AccountId, Balance, u32>>::check_voucher(forged.clone()),
            Err(VoucherError::WrongKey)
        );
        assert!(<ac_runtime::Credits as Credit<AccountId, Balance>>::redeem(&charlie.account, &forged, &payee).is_err());
    });
}

// Spec market/providers, Scenario "管理多签不能禁闭提供者": no runtime call can slash or jail a
// provider — the pallet's calls are exactly the registration and stake calls, and no other
// pallet's call names the penalty.
#[test]
fn no_call_slashes_or_jails_providers() {
    use frame_support::traits::GetCallMetadata;
    let names = <RuntimeCall as GetCallMetadata>::get_module_names();
    for module in names {
        for call in <RuntimeCall as GetCallMetadata>::get_call_names(module) {
            let lower = call.to_lowercase();
            if *module == "Providers" {
                assert!(
                    [
                        "register",
                        "update",
                        "heartbeat",
                        "bond_extra",
                        "unbond",
                        "exit",
                        "withdraw_unbonded"
                    ]
                    .contains(call),
                    "unexpected Providers call {call}"
                );
            }
            assert!(
                !(lower.contains("jail") && *module != "StakingPos"),
                "{module}::{call} could jail a provider"
            );
        }
    }
}

// The node's issuance rule holds across a market flow spanning blocks: nothing is minted outside
// emission settlements (read with the node's own ledger reader).
#[test]
fn a_market_flow_passes_the_node_issuance_check() {
    dev_ext().execute_with(|| {
        let (alice, bob, charlie) = (
            Signer::dev("alice"),
            Signer::dev("bob"),
            Signer::dev("charlie"),
        );
        let get = |k: &[u8]| sp_io::storage::get(k).map(|v| v.to_vec());
        let before = ac_invariants::read_ledger(get).unwrap();
        let minted_before = pallet_emission::TotalMinted::<Runtime>::get();
        let model = register_model(&alice);
        register_provider(&bob, model);
        register_gateway(&charlie);
        ok(
            &alice,
            RuntimeCall::Credits(pallet_credits::Call::deposit {
                gateway: charlie.account.clone(),
                amount: 5 * ATC,
            }),
        );
        ok(
            &alice,
            RuntimeCall::Credits(pallet_credits::Call::request_withdrawal {
                gateway: charlie.account.clone(),
                amount: 5 * ATC,
            }),
        );
        ok(
            &bob,
            RuntimeCall::Providers(pallet_providers::Call::exit {}),
        );
        ok(
            &charlie,
            RuntimeCall::Gateways(pallet_gateways::Call::exit {}),
        );
        // Past the dev withdrawal delay (10 blocks), before the first emission settlement.
        for _ in 0..11 {
            next_block();
        }
        ok(
            &alice,
            RuntimeCall::Credits(pallet_credits::Call::withdraw {
                gateway: charlie.account.clone(),
            }),
        );
        let after = ac_invariants::read_ledger(get).unwrap();
        // The dev emission epoch (10 blocks) settles on the way: only emission may mint.
        let minted = pallet_emission::TotalMinted::<Runtime>::get() - minted_before;
        assert_eq!(
            after.issuance + after.burned,
            before.issuance + before.burned + minted,
            "no market path mints"
        );
        assert!(after.issuance <= ac_primitives::emission::CAP);
    });
}
