//! Weights for `pallet_audit`.
//!
//! Provisional values until `scripts/benchmark-pallet.sh pallet_audit pallets/audit/src/weights.rs`
//! regenerates this file (m6-audit-chain task 3.7); they must not reach a testnet.

#![allow(unused_parens, unused_imports, missing_docs, clippy::unnecessary_cast)]

use core::marker::PhantomData;
use frame_support::{
    traits::Get,
    weights::{Weight, constants::RocksDbWeight},
};

/// Weight functions of `pallet_audit`.
pub trait WeightInfo {
    /// Benchmarked weight of `register`.
    fn register() -> Weight;
    /// Benchmarked weight of `bond_extra`.
    fn bond_extra() -> Weight;
    /// Benchmarked weight of `unbond`.
    fn unbond() -> Weight;
    /// Benchmarked weight of `exit`.
    fn exit() -> Weight;
    /// Benchmarked weight of `withdraw_unbonded`.
    fn withdraw_unbonded() -> Weight;
    /// Benchmarked weight of `submit_verdict`.
    fn submit_verdict() -> Weight;
    /// Benchmarked weight of `vote`.
    fn vote() -> Weight;
    /// Benchmarked weight of `close_dispute`.
    fn close_dispute() -> Weight;
    /// Benchmarked weight of `set_params`.
    fn set_params() -> Weight;
    /// Benchmarked weight of `start_round`.
    fn start_round(n: u32) -> Weight;
    /// Benchmarked weight of `prune`.
    fn prune(n: u32) -> Weight;
}

/// Weights measured on the reference machine, with the runtime's database weights.
pub struct SubstrateWeight<T>(PhantomData<T>);
impl<T: frame_system::Config> WeightInfo for SubstrateWeight<T> {
    fn register() -> Weight {
        Weight::from_parts(60_000_000, 6000)
            .saturating_add(T::DbWeight::get().reads(4_u64))
            .saturating_add(T::DbWeight::get().writes(3_u64))
    }
    fn bond_extra() -> Weight {
        Weight::from_parts(60_000_000, 6000)
            .saturating_add(T::DbWeight::get().reads(4_u64))
            .saturating_add(T::DbWeight::get().writes(3_u64))
    }
    fn unbond() -> Weight {
        Weight::from_parts(60_000_000, 6000)
            .saturating_add(T::DbWeight::get().reads(4_u64))
            .saturating_add(T::DbWeight::get().writes(3_u64))
    }
    fn exit() -> Weight {
        Weight::from_parts(60_000_000, 6000)
            .saturating_add(T::DbWeight::get().reads(4_u64))
            .saturating_add(T::DbWeight::get().writes(3_u64))
    }
    fn withdraw_unbonded() -> Weight {
        Weight::from_parts(60_000_000, 6000)
            .saturating_add(T::DbWeight::get().reads(4_u64))
            .saturating_add(T::DbWeight::get().writes(3_u64))
    }
    fn submit_verdict() -> Weight {
        Weight::from_parts(400_000_000, 20000)
            .saturating_add(T::DbWeight::get().reads(12_u64))
            .saturating_add(T::DbWeight::get().writes(8_u64))
    }
    fn vote() -> Weight {
        Weight::from_parts(800_000_000, 60000)
            .saturating_add(T::DbWeight::get().reads(40_u64))
            .saturating_add(T::DbWeight::get().writes(40_u64))
    }
    fn close_dispute() -> Weight {
        Weight::from_parts(500_000_000, 40000)
            .saturating_add(T::DbWeight::get().reads(30_u64))
            .saturating_add(T::DbWeight::get().writes(30_u64))
    }
    fn set_params() -> Weight {
        Weight::from_parts(60_000_000, 6000)
            .saturating_add(T::DbWeight::get().reads(4_u64))
            .saturating_add(T::DbWeight::get().writes(3_u64))
    }
    fn start_round(n: u32) -> Weight {
        Weight::from_parts(10_000_000, 1_000)
            .saturating_add(Weight::from_parts(50_000_000, 0).saturating_mul(n.into()))
            .saturating_add(T::DbWeight::get().reads((1_u64).saturating_add(n.into())))
            .saturating_add(T::DbWeight::get().writes((4_u64).saturating_add(n.into())))
    }
    fn prune(n: u32) -> Weight {
        Weight::from_parts(10_000_000, 1_000)
            .saturating_add(Weight::from_parts(50_000_000, 0).saturating_mul(n.into()))
            .saturating_add(T::DbWeight::get().reads((1_u64).saturating_add(n.into())))
            .saturating_add(T::DbWeight::get().writes((4_u64).saturating_add(n.into())))
    }
}

impl WeightInfo for () {
    fn register() -> Weight {
        Weight::from_parts(60_000_000, 6000)
            .saturating_add(RocksDbWeight::get().reads(4_u64))
            .saturating_add(RocksDbWeight::get().writes(3_u64))
    }
    fn bond_extra() -> Weight {
        Weight::from_parts(60_000_000, 6000)
            .saturating_add(RocksDbWeight::get().reads(4_u64))
            .saturating_add(RocksDbWeight::get().writes(3_u64))
    }
    fn unbond() -> Weight {
        Weight::from_parts(60_000_000, 6000)
            .saturating_add(RocksDbWeight::get().reads(4_u64))
            .saturating_add(RocksDbWeight::get().writes(3_u64))
    }
    fn exit() -> Weight {
        Weight::from_parts(60_000_000, 6000)
            .saturating_add(RocksDbWeight::get().reads(4_u64))
            .saturating_add(RocksDbWeight::get().writes(3_u64))
    }
    fn withdraw_unbonded() -> Weight {
        Weight::from_parts(60_000_000, 6000)
            .saturating_add(RocksDbWeight::get().reads(4_u64))
            .saturating_add(RocksDbWeight::get().writes(3_u64))
    }
    fn submit_verdict() -> Weight {
        Weight::from_parts(400_000_000, 20000)
            .saturating_add(RocksDbWeight::get().reads(12_u64))
            .saturating_add(RocksDbWeight::get().writes(8_u64))
    }
    fn vote() -> Weight {
        Weight::from_parts(800_000_000, 60000)
            .saturating_add(RocksDbWeight::get().reads(40_u64))
            .saturating_add(RocksDbWeight::get().writes(40_u64))
    }
    fn close_dispute() -> Weight {
        Weight::from_parts(500_000_000, 40000)
            .saturating_add(RocksDbWeight::get().reads(30_u64))
            .saturating_add(RocksDbWeight::get().writes(30_u64))
    }
    fn set_params() -> Weight {
        Weight::from_parts(60_000_000, 6000)
            .saturating_add(RocksDbWeight::get().reads(4_u64))
            .saturating_add(RocksDbWeight::get().writes(3_u64))
    }
    fn start_round(n: u32) -> Weight {
        Weight::from_parts(10_000_000, 1_000)
            .saturating_add(Weight::from_parts(50_000_000, 0).saturating_mul(n.into()))
            .saturating_add(RocksDbWeight::get().reads((1_u64).saturating_add(n.into())))
            .saturating_add(RocksDbWeight::get().writes((4_u64).saturating_add(n.into())))
    }
    fn prune(n: u32) -> Weight {
        Weight::from_parts(10_000_000, 1_000)
            .saturating_add(Weight::from_parts(50_000_000, 0).saturating_mul(n.into()))
            .saturating_add(RocksDbWeight::get().reads((1_u64).saturating_add(n.into())))
            .saturating_add(RocksDbWeight::get().writes((4_u64).saturating_add(n.into())))
    }
}
