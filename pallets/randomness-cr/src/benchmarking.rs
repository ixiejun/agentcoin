//! Benchmarks: recording a commitment and a reveal, and concluding an epoch.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)] // Benchmark setup.

use frame_benchmarking::v2::benchmarks;

use crate::{AccountBytes, Commits, Config, Pallet, Reveals};

fn account(i: u32) -> AccountBytes {
    let mut a = [0u8; 32];
    a[..4].copy_from_slice(&i.to_le_bytes());
    a
}

fn secret(i: u32) -> [u8; 32] {
    let mut s = [7u8; 32];
    s[..4].copy_from_slice(&i.to_le_bytes());
    s
}

#[benchmarks]
mod benchmarks {
    use super::{Commits, Config, Pallet, Reveals, account, secret};
    use frame_benchmarking::v2::BenchmarkError;
    // The macro expands to code naming `impl_test_function` unqualified.
    use frame_benchmarking::impl_test_function;
    use frame_support::traits::Get;
    use sp_runtime::traits::Zero;

    /// Worst case: a new commitment plus a reveal checked against its commitment.
    #[benchmark]
    fn note_randomness() -> Result<(), BenchmarkError> {
        let who = account(0);
        Commits::<T>::insert(0, who, ac_crypto::randomness_commit(&secret(0)).unwrap());
        #[block]
        {
            Pallet::<T>::note(who, 1, Some([1u8; 32]), Some(secret(0)));
        }
        assert!(Reveals::<T>::contains_key(0, who));
        assert!(Commits::<T>::contains_key(1, who));
        Ok(())
    }

    /// Concluding an epoch with `r` reveals among the maximum number of commitments.
    #[benchmark]
    fn conclude_epoch(r: Linear<0, { T::MaxAuthorities::get() }>) -> Result<(), BenchmarkError> {
        for i in 0..T::MaxAuthorities::get() {
            Commits::<T>::insert(0, account(i), [0u8; 32]);
            if i < r {
                Reveals::<T>::insert(0, account(i), secret(i));
            }
        }
        #[block]
        {
            Pallet::<T>::conclude(0, Zero::zero());
        }
        assert_eq!(Commits::<T>::iter_prefix(0).count(), 0);
        Ok(())
    }

    impl_benchmark_test_suite!(Pallet, crate::mock::new_bench_ext(), crate::mock::Test);
}
