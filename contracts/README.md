> 🌐 **English** | [简体中文](README.zh-CN.md)

# AgentCoin contracts

Solidity for AgentCoin's EVM (`pallet-revive`, chain ID 4403), in two Foundry projects with
different licences (decision D47, checked by `scripts/check-license-boundary.sh`).

| Project | Licence zone | Contents |
|---|---|---|
| `contracts/` | permissive, `MIT OR Apache-2.0` | examples: `src/ERC20.sol`, `src/IPqPrecompiles.sol`, `src/PqVerifyDemo.sol` and their tests |
| `contracts/acceptance/` | GPL, `GPL-3.0-or-later` | the official Uniswap V2 and the acceptance script and tests |

Every `.sol` file in the permissive project carries an `MIT OR Apache-2.0` SPDX identifier and
imports nothing from `contracts/acceptance/`.

## Examples

- `ERC20.sol`: a minimal ERC-20; the initial supply goes to the deployer.
- `IPqPrecompiles.sol`: `IPqVerify` (`0x…0A010000`, ML-DSA verification under
  `agentcoin/evm-verify/v1`), `IBlake3` (`0x…0A020000`) and the reserved `STARK_VERIFY`
  address (`0x…0A100000`).
- `PqVerifyDemo.sol`: checks a signature made with `ac-wallet evm sign-message`, hashes with
  BLAKE3, and records messages whose signature verifies.

```bash
cd contracts
forge build
forge test          # solc 0.8.28, EVM version cancun
```

Deploy and call with the wallet (it signs with ML-DSA; Foundry never signs here), read with
`cast` through `ac-eth-rpc`; see the [ac-wallet README](../clients/wallet-cli/README.md).

## Acceptance project: official Uniswap V2

`contracts/acceptance/lib/` holds `Uniswap/v2-core` (tag v1.0.1), `Uniswap/v2-periphery` and
`@uniswap/lib` 4.0.1-alpha, copied byte for byte by `scripts/vendor-uniswap-v2.sh`; `lib/SOURCE.md`
records the URLs, versions and every file's SHA-256, and `--check` reproduces them. They keep
their upstream licence (GPL-3.0-or-later) and compile with the upstream settings: solc 0.5.16
(v2-core) and 0.6.6 (v2-periphery), optimizer 999999 runs, EVM version istanbul.

The only change to upstream is the pair init-code hash in `UniswapV2Library.pairFor`: the
metadata hash embedded in `UniswapV2Pair`'s bytecode depends on source paths, so the constant is
the hash of the init code as this project compiles it (recorded in `SOURCE.md`).

```bash
cd contracts/acceptance
forge test          # deploy, createPair (CREATE2 at pairFor's address), add liquidity, swap
```

`script/DeployV2.s.sol` runs the same flow; simulate it with
`forge script script/DeployV2.s.sol --sender <ac-wallet evm address> --rpc-url <ac-eth-rpc>` and
send the result with `ac-wallet evm broadcast`.
