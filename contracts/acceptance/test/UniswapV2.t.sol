// SPDX-License-Identifier: GPL-3.0-or-later
pragma solidity 0.8.28;

import {IPair, IToken, UniswapV2Flow} from "../script/UniswapV2.sol";

/// @dev The acceptance flow on Foundry's local EVM. Plain `require` checks (no forge-std).
contract UniswapV2Test is UniswapV2Flow {
    function setUp() public {
        runFlow(address(this));
    }

    // The factory creates the pair with CREATE2 at the address UniswapV2Library.pairFor computes:
    // keccak256(0xff ++ factory ++ keccak256(token0 ++ token1) ++ init-code hash).
    function test_pair_address_matches_pairFor() public view {
        (address token0, address token1) =
            address(tokenA) < address(tokenB) ? (address(tokenA), address(tokenB)) : (address(tokenB), address(tokenA));
        bytes32 initCodeHash = keccak256(vm.getCode("lib/v2-core/contracts/UniswapV2Pair.sol:UniswapV2Pair"));
        address expected = address(
            uint160(
                uint256(
                    keccak256(
                        abi.encodePacked(
                            hex"ff", address(factory), keccak256(abi.encodePacked(token0, token1)), initCodeHash
                        )
                    )
                )
            )
        );
        require(pair == expected, "pair address");
        require(factory.getPair(address(tokenA), address(tokenB)) == expected, "getPair");
        // The router (pairFor inside) finds the same pair: quoting reads its reserves.
        require(router.getAmountsOut(1 ether, path())[1] > 0, "router quote");
        require(router.factory() == address(factory) && router.WETH() == weth, "router wiring");
    }

    function test_liquidity_and_swap() public view {
        (uint112 r0, uint112 r1,) = IPair(pair).getReserves();
        (uint256 reserveA, uint256 reserveB) =
            IPair(pair).token0() == address(tokenA) ? (uint256(r0), uint256(r1)) : (uint256(r1), uint256(r0));
        require(reserveA == LIQUIDITY + SWAP_IN, "reserve in");
        // Constant product with the 0.3% fee: out = in*997*R_out / (R_in*1000 + in*997).
        uint256 out = SWAP_IN * 997 * LIQUIDITY / (LIQUIDITY * 1000 + SWAP_IN * 997);
        require(reserveB == LIQUIDITY - out, "reserve out");
        require(tokenB.balanceOf(address(this)) == SUPPLY - LIQUIDITY + out, "swap output");
        require(tokenA.balanceOf(address(this)) == SUPPLY - LIQUIDITY - SWAP_IN, "swap input");
        require(IToken(pair).balanceOf(address(this)) > 0, "liquidity tokens");
    }
}
