// SPDX-License-Identifier: MIT OR Apache-2.0
pragma solidity 0.8.28;

/// @title AgentCoin post-quantum precompiles (m4-evm, spec evm/precompiles).
/// @notice The addresses never change once published.

/// @dev ML-DSA verification under the fixed context `agentcoin/evm-verify/v1`. `alg` is the
/// AgentCoin AlgId (1 = ML-DSA-44, 2 = ML-DSA-65, 3 = ML-DSA-87); `publicKey` and `signature`
/// are the raw FIPS 204 encodings. Returns false for anything that does not verify, including
/// unknown algorithms and wrong lengths. Sign with `ac-wallet evm sign-message`.
interface IPqVerify {
    function verify(uint8 alg, bytes calldata publicKey, bytes calldata message, bytes calldata signature)
        external
        view
        returns (bool);
}

/// @dev BLAKE3-256 of `data`.
interface IBlake3 {
    function hash(bytes calldata data) external pure returns (bytes32);
}

/// @dev Poseidon2-256 of `data` over Goldilocks (Plonky3's width-12 instance; the byte encoding
/// is in the AgentCoin `ac-crypto` README).
interface IPoseidon2 {
    function hash(bytes calldata data) external pure returns (bytes32);
}

library PqPrecompiles {
    IPqVerify internal constant PQ_VERIFY = IPqVerify(0x000000000000000000000000000000000a010000);
    IBlake3 internal constant BLAKE3 = IBlake3(0x000000000000000000000000000000000A020000);
    IPoseidon2 internal constant POSEIDON2 = IPoseidon2(0x000000000000000000000000000000000a030000);
    /// @dev Reserved for STARK verification; every call reverts for now.
    address internal constant STARK_VERIFY = 0x000000000000000000000000000000000A100000;
}
