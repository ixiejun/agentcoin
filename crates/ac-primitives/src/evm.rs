//! EVM integration constants and the native contract-transaction classifier (m4-evm).
//!
//! The constants are published values: the chain ID, the PQ precompile addresses and the
//! `pq_verify` signing context never change once released (spec `evm/precompiles`).
//!
//! [`classify_call`] decides whether the SCALE-encoded call of a native transaction deploys or
//! calls an EVM contract. The wallet uses it before signing and the eth-RPC adapter uses it to
//! refuse every other raw transaction; it works on bytes so that neither needs to match on the
//! runtime's `RuntimeCall` type, and a runtime test checks it against the real encoding.

use alloc::vec::Vec;
use parity_scale_codec::{Compact, Decode, Encode};
use scale_info::TypeInfo;

/// EVM chain ID of the α networks (dev, local and testnet), returned by `block.chainid` and
/// `eth_chainId`. Replay protection comes from the genesis hash in the transaction signature
/// (D36); the chain ID only identifies the network to Ethereum tooling.
pub const EVM_CHAIN_ID: u64 = 4403;

/// Signing context of the `pq_verify` precompile. Wallets sign messages meant for contracts
/// under this context only, so no protocol signature (e.g. `agentcoin/tx/v1`) verifies inside a
/// contract.
pub const EVM_VERIFY_CONTEXT: &[u8] = b"agentcoin/evm-verify/v1";

/// Precompile number of `pq_verify` (`pallet-revive` external matcher `Fixed(0x0A01)`).
pub const PQ_VERIFY_ID: u16 = 0x0A01;
/// Precompile number of `blake3`.
pub const BLAKE3_ID: u16 = 0x0A02;
/// Precompile number of `poseidon2`.
pub const POSEIDON2_ID: u16 = 0x0A03;
/// Precompile number reserved for `stark_verify` (D27); every call reverts in the MVP.
pub const STARK_VERIFY_ID: u16 = 0x0A10;

/// Address of the `pq_verify` precompile.
pub const PQ_VERIFY_ADDRESS: [u8; 20] = precompile_address(PQ_VERIFY_ID);
/// Address of the `blake3` precompile.
pub const BLAKE3_ADDRESS: [u8; 20] = precompile_address(BLAKE3_ID);
/// Address of the `poseidon2` precompile.
pub const POSEIDON2_ADDRESS: [u8; 20] = precompile_address(POSEIDON2_ID);
/// Address reserved for the `stark_verify` precompile.
pub const STARK_VERIFY_ADDRESS: [u8; 20] = precompile_address(STARK_VERIFY_ID);

/// The address layout of `pallet-revive` external precompiles: sixteen zero bytes, the
/// big-endian precompile number, then two zero bytes.
pub const fn precompile_address(id: u16) -> [u8; 20] {
    let be = id.to_be_bytes();
    let mut address = [0u8; 20];
    address[16] = be[0];
    address[17] = be[1];
    address
}

/// Pallet index of `pallet-revive` (`Revive`) in the AgentCoin runtime.
pub const REVIVE_PALLET_INDEX: u8 = 14;
/// Call index of `Revive::call`.
pub const REVIVE_CALL_INDEX: u8 = 1;
/// Call index of `Revive::instantiate_with_code`.
pub const REVIVE_INSTANTIATE_WITH_CODE_INDEX: u8 = 3;

/// Leading bytes of a PolkaVM program blob; such code is never accepted (EVM bytecode only).
pub const POLKAVM_MAGIC: [u8; 4] = *b"PVM\0";

/// A weight limit as encoded in `pallet-revive` calls (`sp_weights::Weight`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Encode, Decode, TypeInfo)]
pub struct WeightLimit {
    /// Computation time, in picoseconds.
    #[codec(compact)]
    pub ref_time: u64,
    /// Proof size, in bytes.
    #[codec(compact)]
    pub proof_size: u64,
}

/// A native transaction that calls an existing contract (`Revive::call`).
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct ContractCall {
    /// Contract (or account) address.
    pub dest: [u8; 20],
    /// Value transferred, in the smallest ATC unit (1 wei).
    pub value: u128,
    /// Weight limit of the execution.
    pub weight_limit: WeightLimit,
    /// Storage deposit limit, in the smallest ATC unit.
    pub storage_deposit_limit: u128,
    /// ABI-encoded calldata.
    pub data: Vec<u8>,
}

