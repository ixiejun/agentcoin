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
    // m4-evm (design D1): the Revive index is published in `ac_primitives::evm`.
    assert_eq!(
        <ac_runtime::Revive as PalletInfoAccess>::index(),
        usize::from(ac_primitives::evm::REVIVE_PALLET_INDEX)
    );
    assert_eq!(<ac_runtime::EvmSupport as PalletInfoAccess>::index(), 15);
    // m5-market-registry (design D9): the market pallets at 16-20, new calls.
    assert_eq!(<ac_runtime::RefRate as PalletInfoAccess>::index(), 16);
    assert_eq!(<ac_runtime::ModelRegistry as PalletInfoAccess>::index(), 17);
    assert_eq!(<ac_runtime::Providers as PalletInfoAccess>::index(), 18);
    assert_eq!(<ac_runtime::Gateways as PalletInfoAccess>::index(), 19);
    assert_eq!(<ac_runtime::Credits as PalletInfoAccess>::index(), 20);
    // m5-work-settlement (design D12): settlement at 21.
    assert_eq!(<ac_runtime::Work as PalletInfoAccess>::index(), 21);
    // m6-audit-chain: audits at 22, new calls.
    assert_eq!(<ac_runtime::Audit as PalletInfoAccess>::index(), 22);
    // m6-public-jobs (design D9): public jobs at 23, new calls.
    assert_eq!(<ac_runtime::PublicJobs as PalletInfoAccess>::index(), 23);
    assert_eq!(VERSION.transaction_version, 8);
    assert_eq!(VERSION.spec_version, 9);
}

// The M2 runtime APIs are declared by the runtime.
#[test]
fn runtime_apis_are_declared() {
    use sp_api::RuntimeApiInfo;
    let has = |id: [u8; 8]| VERSION.apis.iter().any(|(api, _)| *api == id);
    assert!(has(
        <dyn ac_primitives::validator_set::ValidatorSetApi<ac_runtime::Block>>::ID
    ));
    // m5-market-registry (design D9).
    assert!(has(<dyn ac_primitives::market::MarketApi<
            ac_runtime::Block,
            ac_runtime::AccountId,
            ac_runtime::Balance,
            u32,
        >>::ID));
    // m5-work-settlement (design D12).
    assert!(has(<dyn ac_primitives::market::WorkApi<
            ac_runtime::Block,
            ac_runtime::AccountId,
            ac_runtime::Balance,
        >>::ID));
    // m6-audit-chain.
    assert!(has(
        <dyn ac_primitives::market::AuditApi<ac_runtime::Block, ac_runtime::AccountId, u32>>::ID
    ));
    // m6-public-jobs (design D9).
    assert!(has(
        <dyn ac_primitives::market::public::PublicJobsApi<
                ac_runtime::Block,
                ac_runtime::AccountId,
                u32,
            >>::ID
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

// license-internal-features 3.1: `scripts/check-release-runtime.sh` recognises the benchmarking
// API by this ID (BLAKE2b-64 of the trait name), and a benchmark build does expose it.
#[cfg(feature = "runtime-benchmarks")]
#[test]
fn benchmarking_api_id_matches_the_release_check() {
    use sp_api::RuntimeApiInfo;
    let id = <dyn frame_benchmarking::Benchmark<ac_runtime::Block>>::ID;
    assert_eq!(id, [0x67, 0xf4, 0xb8, 0xfb, 0xa8, 0x58, 0x78, 0x2a]);
    assert!(VERSION.apis.iter().any(|(api, _)| *api == id));
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
