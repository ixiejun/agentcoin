//! Benchmarks.
#![allow(clippy::unwrap_used, clippy::expect_used)] // Benchmark setup.

use frame_benchmarking::v2::benchmarks;

use crate::{Config, Pallet};

#[benchmarks]
mod benchmarks {
    use super::{Config, Pallet};
    use crate::{Call, Params, Rate, RefRateParams};
    use ac_primitives::market::AtcPerUsd;
    use frame_benchmarking::v2::BenchmarkError;
    // The macro expands to code naming `impl_test_function` unqualified.
    use frame_benchmarking::impl_test_function;
    use frame_support::traits::EnsureOrigin;
    use frame_system::pallet_prelude::BlockNumberFor;

    /// Worst case: a rate is already set, so the band and the interval are checked.
    #[benchmark]
    fn set_rate() -> Result<(), BenchmarkError> {
        Params::<T>::put(RefRateParams { min_interval: 1 });
        Rate::<T>::put((AtcPerUsd(1_000_000), BlockNumberFor::<T>::default()));
        frame_system::Pallet::<T>::set_block_number(10u32.into());
        let origin =
            T::AdminOrigin::try_successful_origin().map_err(|_| BenchmarkError::Weightless)?;
        #[extrinsic_call]
        _(origin as T::RuntimeOrigin, AtcPerUsd(1_100_000));
        assert_eq!(Rate::<T>::get().map(|(r, _)| r), Some(AtcPerUsd(1_100_000)));
        Ok(())
    }

    impl_benchmark_test_suite!(Pallet, crate::mock::ext(None), crate::mock::Test);
}
