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
    assert_eq!(VERSION.transaction_version, 2);
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
