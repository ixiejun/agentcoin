//! Benchmarks: the administration's own work, without the inner call or the council update.
#![allow(clippy::unwrap_used, clippy::expect_used)] // Benchmark setup.

use frame_benchmarking::v2::benchmarks;

use crate::{Config, Pallet};

#[benchmarks]
mod benchmarks {
    use super::{Config, Pallet};
    use crate::{Call, CouncilInstance, Threshold};
    use alloc::boxed::Box;
    use alloc::vec::Vec;
    use frame_benchmarking::v2::{BenchmarkError, account};
    // The macro expands to code naming `impl_test_function` unqualified.
    use frame_benchmarking::impl_test_function;
    use frame_support::traits::{EnsureOrigin, Get};

    fn members<T: Config>(n: u32) -> Vec<T::AccountId> {
        (0..n).map(|i| account("member", i, 0)).collect()
    }

    fn admin<T: Config>() -> Result<<T as frame_system::Config>::RuntimeOrigin, BenchmarkError> {
        T::AdminOrigin::try_successful_origin().map_err(|_| BenchmarkError::Weightless)
    }

    /// Checking the origin and the filter and dispatching a no-op call as Root.
    #[benchmark]
    fn dispatch_as_root() -> Result<(), BenchmarkError> {
        pallet_collective::Members::<T, CouncilInstance>::put(members::<T>(1));
        Threshold::<T>::put(1);
        let origin = admin::<T>()?;
        let call: <T as Config>::RuntimeCall =
            frame_system::Call::<T>::remark { remark: Vec::new() }.into();
        #[extrinsic_call]
        _(
            origin as <T as frame_system::Config>::RuntimeOrigin,
            Box::new(call),
        );
        Ok(())
    }

    /// Changing the threshold.
    #[benchmark]
    fn set_threshold() -> Result<(), BenchmarkError> {
        let max = <T as pallet_collective::Config<CouncilInstance>>::MaxMembers::get();
        pallet_collective::Members::<T, CouncilInstance>::put(members::<T>(max));
        Threshold::<T>::put(1);
        let origin = admin::<T>()?;
        #[extrinsic_call]
        _(origin as <T as frame_system::Config>::RuntimeOrigin, max);
        assert_eq!(Threshold::<T>::get(), max);
        Ok(())
    }

    /// Validating and sorting `m` new members (the council's own update is weighed by its
    /// benchmarked `set_members`).
    #[benchmark]
    fn set_members(
        m: Linear<1, { <T as pallet_collective::Config<CouncilInstance>>::MaxMembers::get() }>,
    ) -> Result<(), BenchmarkError> {
        pallet_collective::Members::<T, CouncilInstance>::put(members::<T>(1));
        Threshold::<T>::put(1);
        let origin = admin::<T>()?;
        let new: Vec<T::AccountId> = (0..m).map(|i| account("new", i, 1)).collect();
        #[extrinsic_call]
        _(origin as <T as frame_system::Config>::RuntimeOrigin, new, m);
        assert_eq!(
            pallet_collective::Members::<T, CouncilInstance>::get().len(),
            m as usize
        );
        Ok(())
    }

    impl_benchmark_test_suite!(Pallet, crate::mock::ext(), crate::mock::Test);
}
