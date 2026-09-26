//! Weights for `pallet_randomness_cr`. Replaced by `scripts/benchmark-pallet.sh` output.

#![allow(unused_parens, unused_imports, missing_docs, clippy::unnecessary_cast)]

use core::marker::PhantomData;
use frame_support::{
    traits::Get,
    weights::{Weight, constants::RocksDbWeight},
};

/// Weight functions of `pallet_randomness_cr`.
pub trait WeightInfo {
    /// Benchmarked weight of `note_randomness`.
    fn note_randomness() -> Weight;
    /// Benchmarked weight of `conclude_epoch`.
    fn conclude_epoch(r: u32) -> Weight;
}

/// Weights measured on the reference machine, with the runtime's database weights.
pub struct SubstrateWeight<T>(PhantomData<T>);
impl<T: frame_system::Config> WeightInfo for SubstrateWeight<T> {
    fn note_randomness() -> Weight {
        Weight::zero()
    }
    fn conclude_epoch(_r: u32) -> Weight {
        Weight::zero()
    }
}

// For backwards compatibility and tests.
impl WeightInfo for () {
    fn note_randomness() -> Weight {
        Weight::zero()
    }
    fn conclude_epoch(_r: u32) -> Weight {
        Weight::zero()
    }
}
