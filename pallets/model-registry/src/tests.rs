//! Unit tests; each names the spec `market/model-registry` scenario it covers.

use frame_support::traits::fungible::{Inspect, InspectHold};
use frame_support::{BoundedVec, assert_noop, assert_ok};
use parity_scale_codec::Encode;

use ac_primitives::market::model::{Lineage, LineageKind, QuantType, RoyaltySpec};
use ac_primitives::market::traits::ModelLookup;
use ac_primitives::market::{ModelId, ModelManifest};

use crate::mock::{
    ALICE, BOB, Balances, FUNDS, ModelRegistry, POOR, RuntimeOrigin, System, Test, ext,
};
use crate::{Error, Event, HoldReason, Models};

fn manifest(name: &[u8], quant: QuantType, shards: &[u8]) -> ModelManifest {
    ModelManifest::new(
        name,
        b"qwen2",
        quant,
        shards.iter().map(|n| [*n; 32]).collect(),
    )
    .unwrap()
}

fn register(who: u64, m: &ModelManifest, lineage: Option<Lineage>) -> sp_runtime::DispatchResult {
    ModelRegistry::register(
        RuntimeOrigin::signed(who),
        m.clone(),
        lineage,
        BoundedVec::truncate_from(b"apache-2.0".to_vec()),
        None,
    )
}

fn held(who: u64) -> u128 {
    Balances::balance_on_hold(
        &crate::mock::RuntimeHoldReason::ModelRegistry(HoldReason::Deposit),
        &who,
    )
}

// Requirement "模型 ID 由权重清单确定", Scenario "链下复算".
#[test]
fn the_chain_computes_the_same_id_as_off_chain() {
    ext().execute_with(|| {
        let m = manifest(b"Qwen2.5-0.5B-Instruct", QuantType::Int4, &[1, 2]);
        assert_ok!(register(ALICE, &m, None));
        let id = m.id().unwrap();
        let record = ModelRegistry::model(&id).unwrap();
        assert_eq!(record.manifest, m);
        assert_eq!(record.owner, ALICE);
        assert!(<ModelRegistry as ModelLookup>::exists(&id));
        System::assert_last_event(
            Event::ModelRegistered {
                id,
                owner: ALICE,
                deposit: record.deposit,
            }
            .into(),
        );
    });
}

// Requirement "无许可登记与去重", Scenario "重复登记".
#[test]
fn a_manifest_registers_once() {
    ext().execute_with(|| {
        let m = manifest(b"m", QuantType::Bf16, &[1]);
        assert_ok!(register(ALICE, &m, None));
        assert_noop!(register(BOB, &m, None), Error::<Test>::AlreadyRegistered);
        assert_eq!(ModelRegistry::model(&m.id().unwrap()).unwrap().owner, ALICE);
    });
}

// Scenario "超出上限": an oversized manifest cannot even be built or decoded as a call argument;
// invalid text is refused by the pallet.
#[test]
fn bounds_and_text_are_enforced() {
    ext().execute_with(|| {
        assert!(ModelManifest::new(b"m", b"a", QuantType::Fp8, vec![[0; 32]; 1_025]).is_err());
        let mut bad = manifest(b"m", QuantType::Fp8, &[1]);
        bad.name = BoundedVec::truncate_from(vec![0xff]);
        assert_noop!(register(ALICE, &bad, None), Error::<Test>::InvalidManifest);
        let m = manifest(b"n", QuantType::Fp8, &[1]);
        assert_noop!(
            ModelRegistry::register(
                RuntimeOrigin::signed(ALICE),
                m,
                None,
                BoundedVec::truncate_from(vec![0xff]),
                None
            ),
            Error::<Test>::InvalidLicenseTag
        );
    });
}

// Requirement "血缘声明", Scenario "父模型不存在".
#[test]
fn an_unknown_parent_is_refused() {
    ext().execute_with(|| {
        let m = manifest(b"child", QuantType::Int4, &[1]);
        let lineage = Lineage {
            parent: ModelId([9; 32]),
            kind: LineageKind::Finetune,
        };
        assert_noop!(
            register(ALICE, &m, Some(lineage)),
            Error::<Test>::UnknownParent
        );
        let own = Lineage {
            parent: m.id().unwrap(),
            kind: LineageKind::Merge,
        };
        assert_noop!(register(ALICE, &m, Some(own)), Error::<Test>::SelfParent);
    });
}

// Scenario "声明量化血缘".
#[test]
fn a_quantized_child_records_its_parent() {
    ext().execute_with(|| {
        let parent = manifest(b"base", QuantType::Bf16, &[1, 2]);
        assert_ok!(register(ALICE, &parent, None));
        let child = manifest(b"base-int4", QuantType::Int4, &[3]);
        let lineage = Lineage {
            parent: parent.id().unwrap(),
            kind: LineageKind::Quantize,
        };
        assert_ok!(register(BOB, &child, Some(lineage)));
        assert_eq!(
            ModelRegistry::model(&child.id().unwrap()).unwrap().lineage,
            Some(lineage)
        );
    });
}

// Requirement "预留字段", Scenario "填写版税".
#[test]
fn royalties_are_not_enabled() {
    ext().execute_with(|| {
        let m = manifest(b"m", QuantType::Fp16, &[1]);
        assert_noop!(
            ModelRegistry::register(
                RuntimeOrigin::signed(ALICE),
                m,
                None,
                BoundedVec::default(),
                Some(RoyaltySpec {
                    beneficiary: ALICE,
                    bps: 500
                })
            ),
            Error::<Test>::RoyaltyNotEnabled
        );
    });
}

// Requirement "登记押金与不可修改", Scenario "押金冻结" (total conservation).
#[test]
fn the_deposit_is_held_and_issuance_is_unchanged() {
    ext().execute_with(|| {
        let issuance = Balances::total_issuance();
        let m = manifest(b"m", QuantType::Fp16, &[1, 2, 3]);
        assert_ok!(register(ALICE, &m, None));
        let record = Models::<Test>::get(m.id().unwrap()).unwrap();
        assert_eq!(record.deposit, 1_000 + 10 * record.encoded_size() as u128);
        assert_eq!(held(ALICE), record.deposit);
        assert_eq!(Balances::balance(&ALICE), FUNDS - record.deposit);
        assert_eq!(Balances::total_issuance(), issuance);
    });
}

// Scenario "余额不足".
#[test]
fn a_poor_registrant_is_refused() {
    ext().execute_with(|| {
        let m = manifest(b"m", QuantType::Fp16, &[1]);
        assert_noop!(register(POOR, &m, None), Error::<Test>::InsufficientBalance);
        assert_eq!(held(POOR), 0);
        assert_eq!(Balances::balance(&POOR), 100);
    });
}

#[test]
fn deposits_grow_with_the_shards() {
    ext().execute_with(|| {
        let small = manifest(b"s", QuantType::Fp16, &[1]);
        let large = manifest(b"l", QuantType::Fp16, &[1, 2, 3, 4]);
        assert_ok!(register(ALICE, &small, None));
        assert_ok!(register(ALICE, &large, None));
        let d = |m: &ModelManifest| ModelRegistry::model(&m.id().unwrap()).unwrap().deposit;
        assert_eq!(d(&large) - d(&small), 3 * 32 * 10);
    });
}