/// A native transaction that deploys EVM init code (`Revive::instantiate_with_code`).
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct ContractDeploy {
    /// Value transferred to the new contract, in the smallest ATC unit.
    pub value: u128,
    /// Weight limit of the execution.
    pub weight_limit: WeightLimit,
    /// Storage deposit limit, in the smallest ATC unit.
    pub storage_deposit_limit: u128,
    /// EVM init code, constructor arguments appended.
    pub code: Vec<u8>,
    /// Must be empty for EVM code (constructor arguments travel in `code`).
    pub data: Vec<u8>,
    /// Optional CREATE2-style salt.
    pub salt: Option<[u8; 32]>,
}

/// What a native transaction does with the EVM, if anything.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ContractTransaction {
    /// Calls a contract.
    Call(ContractCall),
    /// Deploys EVM init code.
    Deploy(ContractDeploy),
}

/// Why a call is not an acceptable contract transaction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ClassifyError {
    /// The call belongs to another pallet or is another `Revive` call.
    NotContractCall,
    /// The deployment carries a PolkaVM program; only EVM bytecode is accepted.
    PolkaVmCode,
    /// The arguments do not decode, or bytes are left over.
    Malformed,
}

impl core::fmt::Display for ClassifyError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match self {
            Self::NotContractCall => "not a contract call or EVM deployment",
            Self::PolkaVmCode => "PolkaVM code is not accepted; only EVM bytecode",
            Self::Malformed => "malformed contract call arguments",
        })
    }
}

impl core::error::Error for ClassifyError {}

/// Classifies the SCALE encoding of a runtime call (pallet index, call index, arguments).
///
/// # Errors
///
/// [`ClassifyError::NotContractCall`] for any call other than `Revive::call` or
/// `Revive::instantiate_with_code`, [`ClassifyError::PolkaVmCode`] for a PolkaVM deployment and
/// [`ClassifyError::Malformed`] when the arguments do not decode exactly.
pub fn classify_call(encoded_call: &[u8]) -> Result<ContractTransaction, ClassifyError> {
    let (&pallet, rest) = encoded_call
        .split_first()
        .ok_or(ClassifyError::NotContractCall)?;
    let (&call, mut args) = rest.split_first().ok_or(ClassifyError::NotContractCall)?;
    if pallet != REVIVE_PALLET_INDEX {
        return Err(ClassifyError::NotContractCall);
    }
    let decoded = match call {
        REVIVE_CALL_INDEX => ContractTransaction::Call(ContractCall {
            dest: decode(&mut args)?,
            value: decode::<Compact<u128>>(&mut args)?.0,
            weight_limit: decode(&mut args)?,
            storage_deposit_limit: decode::<Compact<u128>>(&mut args)?.0,
            data: decode(&mut args)?,
        }),
        REVIVE_INSTANTIATE_WITH_CODE_INDEX => {
            let deploy = ContractDeploy {
                value: decode::<Compact<u128>>(&mut args)?.0,
                weight_limit: decode(&mut args)?,
                storage_deposit_limit: decode::<Compact<u128>>(&mut args)?.0,
                code: decode(&mut args)?,
                data: decode(&mut args)?,
                salt: decode(&mut args)?,
            };
            if deploy.code.starts_with(&POLKAVM_MAGIC) {
                return Err(ClassifyError::PolkaVmCode);
            }
            ContractTransaction::Deploy(deploy)
        }
        _ => return Err(ClassifyError::NotContractCall),
    };
    if !args.is_empty() {
        return Err(ClassifyError::Malformed);
    }
    Ok(decoded)
}

fn decode<T: Decode>(input: &mut &[u8]) -> Result<T, ClassifyError> {
    T::decode(input).map_err(|_| ClassifyError::Malformed)
}

impl ContractCall {
    /// Builds a contract call.
    pub fn new(
        dest: [u8; 20],
        value: u128,
        weight_limit: WeightLimit,
        storage_deposit_limit: u128,
        data: Vec<u8>,
    ) -> Self {
        Self {
            dest,
            value,
            weight_limit,
            storage_deposit_limit,
            data,
        }
    }

    /// The SCALE encoding of this call as a runtime call (inverse of [`classify_call`]).
    pub fn encode_call(&self) -> Vec<u8> {
        let mut out = alloc::vec![REVIVE_PALLET_INDEX, REVIVE_CALL_INDEX];
        self.dest.encode_to(&mut out);
        Compact(self.value).encode_to(&mut out);
        self.weight_limit.encode_to(&mut out);
        Compact(self.storage_deposit_limit).encode_to(&mut out);
        self.data.encode_to(&mut out);
        out
    }
}

