//! Unit tests (spec economics/emission, economics/fee-distribution "累计销毁量").
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use ac_primitives::emission::{EmissionSchedule, EpochInput, Phase, settle, split_treasury};
use frame_support::traits::fungible::{Balanced, Inspect};
use frame_support::traits::{Hooks, OnUnbalanced};

use crate::mock::{
    Balances, COMMUNITY, Emission, FLOOR, HOLDER, PHASE, RuntimeEvent, System, Test, VALIDATOR,
    WORK, ext,
};
use crate::{EpochLength, Event, LastSettled, Reserve, TotalBurned, TotalMinted};

const L: u64 = 10;

fn run_to(n: u64) {
    while System::block_number() < n {
        let next = System::block_number() + 1;
        System::set_block_number(next);
        Emission::on_initialize(next);
    }
}

fn schedule() -> EmissionSchedule {
    EmissionSchedule::new(L).unwrap()
}

fn settled_events() -> Vec<Event<Test>> {
    System::events()
        .into_iter()
        .filter_map(|e| match e.event {
            RuntimeEvent::Emission(ev @ Event::EpochSettled { .. }) => Some(ev),
            _ => None,
        })
        .collect()
}

// Scenarios "没有工作模块" and "PoA 阶段不发放", and only settlement blocks mint.
#[test]
fn poa_epoch_without_work_mints_only_the_floor() {
    ext(L).execute_with(|| {
        run_to(L);
        assert_eq!(Balances::total_issuance(), 0);
        assert_eq!(LastSettled::<Test>::get(), None);
        run_to(L + 1);
        let s = schedule().scheduled(0);
        let floor = s * 5 / 100;
        assert_eq!(Balances::total_issuance(), floor);
        assert_eq!(Balances::balance(&FLOOR), floor);
        assert_eq!(Balances::balance(&VALIDATOR), 0);
        assert_eq!(
            Balances::balance(&COMMUNITY) + Balances::balance(&HOLDER),
            0
        );
        assert_eq!(Reserve::<Test>::get(), s - floor);
        assert_eq!(TotalMinted::<Test>::get(), floor);
        assert_eq!(LastSettled::<Test>::get(), Some(0));
        // Blocks up to the next settlement mint nothing.
        run_to(2 * L);
        assert_eq!(Balances::total_issuance(), floor);
        assert_eq!(settled_events().len(), 1);
    });
}

// The pallet mints exactly what the pure settlement function allots, epoch after epoch, and
// the reserve follows it (spec "按纪元结算的四路分配").
#[test]
fn matches_the_pure_function_over_many_epochs() {
    ext(L).execute_with(|| {
        PHASE.with(|p| *p.borrow_mut() = Phase::Pos);
        let s = schedule();
        let mut reserve = 0u128;
        let mut expected_issuance = 0u128;
        for epoch in 0..12u64 {
            let work = (
                s.scheduled(epoch) * (epoch as u128 % 3),
                s.scheduled(epoch) / 3,
            );
            WORK.with(|w| *w.borrow_mut() = work);
            run_to((epoch + 1) * L + 1);
            let out = settle(&EpochInput::new(
                s.scheduled(epoch),
                reserve,
                work,
                Phase::Pos,
            ));
            // Market and public shares are not minted before their modules exist.
            let minted = out.security + out.treasury();
            reserve = out.reserve + out.market + out.public;
            expected_issuance += minted;
            assert_eq!(Reserve::<Test>::get(), reserve, "epoch {epoch}");
            assert_eq!(
                Balances::total_issuance(),
                expected_issuance,
                "epoch {epoch}"
            );
            assert_eq!(
                TotalMinted::<Test>::get(),
                expected_issuance,
                "epoch {epoch}"
            );
        }
        let (community, holder) = (Balances::balance(&COMMUNITY), Balances::balance(&HOLDER));
        assert!(community > 0 && holder > community);
        assert!(Balances::balance(&VALIDATOR) > 0);
    });
}

// The proportional share is split 40% / 60%; a refused mint (below the existential deposit of a
// new account) returns to the reserve instead of vanishing.
#[test]
fn refused_mints_return_to_the_reserve() {
    ext(L).execute_with(|| {
        // 35 units of market work: proportional share 10, community 4 < existential deposit 10.
        WORK.with(|w| *w.borrow_mut() = (35, 0));
        run_to(L + 1);
        let s = schedule().scheduled(0);
        let (community, holder) = split_treasury(10, 4_000);
        assert_eq!((community, holder), (4, 6));
        assert_eq!(Balances::balance(&COMMUNITY), 0);
        let minted = TotalMinted::<Test>::get();
        assert_eq!(Balances::total_issuance(), minted);
        // reserve + minted = S exactly.
        assert_eq!(Reserve::<Test>::get() + minted, s);
    });
}

// Scenario "销毁计入累计值": burning through the pallet reduces the issuance and adds to the
// burned total, which exists from genesis.
#[test]
fn burns_are_recorded() {
    ext(L).execute_with(|| {
        assert_eq!(TotalBurned::<Test>::get(), Some(0));
        let credit = <Balances as Balanced<u64>>::issue(1_000);
        assert_eq!(Balances::total_issuance(), 1_000);
        Emission::on_unbalanced(credit);
        assert_eq!(Balances::total_issuance(), 0);
        assert_eq!(Emission::total_burned(), 1_000);
    });
}

// Genesis: the epoch length is stored; 0 leaves emission unset but the burned total exists.
#[test]
fn genesis_parameters() {
    ext(L).execute_with(|| {
        assert_eq!(EpochLength::<Test>::get(), Some(L));
        assert_eq!(Emission::current_epoch(), 0);
        run_to(L);
        assert_eq!(Emission::current_epoch(), 1);
    });
    ext(0).execute_with(|| {
        assert_eq!(EpochLength::<Test>::get(), None);
        assert_eq!(TotalBurned::<Test>::get(), Some(0));
        run_to(3 * L);
        assert_eq!(Balances::total_issuance(), 0);
    });
}

// Scenario "非法纪元长度": building the genesis fails.
#[test]
#[should_panic(expected = "invalid emission genesis")]
fn invalid_epoch_length_fails_genesis() {
    let _ = ext(7);
}
