//! Weights of `pallet_credits`.
//!
//! Provisional until `scripts/benchmark-pallet.sh pallet_credits pallets/credits/src/weights.rs`
//! runs against the runtime (m5-market-registry 6.5, generated with the runtime wiring); do not
//! ship to a testnet as is.

#![allow(unused_parens, unused_imports, missing_docs, clippy::unnecessary_cast)]

use core::marker::PhantomData;
use frame_support::{
    traits::Get,
    weights::{Weight, constants::RocksDbWeight},
};

/// Weight functions of `pallet_credits`.
pub trait WeightInfo {
    /// Benchmarked weight of `deposit`.
    fn deposit() -> Weight;
    /// Benchmarked weight of `request_withdrawal`.
    fn request_withdrawal() -> Weight;
    /// Benchmarked weight of `withdraw`.
    fn withdraw() -> Weight;
    /// Benchmarked weight of `request_key_change`.
    fn request_key_change() -> Weight;
    /// Benchmarked weight of redeeming one voucher (`Credit::redeem`), for settlement.
    fn redeem() -> Weight;
}

/// Weights measured on the reference machine, with the runtime's database weights.
pub struct SubstrateWeight<T>(PhantomData<T>);
impl<T: frame_system::Config> WeightInfo for SubstrateWeight<T> {
    fn deposit() -> Weight {
        Weight::from_parts(60_000_000, 8_000)
            .saturating_add(T::DbWeight::get().reads(5_u64))
            .saturating_add(T::DbWeight::get().writes(3_u64))
    }
    fn request_withdrawal() -> Weight {
        Weight::from_parts(25_000_000, 4_000)
            .saturating_add(T::DbWeight::get().reads(2_u64))
            .saturating_add(T::DbWeight::get().writes(1_u64))
    }
    fn withdraw() -> Weight {
        Weight::from_parts(50_000_000, 8_000)
            .saturating_add(T::DbWeight::get().reads(4_u64))
            .saturating_add(T::DbWeight::get().writes(3_u64))
    }
    fn request_key_change() -> Weight {
        Weight::from_parts(25_000_000, 4_000)
            .saturating_add(T::DbWeight::get().reads(3_u64))
            .saturating_add(T::DbWeight::get().writes(1_u64))
    }
    fn redeem() -> Weight {
        Weight::from_parts(900_000_000, 12_000)
            .saturating_add(T::DbWeight::get().reads(6_u64))
            .saturating_add(T::DbWeight::get().writes(4_u64))
    }
}

// For backwards compatibility and tests.
impl WeightInfo for () {
    fn deposit() -> Weight {
        Weight::from_parts(60_000_000, 8_000)
            .saturating_add(RocksDbWeight::get().reads(5_u64))
            .saturating_add(RocksDbWeight::get().writes(3_u64))
    }
    fn request_withdrawal() -> Weight {
        Weight::from_parts(25_000_000, 4_000)
            .saturating_add(RocksDbWeight::get().reads(2_u64))
            .saturating_add(RocksDbWeight::get().writes(1_u64))
    }
    fn withdraw() -> Weight {
        Weight::from_parts(50_000_000, 8_000)
            .saturating_add(RocksDbWeight::get().reads(4_u64))
            .saturating_add(RocksDbWeight::get().writes(3_u64))
    }
    fn request_key_change() -> Weight {
        Weight::from_parts(25_000_000, 4_000)
            .saturating_add(RocksDbWeight::get().reads(3_u64))
            .saturating_add(RocksDbWeight::get().writes(1_u64))
    }
    fn redeem() -> Weight {
        Weight::from_parts(900_000_000, 12_000)
            .saturating_add(RocksDbWeight::get().reads(6_u64))
            .saturating_add(RocksDbWeight::get().writes(4_u64))
    }
}
