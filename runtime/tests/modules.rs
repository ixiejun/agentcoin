//! Published pallet indices and the M2 runtime APIs (m2-finality 6.1).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

mod common;

use ac_runtime::{Offences, RandomnessCr, VERSION, ValidatorSet};
use common::dev_ext;
use frame_support::traits::PalletInfoAccess;

// Pallet indices are part of the storage and call encoding: never renumber them.
#[test]
fn pallet_indices_are_fixed() {
    assert_eq!(<ValidatorSet as PalletInfoAccess>::index(), 6);
    assert_eq!(<Offences as PalletInfoAccess>::index(), 7);
    assert_eq!(<RandomnessCr as PalletInfoAccess>::index(), 8);
    assert_eq!(<ac_runtime::PqAccounts as PalletInfoAccess>::index(), 5);
    // m3-economics (design D9).
    assert_eq!(<ac_runtime::Emission as PalletInfoAccess>::index(), 9);
    assert_eq!(<ac_runtime::TreasuryDual as PalletInfoAccess>::index(), 10);
    assert_eq!(<ac_runtime::PoaCouncil as PalletInfoAccess>::index(), 11);
    assert_eq!(<ac_runtime::PoaAdmin as PalletInfoAccess>::index(), 12);
    // m3-pos (design D11).
    assert_eq!(<ac_runtime::StakingPos as PalletInfoAccess>::index(), 13);
    assert_eq!(
        <ac_runtime::StakingPos as PalletInfoAccess>::name(),
        "StakingPos"
    );
    assert_eq!(
        <ac_runtime::ValidatorSet as PalletInfoAccess>::name(),
        "ValidatorSet"
    );
    // The published names the node reads storage keys under.
    assert_eq!(
        <ac_runtime::Emission as PalletInfoAccess>::name(),
        "Emission"
    );
    assert_eq!(
        <ac_runtime::PoaCouncil as PalletInfoAccess>::name(),
        "PoaCouncil"
    );
    // m3-pos: new calls (design D11).
    assert_eq!(VERSION.transaction_version, 3);
    assert_eq!(VERSION.spec_version, 3);
}

// The M2 runtime APIs are declared by the runtime.
#[test]
fn runtime_apis_are_declared() {
    use sp_api::RuntimeApiInfo;
    let has = |id: [u8; 8]| VERSION.apis.iter().any(|(api, _)| *api == id);
    assert!(has(
        <dyn ac_primitives::validator_set::ValidatorSetApi<ac_runtime::Block>>::ID
    ));
    assert!(has(
        <dyn ac_primitives::offences::OffencesApi<ac_runtime::Block>>::ID
    ));
    assert!(has(
        <dyn ac_primitives::randomness::RandomnessApi<ac_runtime::Block>>::ID
    ));
    assert!(has(
        <dyn ac_primitives::emission::EmissionApi<ac_runtime::Block>>::ID
    ));
    assert!(has(
        <dyn ac_primitives::emission::TreasuryApi<ac_runtime::Block, ac_runtime::AccountId>>::ID
    ));
    assert!(has(
        <dyn ac_primitives::staking::StakingApi<ac_runtime::Block, ac_runtime::AccountId>>::ID
    ));
}

// The functions behind the APIs answer on the dev chain.
#[test]
fn api_backing_functions_answer() {
    dev_ext().execute_with(|| {
        assert_eq!(ValidatorSet::authority_set().1.len(), 1);
        assert_eq!(ValidatorSet::epoch_length(), 10);
        assert_eq!(ValidatorSet::historical_set(0).map(|s| s.len()), Some(1));
        assert!(Offences::offences(0).is_empty());
        assert_eq!(RandomnessCr::latest(), None);
    });
}
