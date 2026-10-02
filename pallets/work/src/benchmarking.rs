//! Benchmarks of both calls at their bounds (m5-work-settlement 3.5).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::arithmetic_side_effects
)] // Setup.

use frame_benchmarking::v2::benchmarks;

use crate::{Config, Pallet};

#[benchmarks]
mod benchmarks {
    use super::{Config, Pallet};
    use crate::{BenchmarkHelper, Call, Params};
    use ac_primitives::emission::{EpochIndex, MarketPayout, WorkSource};
    use ac_primitives::market::work::{JobKind, MAX_REPORT_ENTRIES, MAX_REPORT_VOUCHERS};
    use ac_primitives::market::{MicroUsd, ModelId, ReportEntry};
    use alloc::vec::Vec;
    use frame_benchmarking::v2::account;
    // The macro expands to code naming `impl_test_function` unqualified.
    use frame_benchmarking::impl_test_function;
    use frame_support::BoundedVec;
    use frame_support::traits::fungible::{Inspect, Mutate};
    use frame_system::RawOrigin;
    use sp_runtime::AccountId32;

    const MODEL: ModelId = ModelId([0x42; 32]);

    fn gateway<T: Config>() -> AccountId32 {
        let g: AccountId32 = account("gateway", 0, 0);
        T::BenchmarkHelper::prepare_gateway(&g);
        g
    }

    /// `e` entries of distinct providers summing to `total` micro-dollars.
    fn entries<T: Config>(e: u32, total: u128) -> Vec<ReportEntry<AccountId32>> {
        let each = total / u128::from(e);
        (0..e)
            .map(|i| {
                let provider: AccountId32 = account("provider", i, 0);
                T::BenchmarkHelper::prepare_provider(&provider, &MODEL);
                let usd = if i == 0 {
                    total - each * u128::from(e - 1)
                } else {
                    each
                };
                ReportEntry {
                    kind: JobKind::Inference,
                    provider,
                    model: MODEL,
                    usd: MicroUsd(usd),
                    unproven: MicroUsd(0),
                    in_tokens: 1_000,
                    out_tokens: 1_000,
                }
            })
            .collect()
    }

    #[benchmark]
    fn submit_report(v: Linear<1, MAX_REPORT_VOUCHERS>, e: Linear<1, MAX_REPORT_ENTRIES>) {
        let g = gateway::<T>();
        let per_voucher = 1_000_000u128;
        let vouchers = T::BenchmarkHelper::vouchers(&g, 0, v, per_voucher);
        let entries = entries::<T>(e, per_voucher * u128::from(v));
        let count = e;

        #[extrinsic_call]
        _(
            RawOrigin::Signed(g.clone()),
            [0xab; 32],
            count,
            BoundedVec::truncate_from(entries),
            BoundedVec::truncate_from(vouchers),
        );

        assert!(Pallet::<T>::report(0).is_some());
    }

    #[benchmark]
    fn claim(n: Linear<1, 64>) {
        let g = gateway::<T>();
        let challenge = EpochIndex::from(Params::<T>::get().challenge_epochs);
        let provider: AccountId32 = account("provider", 0, 0);
        // One report per epoch, each maturing in its own epoch.
        for i in 0..n {
            T::BenchmarkHelper::set_epoch(EpochIndex::from(i));
            let vouchers = T::BenchmarkHelper::vouchers(&g, i, 1, 1_000_000);
            Pallet::<T>::submit_report(
                RawOrigin::Signed(g.clone()).into(),
                [0xab; 32],
                1,
                BoundedVec::truncate_from(entries::<T>(1, 1_000_000)),
                BoundedVec::truncate_from(vouchers),
            )
            .unwrap();
        }
        let matured: Vec<EpochIndex> = (0..n).map(|i| EpochIndex::from(i) + challenge).collect();
        T::BenchmarkHelper::set_epoch(EpochIndex::from(n) + challenge + 1);
        let pot = Pallet::<T>::pot();
        // Well above the existential deposit, which an empty pot's first mint must reach.
        let market = T::Currency::minimum_balance().saturating_mul(1_000);
        for e in &matured {
            T::Currency::mint_into(&pot, market).unwrap();
            let (work, _) = <Pallet<T> as WorkSource>::verified_work(*e);
            <Pallet<T> as MarketPayout<AccountId32>>::settled(*e, market, work);
        }
        let items: Vec<(EpochIndex, AccountId32)> =
            matured.iter().map(|e| (*e, g.clone())).collect();
        let caller: AccountId32 = account("caller", 0, 0);

        #[extrinsic_call]
        _(
            RawOrigin::Signed(caller),
            provider.clone(),
            BoundedVec::truncate_from(items),
        );

        assert!(Pallet::<T>::work(&provider).is_empty());
    }

    impl_benchmark_test_suite!(Pallet, crate::mock::ext(), crate::mock::Test);
}
