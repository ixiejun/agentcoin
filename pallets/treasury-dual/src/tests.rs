//! Unit tests (spec economics/treasury).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use ac_primitives::emission::TreasuryDeposit;
use frame_support::traits::fungible::{Inspect, Mutate};
use frame_support::{assert_noop, assert_ok};
use sp_runtime::{AccountId32, BuildStorage, DispatchError};

use crate::mock::{
    BATCH, Balances, MaxBatches, RuntimeEvent, RuntimeGenesisConfig, RuntimeOrigin, System, Test,
    Treasury, VESTING, ext,
};
use crate::{
    CommunityShare, Error, Event, FloorBatch, FloorBatches, FloorPurpose, FloorSpent, SpendSource,
};

type Pallet = crate::Pallet<Test>;

const USER: AccountId32 = AccountId32::new([1; 32]);

/// Mints `amount` to the floor as emission would, at block `at`.
fn deposit_floor(at: u64, amount: u128) {
    System::set_block_number(at);
    let [_, _, (floor, topup)] = Pallet::recipients(0, amount);
    assert_eq!(topup, amount);
    Balances::mint_into(&floor, amount).unwrap();
    Pallet::floor_minted(amount);
}

fn spent_events() -> Vec<Event<Test>> {
    System::events()
        .into_iter()
        .filter_map(|e| match e.event {
            RuntimeEvent::Treasury(ev @ Event::Spent { .. }) => Some(ev),
            _ => None,
        })
        .collect()
}

// Scenario "按比例入账": 40% (rounded down) to community grants, the rest to the holder
// treasury; the accounts are fixed and distinct.
#[test]
fn proportional_split() {
    ext().execute_with(|| {
        assert_eq!(CommunityShare::<Test>::get(), 4_000);
        let [(c, a), (h, b), (f, z)] = Pallet::recipients(1_000, 0);
        assert_eq!((a, b, z), (400, 600, 0));
        assert_eq!(c, Pallet::community_account());
        assert_eq!(h, Pallet::holder_account());
        assert_eq!(f, Pallet::floor_account());
        assert!(c != h && h != f && c != f);
        let [(_, a), (_, b), _] = Pallet::recipients(7, 0);
        assert_eq!((a, b), (2, 5));
    });
}

// The accounts are derived from the published `PalletId`s without hashing.
#[test]
fn accounts_are_publicly_derivable() {
    ext().execute_with(|| {
        let mut expected = [0u8; 32];
        expected[..12].copy_from_slice(b"modlac/trhld");
        assert_eq!(Pallet::holder_account(), AccountId32::new(expected));
        expected[..12].copy_from_slice(b"modlac/trcom");
        assert_eq!(Pallet::community_account(), AccountId32::new(expected));
        expected[..12].copy_from_slice(b"modlac/trflr");
        assert_eq!(Pallet::floor_account(), AccountId32::new(expected));
    });
}

// Scenario "刚入账不可支出".
#[test]
fn fresh_floor_cannot_be_spent() {
    ext().execute_with(|| {
        deposit_floor(5, 1_000);
        assert_eq!(
            FloorBatches::<Test>::get().to_vec(),
            vec![FloorBatch {
                end: BATCH,
                amount: 1_000
            }]
        );
        assert_eq!(Pallet::floor_spendable(), 0);
        assert_noop!(
            Treasury::spend_floor(RuntimeOrigin::root(), FloorPurpose::Audit, USER, 1_000),
            Error::<Test>::NotVested
        );
    });
}

// Scenarios "解锁进度线性" and "两年后全部解锁".
#[test]
fn floor_vests_linearly_from_the_batch_end() {
    ext().execute_with(|| {
        deposit_floor(3, 600);
        deposit_floor(9, 401);
        System::set_block_number(BATCH);
        assert_eq!(Pallet::floor_spendable(), 0);
        System::set_block_number(BATCH + VESTING / 2);
        assert_eq!(Pallet::floor_spendable(), 1_001 / 2);
        System::set_block_number(BATCH + VESTING - 1);
        assert!(Pallet::floor_spendable() < 1_001);
        System::set_block_number(BATCH + VESTING);
        assert_eq!(Pallet::floor_spendable(), 1_001);
    });
}

