//! Weights for `pallet_validator_set`. Replaced by `scripts/benchmark-pallet.sh` output.

#![allow(unused_parens, unused_imports, missing_docs, clippy::unnecessary_cast)]

use core::marker::PhantomData;
use frame_support::{
    traits::Get,
    weights::{Weight, constants::RocksDbWeight},
};

/// Weight functions of `pallet_validator_set`.
pub trait WeightInfo {
    /// Benchmarked weight of `epoch_boundary_with_change`.
    fn epoch_boundary_with_change(n: u32) -> Weight;
    /// Benchmarked weight of `epoch_boundary_without_change`.
    fn epoch_boundary_without_change(n: u32) -> Weight;
}

/// Weights measured on the reference machine, with the runtime's database weights.
pub struct SubstrateWeight<T>(PhantomData<T>);
impl<T: frame_system::Config> WeightInfo for SubstrateWeight<T> {
    fn epoch_boundary_with_change(_n: u32) -> Weight {
        Weight::zero()
    }
    fn epoch_boundary_without_change(_n: u32) -> Weight {
        Weight::zero()
    }
}

// For backwards compatibility and tests.
impl WeightInfo for () {
    fn epoch_boundary_with_change(_n: u32) -> Weight {
        Weight::zero()
    }
    fn epoch_boundary_without_change(_n: u32) -> Weight {
        Weight::zero()
    }
}
