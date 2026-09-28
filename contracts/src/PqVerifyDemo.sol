// SPDX-License-Identifier: MIT OR Apache-2.0
pragma solidity 0.8.28;

import {PqPrecompiles} from "./IPqPrecompiles.sol";

/// @title Uses the post-quantum precompiles from a contract.
contract PqVerifyDemo {
    /// @notice BLAKE3 digests of messages whose signature was accepted, by signer public key hash.
    mapping(bytes32 => bytes32) public lastAccepted;

    event Accepted(bytes32 indexed signer, bytes32 digest);

    /// @notice Whether `signature` is a valid ML-DSA signature of `message` by `publicKey`.
    function check(uint8 alg, bytes calldata publicKey, bytes calldata message, bytes calldata signature)
        external
        view
        returns (bool)
    {
        return PqPrecompiles.PQ_VERIFY.verify(alg, publicKey, message, signature);
    }

    /// @notice BLAKE3-256 of `data`.
    function digest(bytes calldata data) external pure returns (bytes32) {
        return PqPrecompiles.BLAKE3.hash(data);
    }

    /// @notice Records `message` for the signer if the signature verifies; reverts otherwise.
    function accept(uint8 alg, bytes calldata publicKey, bytes calldata message, bytes calldata signature)
        external
    {
        require(PqPrecompiles.PQ_VERIFY.verify(alg, publicKey, message, signature), "invalid signature");
        bytes32 signer = PqPrecompiles.BLAKE3.hash(publicKey);
        bytes32 messageDigest = PqPrecompiles.BLAKE3.hash(message);
        lastAccepted[signer] = messageDigest;
        emit Accepted(signer, messageDigest);
    }
}
