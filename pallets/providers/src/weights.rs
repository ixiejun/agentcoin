//! Weights of `pallet_providers`.
//!
//! Provisional until `scripts/benchmark-pallet.sh pallet_providers pallets/providers/src/weights.rs`
//! runs against the runtime (m5-market-registry 4.5, generated with the runtime wiring); do not
//! ship to a testnet as is.

#![allow(unused_parens, unused_imports, missing_docs, clippy::unnecessary_cast)]

use core::marker::PhantomData;
use frame_support::{
    traits::Get,
    weights::{Weight, constants::RocksDbWeight},
};

/// Weight functions of `pallet_providers`.
pub trait WeightInfo {
    /// Benchmarked weight of `register` with `m` models.
    fn register(m: u32) -> Weight;
    /// Benchmarked weight of `update` replacing `m` models.
    fn update(m: u32) -> Weight;
    /// Benchmarked weight of `heartbeat`.
    fn heartbeat() -> Weight;
    /// Benchmarked weight of `bond_extra`.
    fn bond_extra() -> Weight;
    /// Benchmarked weight of `unbond`.
    fn unbond() -> Weight;
    /// Benchmarked weight of `exit` with `m` models.
    fn exit(m: u32) -> Weight;
    /// Benchmarked weight of `withdraw_unbonded`.
    fn withdraw_unbonded() -> Weight;
}

fn base(
    ref_time: u64,
    reads: u64,
    writes: u64,
    db: frame_support::weights::RuntimeDbWeight,
) -> Weight {
    Weight::from_parts(ref_time, 8_000)
        .saturating_add(db.reads(reads))
        .saturating_add(db.writes(writes))
}

/// Weights measured on the reference machine, with the runtime's database weights.
pub struct SubstrateWeight<T>(PhantomData<T>);
impl<T: frame_system::Config> WeightInfo for SubstrateWeight<T> {
    fn register(m: u32) -> Weight {
        base(80_000_000, 4, 3, T::DbWeight::get()).saturating_add(
            T::DbWeight::get()
                .reads_writes(1, 1)
                .saturating_mul(m.into()),
        )
    }
    fn update(m: u32) -> Weight {
        base(60_000_000, 1, 1, T::DbWeight::get()).saturating_add(
            T::DbWeight::get()
                .reads_writes(1, 2)
                .saturating_mul(m.into()),
        )
    }
    fn heartbeat() -> Weight {
        base(20_000_000, 2, 1, T::DbWeight::get())
    }
    fn bond_extra() -> Weight {
        base(50_000_000, 3, 3, T::DbWeight::get())
    }
    fn unbond() -> Weight {
        base(40_000_000, 3, 1, T::DbWeight::get())
    }
    fn exit(m: u32) -> Weight {
        base(40_000_000, 2, 1, T::DbWeight::get())
            .saturating_add(T::DbWeight::get().writes(1).saturating_mul(m.into()))
    }
    fn withdraw_unbonded() -> Weight {
        base(60_000_000, 3, 3, T::DbWeight::get())
    }
}

// For backwards compatibility and tests.
impl WeightInfo for () {
    fn register(m: u32) -> Weight {
        base(80_000_000, 4, 3, RocksDbWeight::get()).saturating_add(
            RocksDbWeight::get()
                .reads_writes(1, 1)
                .saturating_mul(m.into()),
        )
    }
    fn update(m: u32) -> Weight {
        base(60_000_000, 1, 1, RocksDbWeight::get()).saturating_add(
            RocksDbWeight::get()
                .reads_writes(1, 2)
                .saturating_mul(m.into()),
        )
    }
    fn heartbeat() -> Weight {
        base(20_000_000, 2, 1, RocksDbWeight::get())
    }
    fn bond_extra() -> Weight {
        base(50_000_000, 3, 3, RocksDbWeight::get())
    }
    fn unbond() -> Weight {
        base(40_000_000, 3, 1, RocksDbWeight::get())
    }
    fn exit(m: u32) -> Weight {
        base(40_000_000, 2, 1, RocksDbWeight::get())
            .saturating_add(RocksDbWeight::get().writes(1).saturating_mul(m.into()))
    }
    fn withdraw_unbonded() -> Weight {
        base(60_000_000, 3, 3, RocksDbWeight::get())
    }
}
