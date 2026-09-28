// SPDX-License-Identifier: GPL-3.0-or-later
pragma solidity 0.8.28;

import {IFactory, IRouter, IToken, UniswapV2Flow} from "./UniswapV2.sol";

/// @notice Deploys the official Uniswap V2 and exercises it (design D14 of m4-evm).
/// @dev Foundry only simulates; `ac-wallet evm broadcast` signs and sends each simulation's
/// transactions with ML-DSA. On AgentCoin a native deployment advances the sender's nonce by two
/// (once for the transaction, once by pallet-revive), so a simulation predicts only its first
/// contract creation correctly: every stage below creates at most one contract, and later stages
/// take the earlier addresses as arguments.
///
///   forge script script/DeployV2.s.sol --sig "stageWeth()" --sender <evm address> --rpc-url <adapter>
///   forge script ... --sig "stageFactory()"
///   forge script ... --sig "stageRouter(address,address)" <factory> <weth>
///   forge script ... --sig "stageToken()"                  (twice: token A, token B)
///   forge script ... --sig "stageExercise(address,address,address,address)" <factory> <router> <a> <b>
contract DeployV2 is UniswapV2Flow {
    function stageWeth() external {
        vm.startBroadcast();
        deployWeth();
        vm.stopBroadcast();
    }

    function stageFactory() external {
        address owner = msg.sender;
        vm.startBroadcast();
        deployFactory(owner);
        vm.stopBroadcast();
    }

    function stageRouter(IFactory factory_, address weth_) external {
        vm.startBroadcast();
        deployRouter(factory_, weth_);
        vm.stopBroadcast();
    }

    function stageToken() external {
        vm.startBroadcast();
        deployToken();
        vm.stopBroadcast();
    }

    function stageExercise(IFactory factory_, IRouter router_, IToken tokenA_, IToken tokenB_) external {
        address owner = msg.sender;
        factory = factory_;
        router = router_;
        tokenA = tokenA_;
        tokenB = tokenB_;
        vm.startBroadcast();
        exercise(owner);
        vm.stopBroadcast();
    }
}
