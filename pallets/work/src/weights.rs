//! Weights for `pallet_work`.
//!
//! Provisional values until `scripts/benchmark-pallet.sh` is run against the runtime
//! (m5-work-settlement 3.5): a voucher costs about one ML-DSA-87 redemption (1.6 ms), an entry a
//! provider lookup and two held-balance writes, a claim item two transfers.

#![allow(unused_parens, unused_imports, missing_docs, clippy::unnecessary_cast)]

use core::marker::PhantomData;
use frame_support::{
    traits::Get,
    weights::{Weight, constants::RocksDbWeight},
};

/// Weight functions of `pallet_work`.
pub trait WeightInfo {
    /// Weight of `submit_report` with `v` vouchers and `e` entries.
    fn submit_report(v: u32, e: u32) -> Weight;
    /// Weight of `claim` with `n` items.
    fn claim(n: u32) -> Weight;
}

/// Weights with the runtime's database weights.
pub struct SubstrateWeight<T>(PhantomData<T>);
impl<T: frame_system::Config> WeightInfo for SubstrateWeight<T> {
    fn submit_report(v: u32, e: u32) -> Weight {
        Weight::from_parts(60_000_000, 8_000)
            .saturating_add(Weight::from_parts(1_700_000_000, 6_000).saturating_mul(v.into()))
            .saturating_add(Weight::from_parts(40_000_000, 3_000).saturating_mul(e.into()))
            .saturating_add(T::DbWeight::get().reads(8_u64))
            .saturating_add(T::DbWeight::get().writes(8_u64))
            .saturating_add(T::DbWeight::get().reads((5_u64).saturating_mul(v.into())))
            .saturating_add(T::DbWeight::get().writes((3_u64).saturating_mul(v.into())))
            .saturating_add(T::DbWeight::get().reads((4_u64).saturating_mul(e.into())))
            .saturating_add(T::DbWeight::get().writes((4_u64).saturating_mul(e.into())))
    }
    fn claim(n: u32) -> Weight {
        Weight::from_parts(20_000_000, 3_000)
            .saturating_add(Weight::from_parts(80_000_000, 6_000).saturating_mul(n.into()))
            .saturating_add(T::DbWeight::get().reads((6_u64).saturating_mul(n.into())))
            .saturating_add(T::DbWeight::get().writes((6_u64).saturating_mul(n.into())))
    }
}

// For tests.
impl WeightInfo for () {
    fn submit_report(v: u32, e: u32) -> Weight {
        Weight::from_parts(1_700_000_000, 6_000)
            .saturating_mul(v.into())
            .saturating_add(Weight::from_parts(40_000_000, 3_000).saturating_mul(e.into()))
            .saturating_add(RocksDbWeight::get().reads_writes(8, 8))
    }
    fn claim(n: u32) -> Weight {
        Weight::from_parts(80_000_000, 6_000)
            .saturating_mul(n.into())
            .saturating_add(RocksDbWeight::get().reads_writes(6, 6))
    }
}
