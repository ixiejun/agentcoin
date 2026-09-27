//! Runtime-level economics: fee distribution (spec economics/fee-distribution,
//! chain/native-token) and emission settlement through the executive (economics/emission,
//! economics/treasury). Tasks 4.2 and 4.3 of m3-economics.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

mod common;

use ac_crypto::SigAlg;
use ac_primitives::emission::{EmissionSchedule, EpochInput, Phase, settle};
use ac_runtime::genesis_config_presets::dev_public_key;
use ac_runtime::transaction::TxParams;
use ac_runtime::{ATC, AccountId, EXISTENTIAL_DEPOSIT, Emission, Runtime, System, TreasuryDual};
use common::{
    GENESIS, Signer, apply, build, context, fees_paid, free, issuance, next_authored_block,
    preset_ext, signed, start_authored_block, sum_of_balances, transfer,
};
use sp_runtime::generic::Era;

/// Account of the dev chain's only authority, alice, derived from her block-sealing key.
fn author() -> AccountId {
    pallet_pq_accounts::derived_account(&dev_public_key("alice", SigAlg::MlDsa65).unwrap())
}

/// Dev chain positioned inside block 1, authored by alice.
fn dev_authored() -> sp_io::TestExternalities {
    let mut ext = preset_ext(sp_genesis_builder::DEV_RUNTIME_PRESET);
    ext.execute_with(|| start_authored_block(1, GENESIS));
    ext
}

fn exists(who: &AccountId) -> bool {
    frame_system::Account::<Runtime>::contains_key(who)
}

// Scenarios "分配比例", "手续费被销毁" and "销毁计入累计值": the author receives 20% of the
// fee (rounded down), the rest is burned and counted.
#[test]
fn fees_go_20_percent_to_the_author() {
    dev_authored().execute_with(|| {
        let alice = Signer::dev("alice");
        let bob = Signer::dev("bob");
        // Give the author's account an existential deposit so it can take small shares.
        assert_eq!(apply(signed(&bob, transfer(&author(), ATC))), Ok(Ok(())));
        for _ in 0..3 {
            let (author0, issuance0, burned0, paid0) = (
                free(&author()),
                issuance(),
                Emission::total_burned(),
                fees_paid(),
            );
            assert_eq!(
                apply(signed(&alice, transfer(&bob.account, ATC))),
                Ok(Ok(()))
            );
            let fee = fees_paid() - paid0;
            assert!(fee > 0);
            let share = fee / 5;
            assert_eq!(free(&author()) - author0, share);
            assert_eq!(issuance0 - issuance(), fee - share);
            assert_eq!(Emission::total_burned() - burned0, fee - share);
        }
        assert_eq!(sum_of_balances(), issuance());
    });
}

// Tips are distributed like fees.
#[test]
fn tips_are_split_like_fees() {
    dev_authored().execute_with(|| {
        let alice = Signer::dev("alice");
        let bob = Signer::dev("bob");
        let nonce = System::account_nonce(&alice.account);
        let params = TxParams {
            nonce,
            tip: ATC,
            era: Era::Immortal,
            era_birth_hash: System::block_hash(0),
        };
        let issuance0 = issuance();
        let xt = build(
            &alice,
            &alice.key,
            transfer(&bob.account, ATC),
            true,
            params,
            context(),
        );
        assert_eq!(apply(xt), Ok(Ok(())));
        let paid = fees_paid();
        assert!(paid > ATC);
        // The author's account is new and its share is far above the existential deposit.
        assert_eq!(free(&author()), paid / 5);
        assert_eq!(issuance0 - issuance(), paid - paid / 5);
    });
}

// Scenario "份额低于存在性押金": a new author account is not created for a dust share; the
// share is burned instead.
#[test]
fn author_share_below_existential_deposit_is_burned() {
    dev_authored().execute_with(|| {
        let alice = Signer::dev("alice");
        let bob = Signer::dev("bob");
        let (issuance0, burned0) = (issuance(), Emission::total_burned());
        assert_eq!(
            apply(signed(&alice, transfer(&bob.account, ATC))),
            Ok(Ok(()))
        );
        let fee = fees_paid();
        assert!(fee / 5 < EXISTENTIAL_DEPOSIT, "fee {fee}");
        assert!(!exists(&author()));
        assert_eq!(issuance0 - issuance(), fee);
        assert_eq!(Emission::total_burned() - burned0, fee);
    });
}

// Emission settles through the executive: nothing is minted inside an epoch, and the first
// block of the next epoch mints the floor top-up to the treasury floor (PoA, no work), exactly
// as the pure settlement function says; the treasury accounts and runtime views agree.
#[test]
fn settlement_through_the_executive() {
    dev_authored().execute_with(|| {
        let length = pallet_emission::EpochLength::<Runtime>::get().unwrap();
        assert!(length <= 20);
        let schedule = EmissionSchedule::new(length).unwrap();
        let genesis = issuance();
        let blocks = u32::try_from(length).unwrap();
        while System::block_number() < blocks {
            next_authored_block();
        }
        assert_eq!(pallet_emission::TotalMinted::<Runtime>::get(), 0);
        next_authored_block(); // block L + 1 settles epoch 0
        let out = settle(&EpochInput::new(
            schedule.scheduled(0),
            0,
            (0, 0),
            Phase::Poa,
        ));
        let floor = TreasuryDual::floor_account();
        assert_eq!(free(&floor), out.treasury_floor_topup);
        assert!(out.treasury_floor_topup > 0);
        assert_eq!(free(&TreasuryDual::community_account()), 0);
        assert_eq!(free(&TreasuryDual::holder_account()), 0);
        assert_eq!(
            pallet_emission::Reserve::<Runtime>::get(),
            out.reserve + out.security
        );
        assert_eq!(
            issuance(),
            genesis + out.treasury_floor_topup - Emission::total_burned()
        );
        assert_eq!(sum_of_balances(), issuance());
        // Fresh floor top-ups are not spendable yet.
        assert_eq!(TreasuryDual::floor_spendable(), 0);
    });
}

// Dust of a reaped account is burned and counted, so Δissuance = minted − burned stays exact.
#[test]
fn dust_is_counted_as_burned() {
    use sp_runtime::traits::Dispatchable;
    dev_authored().execute_with(|| {
        let bob = Signer::dev("bob");
        let fresh = Signer::fresh(9, SigAlg::MlDsa44);
        assert_eq!(
            apply(signed(&bob, transfer(&fresh.account, ATC))),
            Ok(Ok(()))
        );
        let (issuance0, burned0) = (issuance(), Emission::total_burned());
        // Dispatched directly (no fee): leaves half an existential deposit, which is dust.
        let dust = EXISTENTIAL_DEPOSIT / 2;
        transfer(&bob.account, ATC - dust)
            .dispatch(ac_runtime::RuntimeOrigin::signed(fresh.account.clone()))
            .unwrap();
        assert!(!exists(&fresh.account));
        assert_eq!(issuance0 - issuance(), dust);
        assert_eq!(Emission::total_burned() - burned0, dust);
        assert_eq!(sum_of_balances(), issuance());
    });
}
