//! Weights of `pallet_public_jobs`.
//!
//! Placeholder values until `scripts/benchmark-pallet.sh pallet_public_jobs` regenerates this
//! file from the benchmarks (m6-public-jobs task 4.4); they must not reach a testnet.

#![allow(unused_parens, unused_imports, missing_docs, clippy::unnecessary_cast)]

use core::marker::PhantomData;
use frame_support::{
    traits::Get,
    weights::{Weight, constants::RocksDbWeight},
};

/// Weight functions of `pallet_public_jobs`.
pub trait WeightInfo {
    /// Benchmarked weight of `register`.
    fn register() -> Weight;
    /// Benchmarked weight of `set_models`.
    fn set_models() -> Weight;
    /// Benchmarked weight of `deregister`.
    fn deregister() -> Weight;
    /// Benchmarked weight of `ready`.
    fn ready() -> Weight;
    /// Benchmarked weight of `publish`.
    fn publish() -> Weight;
    /// Benchmarked weight of `cancel`.
    fn cancel() -> Weight;
    /// Benchmarked weight of `commit`.
    fn commit() -> Weight;
    /// Benchmarked weight of `reveal`.
    fn reveal() -> Weight;
    /// Benchmarked weight of `close`.
    fn close() -> Weight;
    /// Benchmarked weight of `reveal_canary`.
    fn reveal_canary() -> Weight;
    /// Benchmarked weight of `claim`.
    fn claim(n: u32) -> Weight;
    /// Benchmarked weight of `withdraw`.
    fn withdraw() -> Weight;
    /// Benchmarked weight of `set_price_cap`.
    fn set_price_cap() -> Weight;
    /// Benchmarked weight of `start_round`.
    fn start_round(w: u32, u: u32) -> Weight;
    /// Benchmarked weight of `prune`.
    fn prune(n: u32) -> Weight;
}

/// Weights measured on the reference machine, with the runtime's database weights.
pub struct SubstrateWeight<T>(PhantomData<T>);
impl<T: frame_system::Config> WeightInfo for SubstrateWeight<T> {
    fn register() -> Weight {
        Weight::from_parts(100_000_000, 10_000)
            .saturating_add(T::DbWeight::get().reads(10_u64))
            .saturating_add(T::DbWeight::get().writes(10_u64))
    }
    fn set_models() -> Weight {
        Weight::from_parts(100_000_000, 10_000)
            .saturating_add(T::DbWeight::get().reads(10_u64))
            .saturating_add(T::DbWeight::get().writes(10_u64))
    }
    fn deregister() -> Weight {
        Weight::from_parts(100_000_000, 10_000)
            .saturating_add(T::DbWeight::get().reads(10_u64))
            .saturating_add(T::DbWeight::get().writes(10_u64))
    }
    fn ready() -> Weight {
        Weight::from_parts(100_000_000, 10_000)
            .saturating_add(T::DbWeight::get().reads(10_u64))
            .saturating_add(T::DbWeight::get().writes(10_u64))
    }
    fn publish() -> Weight {
        Weight::from_parts(100_000_000, 10_000)
            .saturating_add(T::DbWeight::get().reads(10_u64))
            .saturating_add(T::DbWeight::get().writes(10_u64))
    }
    fn cancel() -> Weight {
        Weight::from_parts(100_000_000, 10_000)
            .saturating_add(T::DbWeight::get().reads(10_u64))
            .saturating_add(T::DbWeight::get().writes(10_u64))
    }
    fn commit() -> Weight {
        Weight::from_parts(100_000_000, 10_000)
            .saturating_add(T::DbWeight::get().reads(10_u64))
            .saturating_add(T::DbWeight::get().writes(10_u64))
    }
    fn reveal() -> Weight {
        Weight::from_parts(100_000_000, 10_000)
            .saturating_add(T::DbWeight::get().reads(10_u64))
            .saturating_add(T::DbWeight::get().writes(10_u64))
    }
    fn close() -> Weight {
        Weight::from_parts(100_000_000, 10_000)
            .saturating_add(T::DbWeight::get().reads(10_u64))
            .saturating_add(T::DbWeight::get().writes(10_u64))
    }
    fn reveal_canary() -> Weight {
        Weight::from_parts(100_000_000, 10_000)
            .saturating_add(T::DbWeight::get().reads(10_u64))
            .saturating_add(T::DbWeight::get().writes(10_u64))
    }
    fn claim(_n: u32) -> Weight {
        Weight::from_parts(100_000_000, 10_000)
            .saturating_add(T::DbWeight::get().reads(10_u64))
            .saturating_add(T::DbWeight::get().writes(10_u64))
    }
    fn withdraw() -> Weight {
        Weight::from_parts(100_000_000, 10_000)
            .saturating_add(T::DbWeight::get().reads(10_u64))
            .saturating_add(T::DbWeight::get().writes(10_u64))
    }
    fn set_price_cap() -> Weight {
        Weight::from_parts(100_000_000, 10_000)
            .saturating_add(T::DbWeight::get().reads(10_u64))
            .saturating_add(T::DbWeight::get().writes(10_u64))
    }
    fn start_round(_w: u32, _u: u32) -> Weight {
        Weight::from_parts(100_000_000, 10_000)
            .saturating_add(T::DbWeight::get().reads(10_u64))
            .saturating_add(T::DbWeight::get().writes(10_u64))
    }
    fn prune(_n: u32) -> Weight {
        Weight::from_parts(100_000_000, 10_000)
            .saturating_add(T::DbWeight::get().reads(10_u64))
            .saturating_add(T::DbWeight::get().writes(10_u64))
    }
}

