//! Weights of `pallet_ref_rate`.
//!
//! Provisional until `scripts/benchmark-pallet.sh pallet_ref_rate pallets/ref-rate/src/weights.rs`
//! runs against the runtime (m5-market-registry 2.2, generated in group 7); do not ship to a
//! testnet as is.

#![allow(unused_parens, unused_imports, missing_docs, clippy::unnecessary_cast)]

use core::marker::PhantomData;
use frame_support::{
    traits::Get,
    weights::{Weight, constants::RocksDbWeight},
};

/// Weight functions of `pallet_ref_rate`.
pub trait WeightInfo {
    /// Benchmarked weight of `set_rate`.
    fn set_rate() -> Weight;
}

/// Weights measured on the reference machine, with the runtime's database weights.
pub struct SubstrateWeight<T>(PhantomData<T>);
impl<T: frame_system::Config> WeightInfo for SubstrateWeight<T> {
    fn set_rate() -> Weight {
        Weight::from_parts(20_000_000, 1_500)
            .saturating_add(T::DbWeight::get().reads(2_u64))
            .saturating_add(T::DbWeight::get().writes(1_u64))
    }
}

// For backwards compatibility and tests.
impl WeightInfo for () {
    fn set_rate() -> Weight {
        Weight::from_parts(20_000_000, 1_500)
            .saturating_add(RocksDbWeight::get().reads(2_u64))
            .saturating_add(RocksDbWeight::get().writes(1_u64))
    }
}
