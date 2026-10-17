//! The runtime's storage migrations, chained as `Migrations` runs them, on the state of a local
//! chain from before them (m6-audit-hardening 6.3).
// Test code: AGENT.md §5.3 permits unwrap and panics in tests.
#![allow(clippy::unwrap_used, clippy::indexing_slicing)]

mod common;

use ac_primitives::market::MicroUsd;
use ac_primitives::market::audit::{
    AuditProfile, CURRENT_STATS, CURRENT_THRESHOLDS_VERSION, PenaltyParams, SprtEntry, SprtState,
    StatsConfig,
};
use ac_runtime::{Audit, Migrations, Runtime, Work};
use common::{issuance, preset_ext};
use frame_support::storage::unhashed;
use frame_support::traits::{GetStorageVersion, OnRuntimeUpgrade, StorageVersion};
use pallet_audit::migrations::old::{SprtStates, StatsSettings};
use pallet_audit::migrations::v2::OldAdjustable;
use parity_scale_codec::Encode;
use sp_runtime::AccountId32;

/// Rewinds the audit pallet of a fresh local chain to storage version 1: the global versions in
/// the adjustable parameters and the statistical configuration, a per-provider state, and none
/// of version 2's storage.
fn rewind_to_v1() {
    unhashed::put_raw(
        &pallet_audit::Adjustable::<Runtime>::hashed_key(),
        &OldAdjustable {
            stake_usd: MicroUsd(1_000_000_000),
            payment_usd: MicroUsd(50_000),
            thresholds_version: CURRENT_THRESHOLDS_VERSION,
        }
        .encode(),
    );
    StatsSettings::<Runtime>::put(StatsConfig {
        version: CURRENT_STATS.version,
        enabled: true,
    });
    let state = SprtState {
        cumulative: 3_000,
        entries: vec![SprtEntry {
            auditor: AccountId32::new([1; 32]),
            round: 0,
            contribution: 3_000,
        }]
        .try_into()
        .unwrap(),
    };
    SprtStates::<Runtime>::insert(AccountId32::new([2; 32]), (CURRENT_STATS.version, state));
    pallet_audit::DefaultProfile::<Runtime>::kill();
    pallet_audit::Penalty::<Runtime>::kill();
    pallet_audit::StatsEnabled::<Runtime>::kill();
    StorageVersion::new(1).put::<Audit>();
    // Work settlement before its recent revenue (version 0).
    pallet_work::RevenueWindow::<Runtime>::kill();
    StorageVersion::new(0).put::<Work>();
}

#[test]
fn migrations_bring_a_chain_from_before_them_up_to_date() {
    preset_ext(sp_genesis_builder::LOCAL_TESTNET_RUNTIME_PRESET).execute_with(|| {
        rewind_to_v1();
        let before = issuance();

        <Migrations as OnRuntimeUpgrade>::on_runtime_upgrade();

        assert_eq!(Audit::on_chain_storage_version(), 2);
        assert_eq!(Audit::in_code_storage_version(), 2);
        assert_eq!(Work::on_chain_storage_version(), 1);
        assert_eq!(Work::in_code_storage_version(), 1);
        assert_eq!(
            pallet_work::RevenueWindow::<Runtime>::get(),
            pallet_work::LIVE_REVENUE_WINDOW
        );
        assert!(pallet_work::RevenueWindow::<Runtime>::exists());
        let adjustable = pallet_audit::Adjustable::<Runtime>::get().unwrap();
        assert_eq!(
            (adjustable.stake_usd, adjustable.spot_check_bps),
            (MicroUsd(1_000_000_000), 500)
        );
        let profile = AuditProfile {
            thresholds_version: CURRENT_THRESHOLDS_VERSION,
            stats_version: CURRENT_STATS.version,
        };
        assert_eq!(
            pallet_audit::DefaultProfile::<Runtime>::get(),
            Some(profile)
        );
        assert!(pallet_audit::StatsEnabled::<Runtime>::get());
        assert_eq!(
            pallet_audit::Penalty::<Runtime>::get(),
            Some(PenaltyParams::LIVE)
        );
        assert!(!StatsSettings::<Runtime>::exists());
        assert_eq!(SprtStates::<Runtime>::iter().count(), 0);
        // Migrations move no balance.
        assert_eq!(issuance(), before);

        // Run again: both versioned migrations are no-ops now.
        <Migrations as OnRuntimeUpgrade>::on_runtime_upgrade();
        assert_eq!(pallet_audit::Adjustable::<Runtime>::get(), Some(adjustable));
        assert_eq!(Audit::on_chain_storage_version(), 2);
    });
}

#[test]
fn a_fresh_chain_is_already_up_to_date() {
    preset_ext(sp_genesis_builder::LOCAL_TESTNET_RUNTIME_PRESET).execute_with(|| {
        assert_eq!(Audit::on_chain_storage_version(), 2);
        assert_eq!(Work::on_chain_storage_version(), 1);
        let adjustable = pallet_audit::Adjustable::<Runtime>::get();
        <Migrations as OnRuntimeUpgrade>::on_runtime_upgrade();
        assert_eq!(pallet_audit::Adjustable::<Runtime>::get(), adjustable);
    });
}
