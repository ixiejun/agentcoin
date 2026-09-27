//! Benchmarks of epoch-boundary processing, the only work this pallet does in blocks.
//!
//! Keys are derived from fixed seeds: benchmarks must be reproducible.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)] // Benchmark setup.

use alloc::vec::Vec;

use ac_crypto::sig::{SecretSeed, SigningKey};
use ac_crypto::{PqPublicKey, SigAlg};
use ac_primitives::ac_bft::Authority;
use ac_primitives::validator_set::BlockAuthorities;
use frame_benchmarking::v2::benchmarks;
use frame_support::BoundedVec;
use frame_support::traits::Get;

use crate::{
    Authorities, Call, Config, CurrentSetId, EpochLength, HistoricalSet, OldestSet, Pallet,
    PendingRemovals, PoaAuthorities, SetHistory, TransitionParams, ValidatorCount,
};
use frame_support::traits::EnsureOrigin;

fn key(i: u32) -> PqPublicKey {
    let mut seed = [0u8; 32];
    seed[..4].copy_from_slice(&i.to_le_bytes());
    SigningKey::from_seed(SigAlg::MlDsa65, &SecretSeed::new(seed))
        .unwrap()
        .public_key()
        .unwrap()
}

/// Installs `n` authorities and a history whose oldest set is due for pruning at epoch
/// `HistoryEpochs + 2` (the worst case for [`Pallet::enact`]).
fn setup<T: Config>(n: u32) -> u64 {
    let keys: Vec<PqPublicKey> = (0..n).map(key).collect();
    T::BlockAuthorities::set_authorities(keys.clone()).unwrap();
    PoaAuthorities::<T>::put(BoundedVec::truncate_from(keys.clone()));
    let set: BoundedVec<Authority, T::MaxAuthorities> =
        BoundedVec::try_from(keys.into_iter().map(Authority::poa).collect::<Vec<_>>()).unwrap();
    Authorities::<T>::put(set.clone());
    SetHistory::<T>::insert(
        0,
        HistoricalSet {
            authorities: set.clone(),
            since: 0,
        },
    );
    SetHistory::<T>::insert(
        1,
        HistoricalSet {
            authorities: set,
            since: 1,
        },
    );
    OldestSet::<T>::put(0);
    CurrentSetId::<T>::put(1);
    u64::from(T::HistoryEpochs::get()) + 2
}

#[benchmarks]
mod benchmarks {
    use super::{
        Call, Config, EnsureOrigin, EpochLength, Get, Pallet, PendingRemovals, PoaAuthorities,
        TransitionParams, ValidatorCount, Vec, key, setup,
    };
    // The macro expands to code naming `impl_test_function` unqualified.
    use frame_benchmarking::impl_test_function;
    use frame_benchmarking::v2::BenchmarkError;

    /// Boundary that removes one validator: new aura list, digest, history, events.
    #[benchmark]
    fn epoch_boundary_with_change(
        n: Linear<2, { T::MaxAuthorities::get() }>,
    ) -> Result<(), BenchmarkError> {
        let epoch = setup::<T>(n);
        PendingRemovals::<T>::put(
            frame_support::BoundedVec::try_from(Vec::from([key(n.saturating_sub(1))])).unwrap(),
        );
        let changed;
        #[block]
        {
            changed = Pallet::<T>::enact(epoch, 1).0;
        }
        assert!(changed);
        Ok(())
    }

    /// Boundary without offences: only history pruning.
    #[benchmark]
    fn epoch_boundary_without_change(
        n: Linear<2, { T::MaxAuthorities::get() }>,
    ) -> Result<(), BenchmarkError> {
        let epoch = setup::<T>(n);
        let changed;
        #[block]
        {
            changed = Pallet::<T>::enact(epoch, 1).0;
        }
        assert!(!changed);
        Ok(())
    }

    /// Adds a key to a roster one short of the maximum.
    #[benchmark]
    fn add_poa_authority() -> Result<(), BenchmarkError> {
        let max = T::MaxAuthorities::get();
        let roster: Vec<_> = (0..max.saturating_sub(1)).map(key).collect();
        PoaAuthorities::<T>::put(frame_support::BoundedVec::truncate_from(roster));
        EpochLength::<T>::put(u64::from(max).saturating_mul(2));
        let origin =
            T::AdminOrigin::try_successful_origin().map_err(|_| BenchmarkError::Weightless)?;
        let new = key(max);

        #[extrinsic_call]
        _(origin as T::RuntimeOrigin, new.clone());

        assert!(PoaAuthorities::<T>::get().contains(&new));
        Ok(())
    }

    /// Removes the last key of a full roster.
    #[benchmark]
    fn remove_poa_authority() -> Result<(), BenchmarkError> {
        let max = T::MaxAuthorities::get();
        let roster: Vec<_> = (0..max).map(key).collect();
        PoaAuthorities::<T>::put(frame_support::BoundedVec::truncate_from(roster));
        let origin =
            T::AdminOrigin::try_successful_origin().map_err(|_| BenchmarkError::Weightless)?;
        let gone = key(max.saturating_sub(1));

        #[extrinsic_call]
        _(origin as T::RuntimeOrigin, gone.clone());

        assert!(!PoaAuthorities::<T>::get().contains(&gone));
        Ok(())
    }

    #[benchmark]
    fn set_validator_count() -> Result<(), BenchmarkError> {
        EpochLength::<T>::put(u64::from(T::MaxAuthorities::get()).saturating_mul(2));
        TransitionParams::<T>::mutate(|p| p.min_candidates = 1);
        let origin =
            T::AdminOrigin::try_successful_origin().map_err(|_| BenchmarkError::Weightless)?;
        let count = T::MaxAuthorities::get();

        #[extrinsic_call]
        _(origin as T::RuntimeOrigin, count);

        assert_eq!(ValidatorCount::<T>::get(), count);
        Ok(())
    }

    impl_benchmark_test_suite!(Pallet, crate::mock::new_bench_ext(), crate::mock::Test);
}
