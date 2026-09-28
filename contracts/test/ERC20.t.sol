// SPDX-License-Identifier: MIT OR Apache-2.0
pragma solidity 0.8.28;

import {ERC20} from "../src/ERC20.sol";
import {IBlake3, IPoseidon2, IPqVerify, PqPrecompiles} from "../src/IPqPrecompiles.sol";

/// @dev A second account for transferFrom tests.
contract Spender {
    function pull(ERC20 token, address from, address to, uint256 value) external returns (bool) {
        return token.transferFrom(from, to, value);
    }
}

/// @dev Plain Solidity checks (no forge-std): a failing `require` fails the test.
contract ERC20Test {
    ERC20 token;
    Spender spender;
    address constant BOB = address(0xB0B);

    function setUp() public {
        token = new ERC20("Token", "TKN", 1000 ether);
        spender = new Spender();
    }

    function test_constructor_mints_to_the_deployer() public view {
        require(token.totalSupply() == 1000 ether);
        require(token.balanceOf(address(this)) == 1000 ether);
        require(keccak256(bytes(token.name())) == keccak256("Token"));
        require(token.decimals() == 18);
    }

    function test_transfer() public {
        require(token.transfer(BOB, 1 ether));
        require(token.balanceOf(BOB) == 1 ether);
        require(token.balanceOf(address(this)) == 999 ether);
    }

    function test_transfer_above_balance_reverts() public {
        (bool ok, bytes memory data) =
            address(token).call(abi.encodeCall(ERC20.transfer, (BOB, 1001 ether)));
        require(!ok);
        require(keccak256(data) == keccak256(abi.encodeWithSignature("Error(string)", "ERC20: insufficient balance")));
    }

    function test_approve_and_transfer_from() public {
        require(token.approve(address(spender), 5 ether));
        require(token.allowance(address(this), address(spender)) == 5 ether);
        require(spender.pull(token, address(this), BOB, 2 ether));
        require(token.balanceOf(BOB) == 2 ether);
        require(token.allowance(address(this), address(spender)) == 3 ether);
        (bool ok,) = address(spender).call(abi.encodeCall(Spender.pull, (token, address(this), BOB, 4 ether)));
        require(!ok);
    }

    function test_unlimited_allowance_is_not_spent() public {
        require(token.approve(address(spender), type(uint256).max));
        require(spender.pull(token, address(this), BOB, 1 ether));
        require(token.allowance(address(this), address(spender)) == type(uint256).max);
    }

    // The precompile interfaces encode as the chain's precompiles decode (selectors fixed by
    // the Solidity signatures in spec evm/precompiles).
    function test_precompile_abi() public pure {
        bytes memory call = abi.encodeCall(IPqVerify.verify, (uint8(1), hex"01", hex"02", hex"03"));
        bytes memory expected = abi.encodeWithSelector(
            bytes4(keccak256("verify(uint8,bytes,bytes,bytes)")), uint8(1), hex"01", hex"02", hex"03"
        );
        require(keccak256(call) == keccak256(expected));
        require(IBlake3.hash.selector == bytes4(keccak256("hash(bytes)")));
        require(IPoseidon2.hash.selector == bytes4(keccak256("hash(bytes)")));
        require(address(PqPrecompiles.PQ_VERIFY) == address(uint160(0x0A01) << 16));
        require(address(PqPrecompiles.BLAKE3) == address(uint160(0x0A02) << 16));
        require(address(PqPrecompiles.POSEIDON2) == address(uint160(0x0A03) << 16));
        require(PqPrecompiles.STARK_VERIFY == address(uint160(0x0A10) << 16));
    }
}
