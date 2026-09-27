//! Benchmarks: spending from community grants and from the vested floor.
#![allow(clippy::unwrap_used, clippy::expect_used)] // Benchmark setup.

use frame_benchmarking::v2::benchmarks;

use crate::{Config, FloorPurpose, Pallet};

#[benchmarks]
mod benchmarks {
    use super::{Config, FloorPurpose, Pallet};
    use crate::{Call, FloorBatch, FloorBatches};
    use frame_benchmarking::v2::{BenchmarkError, account};
    // The macro expands to code naming `impl_test_function` unqualified.
    use frame_benchmarking::impl_test_function;
    use frame_support::traits::fungible::{Inspect, Mutate};
    use frame_support::traits::{EnsureOrigin, Get};
    use sp_runtime::SaturatedConversion;

    const AMOUNT: u128 = 1_000_000_000_000_000_000;

    /// Spending from community grants to a new account.
    #[benchmark]
    fn spend() -> Result<(), BenchmarkError> {
        let origin =
            T::AdminOrigin::try_successful_origin().map_err(|_| BenchmarkError::Weightless)?;
        let to: T::AccountId = account("to", 0, 0);
        T::Currency::mint_into(&Pallet::<T>::community_account(), AMOUNT * 2)
            .map_err(|_| BenchmarkError::Stop("cannot fund community grants"))?;
        #[extrinsic_call]
        _(origin as T::RuntimeOrigin, to.clone(), AMOUNT);
        assert_eq!(T::Currency::balance(&to), AMOUNT);
        Ok(())
    }

    /// Spending from the floor with the maximum number of vesting batches.
    #[benchmark]
    fn spend_floor() -> Result<(), BenchmarkError> {
        let origin =
            T::AdminOrigin::try_successful_origin().map_err(|_| BenchmarkError::Weightless)?;
        let to: T::AccountId = account("to", 0, 0);
        let batches = T::MaxBatches::get();
        let mut list = FloorBatches::<T>::get();
        for i in 0..batches {
            let end = u64::from(i).saturating_mul(T::BatchBlocks::get());
            list.try_push(FloorBatch {
                end,
                amount: AMOUNT,
            })
            .map_err(|_| BenchmarkError::Stop("too many batches"))?;
        }
        FloorBatches::<T>::put(list);
        T::Currency::mint_into(
            &Pallet::<T>::floor_account(),
            AMOUNT.saturating_mul(u128::from(batches)),
        )
        .map_err(|_| BenchmarkError::Stop("cannot fund the floor"))?;
        let now = u64::from(batches)
            .saturating_mul(T::BatchBlocks::get())
            .saturating_add(T::VestingBlocks::get());
        frame_system::Pallet::<T>::set_block_number(now.saturated_into());
        #[extrinsic_call]
        _(
            origin as T::RuntimeOrigin,
            FloorPurpose::Audit,
            to.clone(),
            AMOUNT,
        );
        assert_eq!(T::Currency::balance(&to), AMOUNT);
        Ok(())
    }

    impl_benchmark_test_suite!(Pallet, crate::mock::ext(), crate::mock::Test);
}
