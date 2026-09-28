// SPDX-License-Identifier: GPL-3.0-or-later
pragma solidity 0.8.28;

// Minimal views of the official Uniswap V2 contracts (compiled from lib/ with solc 0.5.16 and
// 0.6.6), and of Foundry's cheatcodes, for the 0.8 script and tests. Deployment goes through the
// compiled artifacts, so this file never imports the upstream sources.

interface Vm {
    function getCode(string calldata artifactPath) external view returns (bytes memory);
    function startBroadcast() external;
    function stopBroadcast() external;
}

interface IToken {
    function approve(address spender, uint256 value) external returns (bool);
    function balanceOf(address owner) external view returns (uint256);
}

interface IFactory {
    function createPair(address tokenA, address tokenB) external returns (address pair);
    function getPair(address tokenA, address tokenB) external view returns (address pair);
}

interface IPair {
    function token0() external view returns (address);
    function getReserves() external view returns (uint112 reserve0, uint112 reserve1, uint32 timestamp);
}

interface IRouter {
    function factory() external view returns (address);
    function WETH() external view returns (address);
    function addLiquidity(
        address tokenA,
        address tokenB,
        uint256 amountADesired,
        uint256 amountBDesired,
        uint256 amountAMin,
        uint256 amountBMin,
        address to,
        uint256 deadline
    ) external returns (uint256 amountA, uint256 amountB, uint256 liquidity);
    function swapExactTokensForTokens(
        uint256 amountIn,
        uint256 amountOutMin,
        address[] calldata path,
        address to,
        uint256 deadline
    ) external returns (uint256[] memory amounts);
    function getAmountsOut(uint256 amountIn, address[] calldata path) external view returns (uint256[] memory);
}

/// @dev Deploys WETH9, UniswapV2Factory, UniswapV2Router02 and two test tokens, creates their pair
/// through the factory (CREATE2), adds liquidity and swaps: the acceptance flow of design D14.
abstract contract UniswapV2Flow {
    Vm internal constant vm = Vm(address(uint160(uint256(keccak256("hevm cheat code")))));

    uint256 internal constant SUPPLY = 1_000_000 ether;
    uint256 internal constant LIQUIDITY = 10_000 ether;
    uint256 internal constant SWAP_IN = 100 ether;

    address public weth;
    IFactory public factory;
    IRouter public router;
    IToken public tokenA;
    IToken public tokenB;
    address public pair;

    function deploy(string memory artifact, bytes memory args) internal returns (address addr) {
        bytes memory code = abi.encodePacked(vm.getCode(artifact), args);
        assembly {
            addr := create(0, add(code, 0x20), mload(code))
        }
        require(addr != address(0), artifact);
    }

    function path() internal view returns (address[] memory route) {
        route = new address[](2);
        route[0] = address(tokenA);
        route[1] = address(tokenB);
    }

    function deployWeth() internal returns (address) {
        return deploy("lib/v2-periphery/contracts/test/WETH9.sol:WETH9", "");
    }

    function deployFactory(address feeToSetter) internal returns (IFactory) {
        return IFactory(deploy("lib/v2-core/contracts/UniswapV2Factory.sol:UniswapV2Factory", abi.encode(feeToSetter)));
    }

    function deployRouter(IFactory factory_, address weth_) internal returns (IRouter) {
        return IRouter(
            deploy(
                "lib/v2-periphery/contracts/UniswapV2Router02.sol:UniswapV2Router02", abi.encode(address(factory_), weth_)
            )
        );
    }

    function deployToken() internal returns (IToken) {
        return IToken(deploy("lib/v2-core/contracts/test/ERC20.sol:ERC20", abi.encode(SUPPLY)));
    }

    /// @dev Creates the pair through the factory (CREATE2 inside the call), adds liquidity and swaps;
    /// `owner` receives the liquidity and the swap output. Calls only, no contract creation.
    function exercise(address owner) internal {
        pair = factory.createPair(address(tokenA), address(tokenB));
        tokenA.approve(address(router), type(uint256).max);
        tokenB.approve(address(router), type(uint256).max);
        router.addLiquidity(
            address(tokenA), address(tokenB), LIQUIDITY, LIQUIDITY, 0, 0, owner, type(uint256).max
        );
        router.swapExactTokensForTokens(SWAP_IN, 0, path(), owner, type(uint256).max);
    }

    /// @dev The whole flow in one EVM (Foundry tests).
    function runFlow(address owner) internal {
        weth = deployWeth();
        factory = deployFactory(owner);
        router = deployRouter(factory, weth);
        tokenA = deployToken();
        tokenB = deployToken();
        exercise(owner);
    }
}
