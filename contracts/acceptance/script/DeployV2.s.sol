// SPDX-License-Identifier: GPL-3.0-or-later
pragma solidity 0.8.28;

import {UniswapV2Flow} from "./UniswapV2.sol";

/// @notice Deploys the official Uniswap V2 and exercises it (design D14 of m4-evm).
/// @dev Simulate with `forge script script/DeployV2.s.sol --sender <ac-wallet evm address>
/// --rpc-url <eth-RPC adapter>`, then send the transactions of the resulting run-latest.json with
/// `ac-wallet evm broadcast`: Foundry never signs, the wallet signs each one with ML-DSA.
contract DeployV2 is UniswapV2Flow {
    function run() external {
        address owner = msg.sender;
        vm.startBroadcast();
        runFlow(owner);
        vm.stopBroadcast();
    }
}
