//! Weights of `pallet-pq-accounts`.
//!
//! `SubstrateWeight` is replaced by benchmark output in m1-pq-chain task 4.4.

use frame_support::weights::Weight;

/// Weight functions of this pallet.
pub trait WeightInfo {
    /// `rotate_key`, worst case (ML-DSA-87 proof of possession).
    fn rotate_key() -> Weight;
    /// `PqAuthorize` with an ML-DSA-44 signature.
    fn authorize_ml_dsa_44() -> Weight;
    /// `PqAuthorize` with an ML-DSA-65 signature.
    fn authorize_ml_dsa_65() -> Weight;
    /// `PqAuthorize` with an ML-DSA-87 signature.
    fn authorize_ml_dsa_87() -> Weight;
    /// Extra cost of registering a first key.
    fn register_key() -> Weight;
}

impl WeightInfo for () {
    fn rotate_key() -> Weight {
        Weight::zero()
    }
    fn authorize_ml_dsa_44() -> Weight {
        Weight::zero()
    }
    fn authorize_ml_dsa_65() -> Weight {
        Weight::zero()
    }
    fn authorize_ml_dsa_87() -> Weight {
        Weight::zero()
    }
    fn register_key() -> Weight {
        Weight::zero()
    }
}