// For tests.
impl WeightInfo for () {
    fn register() -> Weight {
        Weight::from_parts(100_000_000, 10_000)
            .saturating_add(RocksDbWeight::get().reads(10_u64))
            .saturating_add(RocksDbWeight::get().writes(10_u64))
    }
    fn set_models() -> Weight {
        Weight::from_parts(100_000_000, 10_000)
            .saturating_add(RocksDbWeight::get().reads(10_u64))
            .saturating_add(RocksDbWeight::get().writes(10_u64))
    }
    fn deregister() -> Weight {
        Weight::from_parts(100_000_000, 10_000)
            .saturating_add(RocksDbWeight::get().reads(10_u64))
            .saturating_add(RocksDbWeight::get().writes(10_u64))
    }
    fn ready() -> Weight {
        Weight::from_parts(100_000_000, 10_000)
            .saturating_add(RocksDbWeight::get().reads(10_u64))
            .saturating_add(RocksDbWeight::get().writes(10_u64))
    }
    fn publish() -> Weight {
        Weight::from_parts(100_000_000, 10_000)
            .saturating_add(RocksDbWeight::get().reads(10_u64))
            .saturating_add(RocksDbWeight::get().writes(10_u64))
    }
    fn cancel() -> Weight {
        Weight::from_parts(100_000_000, 10_000)
            .saturating_add(RocksDbWeight::get().reads(10_u64))
            .saturating_add(RocksDbWeight::get().writes(10_u64))
    }
    fn commit() -> Weight {
        Weight::from_parts(100_000_000, 10_000)
            .saturating_add(RocksDbWeight::get().reads(10_u64))
            .saturating_add(RocksDbWeight::get().writes(10_u64))
    }
    fn reveal() -> Weight {
        Weight::from_parts(100_000_000, 10_000)
            .saturating_add(RocksDbWeight::get().reads(10_u64))
            .saturating_add(RocksDbWeight::get().writes(10_u64))
    }
    fn close() -> Weight {
        Weight::from_parts(100_000_000, 10_000)
            .saturating_add(RocksDbWeight::get().reads(10_u64))
            .saturating_add(RocksDbWeight::get().writes(10_u64))
    }
    fn reveal_canary() -> Weight {
        Weight::from_parts(100_000_000, 10_000)
            .saturating_add(RocksDbWeight::get().reads(10_u64))
            .saturating_add(RocksDbWeight::get().writes(10_u64))
    }
    fn claim(_n: u32) -> Weight {
        Weight::from_parts(100_000_000, 10_000)
            .saturating_add(RocksDbWeight::get().reads(10_u64))
            .saturating_add(RocksDbWeight::get().writes(10_u64))
    }
    fn withdraw() -> Weight {
        Weight::from_parts(100_000_000, 10_000)
            .saturating_add(RocksDbWeight::get().reads(10_u64))
            .saturating_add(RocksDbWeight::get().writes(10_u64))
    }
    fn set_price_cap() -> Weight {
        Weight::from_parts(100_000_000, 10_000)
            .saturating_add(RocksDbWeight::get().reads(10_u64))
            .saturating_add(RocksDbWeight::get().writes(10_u64))
    }
    fn start_round(_w: u32, _u: u32) -> Weight {
        Weight::from_parts(100_000_000, 10_000)
            .saturating_add(RocksDbWeight::get().reads(10_u64))
            .saturating_add(RocksDbWeight::get().writes(10_u64))
    }
    fn prune(_n: u32) -> Weight {
        Weight::from_parts(100_000_000, 10_000)
            .saturating_add(RocksDbWeight::get().reads(10_u64))
            .saturating_add(RocksDbWeight::get().writes(10_u64))
    }
}
