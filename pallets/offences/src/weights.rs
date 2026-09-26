//! Weights for `pallet_ac_offences`. Replaced by `scripts/benchmark-pallet.sh` output.

#![allow(unused_parens, unused_imports, missing_docs, clippy::unnecessary_cast)]

use core::marker::PhantomData;
use frame_support::{
    traits::Get,
    weights::{Weight, constants::RocksDbWeight},
};

/// Weight functions of `pallet_ac_offences`.
pub trait WeightInfo {
    /// Benchmarked weight of `report_equivocation`.
    fn report_equivocation() -> Weight;
    /// Benchmarked weight of `authorize_report_equivocation`.
    fn authorize_report_equivocation() -> Weight;
}

/// Weights measured on the reference machine, with the runtime's database weights.
pub struct SubstrateWeight<T>(PhantomData<T>);
impl<T: frame_system::Config> WeightInfo for SubstrateWeight<T> {
    fn report_equivocation() -> Weight {
        Weight::zero()
    }
    fn authorize_report_equivocation() -> Weight {
        Weight::zero()
    }
}

// For backwards compatibility and tests.
impl WeightInfo for () {
    fn report_equivocation() -> Weight {
        Weight::zero()
    }
    fn authorize_report_equivocation() -> Weight {
        Weight::zero()
    }
}