// Scenario "保底支出注明用途", and spending counts against the vested amount.
#[test]
fn floor_spend_records_its_purpose() {
    ext().execute_with(|| {
        deposit_floor(1, 1_000);
        System::set_block_number(BATCH + VESTING / 2);
        assert_ok!(Treasury::spend_floor(
            RuntimeOrigin::root(),
            FloorPurpose::Audit,
            USER,
            300
        ));
        assert_eq!(FloorSpent::<Test>::get(), 300);
        assert_eq!(Pallet::floor_spendable(), 200);
        assert_eq!(
            spent_events(),
            vec![Event::Spent {
                source: SpendSource::Floor,
                to: USER,
                amount: 300,
                purpose: Some(FloorPurpose::Audit),
            }]
        );
        assert_noop!(
            Treasury::spend_floor(RuntimeOrigin::root(), FloorPurpose::ColdStart, USER, 201),
            Error::<Test>::NotVested
        );
        assert_ok!(Treasury::spend_floor(
            RuntimeOrigin::root(),
            FloorPurpose::ColdStart,
            USER,
            200
        ));
        assert_eq!(Pallet::floor_spendable(), 0);
    });
}

// Scenarios "普通账户无法支出" and "支出前后发行量不变".
#[test]
fn only_the_administration_spends_and_spending_is_a_transfer() {
    ext().execute_with(|| {
        let community = Pallet::community_account();
        Balances::mint_into(&community, 1_000).unwrap();
        assert_noop!(
            Treasury::spend(RuntimeOrigin::signed(USER.clone()), USER, 100),
            DispatchError::BadOrigin
        );
        assert_noop!(
            Treasury::spend_floor(
                RuntimeOrigin::signed(USER.clone()),
                FloorPurpose::Audit,
                USER,
                0
            ),
            DispatchError::BadOrigin
        );
        let issuance = Balances::total_issuance();
        assert_ok!(Treasury::spend(RuntimeOrigin::root(), USER, 400));
        assert_eq!(Balances::total_issuance(), issuance);
        assert_eq!(Balances::balance(&community), 600);
        assert_eq!(Balances::balance(&USER), 400);
        assert_eq!(
            spent_events(),
            vec![Event::Spent {
                source: SpendSource::Community,
                to: USER,
                amount: 400,
                purpose: None,
            }]
        );
        // More than the balance fails and changes nothing.
        assert!(Treasury::spend(RuntimeOrigin::root(), USER, 601).is_err());
        assert_eq!(Balances::balance(&community), 600);
    });
}

// Scenario "持币人国库持续入账": the holder treasury only ever grows; no call of this pallet
// takes funds from it.
#[test]
fn holder_treasury_only_grows() {
    ext().execute_with(|| {
        let holder = Pallet::holder_account();
        let mut last = 0;
        for epoch in 1..50u128 {
            for (who, amount) in Pallet::recipients(epoch * 997, 0) {
                if amount > 0 {
                    let _ = Balances::mint_into(&who, amount);
                }
            }
            let _ = Treasury::spend(RuntimeOrigin::root(), USER, epoch * 100);
            let now = Balances::balance(&holder);
            assert!(now >= last);
            last = now;
        }
        assert!(last > 0);
    });
}

// Batches stay bounded over a long time and fully vested ones are folded into the matured
// total without changing the vested amount.
#[test]
fn batches_stay_bounded() {
    ext().execute_with(|| {
        let mut total = 0u128;
        for at in (1..2_000u64).step_by(7) {
            deposit_floor(at, 100);
            total += 100;
            assert!(FloorBatches::<Test>::get().len() <= MaxBatches::get() as usize);
        }
        System::set_block_number(2_000 + BATCH + VESTING);
        assert_eq!(Pallet::floor_spendable(), total);
    });
}

// A community share above 100% fails genesis.
#[test]
#[should_panic(expected = "invalid treasury genesis")]
fn invalid_share_fails_genesis() {
    let _ = RuntimeGenesisConfig {
        treasury: crate::GenesisConfig {
            community_share: 10_001,
            ..Default::default()
        },
        ..Default::default()
    }
    .build_storage();
}
