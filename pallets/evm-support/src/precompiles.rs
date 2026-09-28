//! Post-quantum precompiles (spec evm/precompiles; m4-evm design D6).
//!
//! | Precompile | Address | Interface |
//! |---|---|---|
//! | [`PqVerify`] | `0x…0a010000` | `verify(uint8 alg, bytes publicKey, bytes message, bytes signature) returns (bool)` |
//! | [`Blake3`] | `0x…0a020000` | `hash(bytes data) returns (bytes32)` |
//! | [`StarkVerifyReserved`] | `0x…0a100000` | reserved (D27): every call reverts |
//!
//! All of them are pure: they read and write no storage, create no account (`HAS_CONTRACT_INFO`
//! is `false`) and move no balance. Cryptography goes through `ac-crypto` only (AGENT.md §6).
//! Each charges its benchmarked weight before doing any work.

use crate::weights::WeightInfo;
use ac_crypto::{PqPublicKey, PqSignature, SigAlg};
use ac_primitives::evm::{BLAKE3_ID, EVM_VERIFY_CONTEXT, PQ_VERIFY_ID, STARK_VERIFY_ID};
use alloc::vec::Vec;
use alloy_core::sol_types::SolValue;
use core::{marker::PhantomData, num::NonZero};
use frame_support::weights::Weight;
use pallet_revive::precompiles::{AddressMatcher, Error, Ext, Precompile};

alloy_core::sol! {
    /// `pq_verify`: verifies an ML-DSA signature made under `agentcoin/evm-verify/v1`.
    interface IPqVerify {
        function verify(uint8 alg, bytes calldata publicKey, bytes calldata message, bytes calldata signature) external view returns (bool);
    }

    /// `blake3`: the 32-byte BLAKE3 hash (plain mode).
    interface IBlake3 {
        function hash(bytes calldata data) external pure returns (bytes32);
    }

    /// `stark_verify`: reserved address (D27); every call reverts until the full version.
    interface IStarkVerify {
        function verify(bytes calldata data) external view returns (bool);
    }
}

/// The `NonZero` precompile number of a published id; ids are non-zero constants, which the
/// compile-time checks below enforce.
const fn fixed(id: u16) -> AddressMatcher {
    match NonZero::new(id) {
        Some(id) => AddressMatcher::Fixed(id),
        // Unreachable for the published ids (checked at compile time below).
        None => AddressMatcher::Fixed(NonZero::<u16>::MAX),
    }
}

const _: () = assert!(PQ_VERIFY_ID != 0 && BLAKE3_ID != 0 && STARK_VERIFY_ID != 0);

/// Weight `pq_verify` charges for a call with algorithm `alg` and a message of `message_len`
/// bytes. Unknown and reserved algorithms cost the smallest verification weight: they are
/// rejected without verifying.
pub fn pq_verify_weight<W: WeightInfo>(alg: u8, message_len: usize) -> Weight {
    let m = u32::try_from(message_len).unwrap_or(u32::MAX);
    match SigAlg::from_id(alg) {
        Ok(SigAlg::MlDsa44) => W::pq_verify_ml_dsa_44(m),
        Ok(SigAlg::MlDsa65) => W::pq_verify_ml_dsa_65(m),
        Ok(SigAlg::MlDsa87) => W::pq_verify_ml_dsa_87(m),
        _ => W::pq_verify_rejected(),
    }
}

/// Whether `signature` by `public_key` (raw bytes of algorithm `alg`) is valid for `message`
/// under the fixed context [`EVM_VERIFY_CONTEXT`]. Any failure (unknown or reserved AlgId, wrong
/// lengths, invalid signature) is `false`; there is no fallback to another algorithm.
pub fn pq_verify(alg: u8, public_key: &[u8], message: &[u8], signature: &[u8]) -> bool {
    let Ok(alg) = SigAlg::from_id(alg) else {
        return false;
    };
    let (Ok(public_key), Ok(signature)) = (
        PqPublicKey::new(alg, public_key),
        PqSignature::new(alg, signature),
    ) else {
        return false;
    };
    ac_crypto::sig::verify(&public_key, message, EVM_VERIFY_CONTEXT, &signature).is_ok()
}

/// The `pq_verify` precompile at `0x…0a010000`.
pub struct PqVerify<T>(PhantomData<T>);

impl<T: crate::Config> Precompile for PqVerify<T> {
    type T = T;
    type Interface = IPqVerify::IPqVerifyCalls;
    const MATCHER: AddressMatcher = fixed(PQ_VERIFY_ID);
    const HAS_CONTRACT_INFO: bool = false;

    fn call(
        _address: &[u8; 20],
        input: &Self::Interface,
        env: &mut impl Ext<T = Self::T>,
    ) -> Result<Vec<u8>, Error> {
        let IPqVerify::IPqVerifyCalls::verify(call) = input;
        env.charge(pq_verify_weight::<<T as crate::Config>::WeightInfo>(
            call.alg,
            call.message.len(),
        ))?;
        Ok(pq_verify_output(call))
    }
}

/// ABI-encoded result of a decoded `verify` call.
pub fn pq_verify_output(call: &IPqVerify::verifyCall) -> Vec<u8> {
    pq_verify(call.alg, &call.publicKey, &call.message, &call.signature).abi_encode()
}

/// The `blake3` precompile at `0x…0a020000`.
pub struct Blake3<T>(PhantomData<T>);

impl<T: crate::Config> Precompile for Blake3<T> {
    type T = T;
    type Interface = IBlake3::IBlake3Calls;
    const MATCHER: AddressMatcher = fixed(BLAKE3_ID);
    const HAS_CONTRACT_INFO: bool = false;

    fn call(
        _address: &[u8; 20],
        input: &Self::Interface,
        env: &mut impl Ext<T = Self::T>,
    ) -> Result<Vec<u8>, Error> {
        let IBlake3::IBlake3Calls::hash(call) = input;
        let n = u32::try_from(call.data.len()).unwrap_or(u32::MAX);
        env.charge(<T as crate::Config>::WeightInfo::blake3(n))?;
        Ok(blake3_output(&call.data))
    }
}

/// ABI-encoded `bytes32` BLAKE3 hash of `data`.
pub fn blake3_output(data: &[u8]) -> Vec<u8> {
    alloy_core::primitives::FixedBytes::<32>(ac_crypto::hash::blake3_256(data)).abi_encode()
}

/// Reserves `0x…0a100000` for `stark_verify` (D27): the address is occupied, every call reverts.
pub struct StarkVerifyReserved<T>(PhantomData<T>);

impl<T: crate::Config> Precompile for StarkVerifyReserved<T> {
    type T = T;
    type Interface = IStarkVerify::IStarkVerifyCalls;
    const MATCHER: AddressMatcher = fixed(STARK_VERIFY_ID);
    const HAS_CONTRACT_INFO: bool = false;

    fn call(
        _address: &[u8; 20],
        _input: &Self::Interface,
        env: &mut impl Ext<T = Self::T>,
    ) -> Result<Vec<u8>, Error> {
        env.charge(<T as crate::Config>::WeightInfo::pq_verify_rejected())?;
        Err(Error::Revert("stark_verify is reserved".into()))
    }
}

/// The AgentCoin precompiles, for `pallet_revive::Config::Precompiles`.
pub type PqPrecompiles<T> = (PqVerify<T>, Blake3<T>, StarkVerifyReserved<T>);
