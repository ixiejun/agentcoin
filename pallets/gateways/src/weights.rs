//! Weights of `pallet_gateways`.
//!
//! Provisional until `scripts/benchmark-pallet.sh pallet_gateways pallets/gateways/src/weights.rs`
//! runs against the runtime (m5-market-registry 5.2, generated with the runtime wiring); do not
//! ship to a testnet as is.

#![allow(unused_parens, unused_imports, missing_docs, clippy::unnecessary_cast)]

use core::marker::PhantomData;
use frame_support::{
    traits::Get,
    weights::{Weight, constants::RocksDbWeight},
};

/// Weight functions of `pallet_gateways`.
pub trait WeightInfo {
    /// Benchmarked weight of `register`.
    fn register() -> Weight;
    /// Benchmarked weight of `update`.
    fn update() -> Weight;
    /// Benchmarked weight of `bond_extra`.
    fn bond_extra() -> Weight;
    /// Benchmarked weight of `unbond`.
    fn unbond() -> Weight;
    /// Benchmarked weight of `exit`.
    fn exit() -> Weight;
    /// Benchmarked weight of `withdraw_unbonded`.
    fn withdraw_unbonded() -> Weight;
}

/// Weights measured on the reference machine, with the runtime's database weights.
pub struct SubstrateWeight<T>(PhantomData<T>);
impl<T: frame_system::Config> WeightInfo for SubstrateWeight<T> {
    fn register() -> Weight {
        Weight::from_parts(60_000_000, 8_000)
            .saturating_add(T::DbWeight::get().reads(4_u64))
            .saturating_add(T::DbWeight::get().writes(3_u64))
    }
    fn update() -> Weight {
        Weight::from_parts(30_000_000, 8_000)
            .saturating_add(T::DbWeight::get().reads(2_u64))
            .saturating_add(T::DbWeight::get().writes(1_u64))
    }
    fn bond_extra() -> Weight {
        Weight::from_parts(50_000_000, 8_000)
            .saturating_add(T::DbWeight::get().reads(3_u64))
            .saturating_add(T::DbWeight::get().writes(3_u64))
    }
    fn unbond() -> Weight {
        Weight::from_parts(40_000_000, 8_000)
            .saturating_add(T::DbWeight::get().reads(3_u64))
            .saturating_add(T::DbWeight::get().writes(1_u64))
    }
    fn exit() -> Weight {
        Weight::from_parts(40_000_000, 8_000)
            .saturating_add(T::DbWeight::get().reads(2_u64))
            .saturating_add(T::DbWeight::get().writes(1_u64))
    }
    fn withdraw_unbonded() -> Weight {
        Weight::from_parts(60_000_000, 8_000)
            .saturating_add(T::DbWeight::get().reads(3_u64))
            .saturating_add(T::DbWeight::get().writes(3_u64))
    }
}

// For backwards compatibility and tests.
impl WeightInfo for () {
    fn register() -> Weight {
        Weight::from_parts(60_000_000, 8_000)
            .saturating_add(RocksDbWeight::get().reads(4_u64))
            .saturating_add(RocksDbWeight::get().writes(3_u64))
    }
    fn update() -> Weight {
        Weight::from_parts(30_000_000, 8_000)
            .saturating_add(RocksDbWeight::get().reads(2_u64))
            .saturating_add(RocksDbWeight::get().writes(1_u64))
    }
    fn bond_extra() -> Weight {
        Weight::from_parts(50_000_000, 8_000)
            .saturating_add(RocksDbWeight::get().reads(3_u64))
            .saturating_add(RocksDbWeight::get().writes(3_u64))
    }
    fn unbond() -> Weight {
        Weight::from_parts(40_000_000, 8_000)
            .saturating_add(RocksDbWeight::get().reads(3_u64))
            .saturating_add(RocksDbWeight::get().writes(1_u64))
    }
    fn exit() -> Weight {
        Weight::from_parts(40_000_000, 8_000)
            .saturating_add(RocksDbWeight::get().reads(2_u64))
            .saturating_add(RocksDbWeight::get().writes(1_u64))
    }
    fn withdraw_unbonded() -> Weight {
        Weight::from_parts(60_000_000, 8_000)
            .saturating_add(RocksDbWeight::get().reads(3_u64))
            .saturating_add(RocksDbWeight::get().writes(3_u64))
    }
}
