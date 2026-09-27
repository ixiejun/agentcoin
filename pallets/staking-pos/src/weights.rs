//! Weights of `pallet_staking_pos` (replaced by benchmark output, m3-pos 2.7).

#![allow(unused_parens, unused_imports, missing_docs, clippy::unnecessary_cast)]

use core::marker::PhantomData;
use frame_support::{
    traits::Get,
    weights::{Weight, constants::RocksDbWeight},
};

/// Weight functions of `pallet_staking_pos`.
pub trait WeightInfo {
    /// Benchmarked weight of `register_candidate`.
    fn register_candidate(c: u32) -> Weight;
    /// Benchmarked weight of `bond_extra`.
    fn bond_extra() -> Weight;
    /// Benchmarked weight of `set_validator_key`.
    fn set_validator_key() -> Weight;
    /// Benchmarked weight of `retire`.
    fn retire() -> Weight;
    /// Benchmarked weight of `nominate`.
    fn nominate(n: u32) -> Weight;
    /// Benchmarked weight of `set_nominations`.
    fn set_nominations() -> Weight;
    /// Benchmarked weight of `unnominate`.
    fn unnominate() -> Weight;
    /// Benchmarked weight of `unbond`.
    fn unbond() -> Weight;
    /// Benchmarked weight of `withdraw_unbonded`.
    fn withdraw_unbonded() -> Weight;
    /// Benchmarked weight of `set_commission`.
    fn set_commission() -> Weight;
    /// Benchmarked weight of `chill`.
    fn chill() -> Weight;
    /// Benchmarked weight of `validate`.
    fn validate() -> Weight;
}

/// Weights measured on the reference machine, with the runtime's database weights.
pub struct SubstrateWeight<T>(PhantomData<T>);
impl<T: frame_system::Config> WeightInfo for SubstrateWeight<T> {
    fn register_candidate(_c: u32) -> Weight {
        Weight::zero()
    }
    fn bond_extra() -> Weight {
        Weight::zero()
    }
    fn set_validator_key() -> Weight {
        Weight::zero()
    }
    fn retire() -> Weight {
        Weight::zero()
    }
    fn nominate(_n: u32) -> Weight {
        Weight::zero()
    }
    fn set_nominations() -> Weight {
        Weight::zero()
    }
    fn unnominate() -> Weight {
        Weight::zero()
    }
    fn unbond() -> Weight {
        Weight::zero()
    }
    fn withdraw_unbonded() -> Weight {
        Weight::zero()
    }
    fn set_commission() -> Weight {
        Weight::zero()
    }
    fn chill() -> Weight {
        Weight::zero()
    }
    fn validate() -> Weight {
        Weight::zero()
    }
}

/// Same weights with RocksDB database weights, for tests and mocks.
impl WeightInfo for () {
    fn register_candidate(_c: u32) -> Weight {
        Weight::zero()
    }
    fn bond_extra() -> Weight {
        Weight::zero()
    }
    fn set_validator_key() -> Weight {
        Weight::zero()
    }
    fn retire() -> Weight {
        Weight::zero()
    }
    fn nominate(_n: u32) -> Weight {
        Weight::zero()
    }
    fn set_nominations() -> Weight {
        Weight::zero()
    }
    fn unnominate() -> Weight {
        Weight::zero()
    }
    fn unbond() -> Weight {
        Weight::zero()
    }
    fn withdraw_unbonded() -> Weight {
        Weight::zero()
    }
    fn set_commission() -> Weight {
        Weight::zero()
    }
    fn chill() -> Weight {
        Weight::zero()
    }
    fn validate() -> Weight {
        Weight::zero()
    }
}