impl ContractDeploy {
    /// Builds an EVM deployment; `code` is the init code with constructor arguments appended.
    pub fn new(
        value: u128,
        weight_limit: WeightLimit,
        storage_deposit_limit: u128,
        code: Vec<u8>,
        salt: Option<[u8; 32]>,
    ) -> Self {
        Self {
            value,
            weight_limit,
            storage_deposit_limit,
            code,
            data: Vec::new(),
            salt,
        }
    }

    /// The SCALE encoding of this deployment as a runtime call (inverse of [`classify_call`]).
    pub fn encode_call(&self) -> Vec<u8> {
        let mut out = alloc::vec![REVIVE_PALLET_INDEX, REVIVE_INSTANTIATE_WITH_CODE_INDEX];
        Compact(self.value).encode_to(&mut out);
        self.weight_limit.encode_to(&mut out);
        Compact(self.storage_deposit_limit).encode_to(&mut out);
        self.code.encode_to(&mut out);
        self.data.encode_to(&mut out);
        self.salt.encode_to(&mut out);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex20(s: &str) -> [u8; 20] {
        let bytes = hex::decode(s.trim_start_matches("0x")).unwrap();
        bytes.try_into().unwrap()
    }

    // Spec evm/precompiles, Requirement "预编译地址固定", Scenario "地址回归".
    #[test]
    fn precompile_addresses_match_the_spec() {
        assert_eq!(
            PQ_VERIFY_ADDRESS,
            hex20("0x000000000000000000000000000000000a010000")
        );
        assert_eq!(
            BLAKE3_ADDRESS,
            hex20("0x000000000000000000000000000000000a020000")
        );
        assert_eq!(
            POSEIDON2_ADDRESS,
            hex20("0x000000000000000000000000000000000a030000")
        );
        assert_eq!(
            STARK_VERIFY_ADDRESS,
            hex20("0x000000000000000000000000000000000a100000")
        );
    }

    #[test]
    fn published_constants() {
        assert_eq!(EVM_CHAIN_ID, 4403);
        assert_eq!(EVM_VERIFY_CONTEXT, b"agentcoin/evm-verify/v1");
        assert_eq!(REVIVE_PALLET_INDEX, 14);
    }

    fn weight() -> WeightLimit {
        WeightLimit {
            ref_time: 1_000_000_000,
            proof_size: 65_536,
        }
    }

    #[test]
    fn contract_call_round_trips() {
        let call = ContractCall::new([7; 20], 5, weight(), 1_000, alloc::vec![1, 2, 3]);
        assert_eq!(
            classify_call(&call.encode_call()),
            Ok(ContractTransaction::Call(call))
        );
    }

    #[test]
    fn evm_deploy_round_trips() {
        let deploy =
            ContractDeploy::new(0, weight(), 1_000, alloc::vec![0x60, 0x80], Some([9; 32]));
        assert_eq!(
            classify_call(&deploy.encode_call()),
            Ok(ContractTransaction::Deploy(deploy))
        );
    }

    #[test]
    fn polkavm_deploy_is_rejected() {
        let mut code = POLKAVM_MAGIC.to_vec();
        code.extend_from_slice(&[1, 2, 3]);
        let deploy = ContractDeploy::new(0, weight(), 0, code, None);
        assert_eq!(
            classify_call(&deploy.encode_call()),
            Err(ClassifyError::PolkaVmCode)
        );
    }

    #[test]
    fn other_calls_are_rejected() {
        // A balances transfer (pallet 3 in the runtime) and other Revive calls.
        assert_eq!(
            classify_call(&[3, 0, 1, 2, 3]),
            Err(ClassifyError::NotContractCall)
        );
        for index in [0u8, 2, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13] {
            assert_eq!(
                classify_call(&[REVIVE_PALLET_INDEX, index, 0]),
                Err(ClassifyError::NotContractCall),
                "Revive call {index}"
            );
        }
        assert_eq!(classify_call(&[]), Err(ClassifyError::NotContractCall));
    }

    #[test]
    fn malformed_arguments_are_rejected() {
        let mut encoded =
            ContractCall::new([7; 20], 5, weight(), 1_000, alloc::vec![1]).encode_call();
        encoded.push(0);
        assert_eq!(classify_call(&encoded), Err(ClassifyError::Malformed));
        assert_eq!(
            classify_call(&[REVIVE_PALLET_INDEX, REVIVE_CALL_INDEX, 1]),
            Err(ClassifyError::Malformed)
        );
    }
}
