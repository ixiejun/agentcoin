//! Weights of `pallet_model_registry`.
//!
//! Provisional until `scripts/benchmark-pallet.sh pallet_model_registry
//! pallets/model-registry/src/weights.rs` runs against the runtime (m5-market-registry 3.2,
//! generated with the runtime wiring); do not ship to a testnet as is.

#![allow(unused_parens, unused_imports, missing_docs, clippy::unnecessary_cast)]

use core::marker::PhantomData;
use frame_support::{
    traits::Get,
    weights::{Weight, constants::RocksDbWeight},
};

/// Weight functions of `pallet_model_registry`.
pub trait WeightInfo {
    /// Benchmarked weight of `register` with `s` shards.
    fn register(s: u32) -> Weight;
}

/// Weights measured on the reference machine, with the runtime's database weights.
pub struct SubstrateWeight<T>(PhantomData<T>);
impl<T: frame_system::Config> WeightInfo for SubstrateWeight<T> {
    fn register(s: u32) -> Weight {
        Weight::from_parts(60_000_000, 40_000)
            .saturating_add(Weight::from_parts(200_000, 32).saturating_mul(s.into()))
            .saturating_add(T::DbWeight::get().reads(4_u64))
            .saturating_add(T::DbWeight::get().writes(3_u64))
    }
}

// For backwards compatibility and tests.
impl WeightInfo for () {
    fn register(s: u32) -> Weight {
        Weight::from_parts(60_000_000, 40_000)
            .saturating_add(Weight::from_parts(200_000, 32).saturating_mul(s.into()))
            .saturating_add(RocksDbWeight::get().reads(4_u64))
            .saturating_add(RocksDbWeight::get().writes(3_u64))
    }
}
