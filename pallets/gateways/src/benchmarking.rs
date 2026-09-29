//! Benchmarks: every call at its worst case.
#![allow(clippy::unwrap_used, clippy::expect_used)] // Benchmark setup.

use frame_benchmarking::v2::benchmarks;

use crate::{Config, Pallet};

#[benchmarks]
mod benchmarks {
    use super::{Config, Pallet};
    use crate::{BenchmarkHelper, Call, Gateways, Params};
    use ac_primitives::market::records::{MAX_ENDPOINT_LEN, MAX_UNLOCKING};
    use alloc::vec;
    use frame_benchmarking::v2::whitelisted_caller;
    // The macro expands to code naming `impl_test_function` unqualified.
    use frame_benchmarking::impl_test_function;
    use frame_support::BoundedVec;
    use frame_support::traits::fungible::Mutate;
    use frame_system::RawOrigin;
    use sp_runtime::{SaturatedConversion, Saturating};

    const STAKE: u128 = 1_000_000_000_000;

    fn endpoint() -> ac_primitives::market::records::Endpoint {
        BoundedVec::truncate_from(vec![b'e'; MAX_ENDPOINT_LEN as usize])
    }

    fn funded<T: Config>() -> T::AccountId {
        // One smallest unit per dollar: every stake meets the threshold.
        T::BenchmarkHelper::set_rate(1);
        let who: T::AccountId = whitelisted_caller();
        T::Currency::set_balance(&who, STAKE.saturating_mul(10));
        who
    }

    fn registered<T: Config>() -> T::AccountId {
        let who = funded::<T>();
        Pallet::<T>::register(
            RawOrigin::Signed(who.clone()).into(),
            endpoint(),
            100,
            STAKE,
        )
        .unwrap();
        who
    }

    #[benchmark]
    fn register() {
        let who = funded::<T>();
        #[extrinsic_call]
        _(RawOrigin::Signed(who.clone()), endpoint(), 100, STAKE);
        assert!(Gateways::<T>::contains_key(&who));
    }

    #[benchmark]
    fn update() {
        let who = registered::<T>();
        #[extrinsic_call]
        _(RawOrigin::Signed(who), Some(endpoint()), Some(200));
    }

    #[benchmark]
    fn bond_extra() {
        let who = registered::<T>();
        #[extrinsic_call]
        _(RawOrigin::Signed(who), STAKE);
    }

    /// A full unbonding list: the new chunk merges into the last one.
    #[benchmark]
    fn unbond() {
        let who = registered::<T>();
        for _ in 0..MAX_UNLOCKING {
            Pallet::<T>::unbond(RawOrigin::Signed(who.clone()).into(), 1).unwrap();
        }
        #[extrinsic_call]
        _(RawOrigin::Signed(who), 1);
    }

    #[benchmark]
    fn exit() {
        let who = registered::<T>();
        #[extrinsic_call]
        _(RawOrigin::Signed(who));
    }

    /// Every chunk is due and the gateway is removed.
    #[benchmark]
    fn withdraw_unbonded() {
        let who = registered::<T>();
        for _ in 0..MAX_UNLOCKING.saturating_sub(1) {
            Pallet::<T>::unbond(RawOrigin::Signed(who.clone()).into(), 1).unwrap();
        }
        Pallet::<T>::exit(RawOrigin::Signed(who.clone()).into()).unwrap();
        let later = frame_system::Pallet::<T>::block_number()
            .saturating_add(Params::<T>::get().unbond_blocks.saturated_into());
        frame_system::Pallet::<T>::set_block_number(later);
        #[extrinsic_call]
        _(RawOrigin::Signed(who.clone()));
        assert!(!Gateways::<T>::contains_key(&who));
    }

    impl_benchmark_test_suite!(Pallet, crate::mock::ext(), crate::mock::Test);
}
