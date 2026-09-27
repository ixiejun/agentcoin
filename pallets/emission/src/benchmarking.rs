//! Benchmarks: settling an emission epoch.
#![allow(clippy::unwrap_used, clippy::expect_used)] // Benchmark setup.

use frame_benchmarking::v2::benchmarks;

use crate::{Config, EpochLength, LastSettled, Pallet};

#[benchmarks]
mod benchmarks {
    use super::{Config, EpochLength, LastSettled, Pallet};
    use frame_benchmarking::v2::BenchmarkError;
    // The macro expands to code naming `impl_test_function` unqualified.
    use frame_benchmarking::impl_test_function;

    /// Settling an epoch: reading the work and the security budget, minting to every
    /// recipient (all accounts new, the costliest case) and recording the reserve.
    #[benchmark]
    fn settle_epoch() -> Result<(), BenchmarkError> {
        if EpochLength::<T>::get().is_none() {
            EpochLength::<T>::put(10);
        }
        let schedule =
            Pallet::<T>::schedule().ok_or(BenchmarkError::Stop("no emission schedule"))?;
        #[block]
        {
            Pallet::<T>::settle_epoch(&schedule, 0);
        }
        assert_eq!(LastSettled::<T>::get(), Some(0));
        Ok(())
    }

    impl_benchmark_test_suite!(Pallet, crate::mock::ext(10), crate::mock::Test);
}
