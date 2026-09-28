> 🌐 [English](README.md) | **简体中文**

# AgentCoin 合约

面向 AgentCoin EVM（`pallet-revive`，链 ID 4403）的 Solidity 代码，分为两个许可证不同的 Foundry 工程（决策 D47，由 `scripts/check-license-boundary.sh` 检查）。

| 工程 | 许可证区 | 内容 |
|---|---|---|
| `contracts/` | 宽松区，`MIT OR Apache-2.0` | 示例：`src/ERC20.sol`、`src/IPqPrecompiles.sol`、`src/PqVerifyDemo.sol` 及其测试 |
| `contracts/acceptance/` | GPL 区，`GPL-3.0-or-later` | 官方 Uniswap V2 以及验收脚本和测试 |

宽松工程中的每个 `.sol` 文件都带 `MIT OR Apache-2.0` 的 SPDX 标识，且不导入 `contracts/acceptance/` 中的任何文件。

## 示例

- `ERC20.sol`：最小的 ERC-20，初始供应量归部署者。
- `IPqPrecompiles.sol`：`IPqVerify`（`0x…0A010000`，在 `agentcoin/evm-verify/v1` 下验证 ML-DSA 签名）、`IBlake3`（`0x…0A020000`），以及预留的 `STARK_VERIFY` 地址（`0x…0A100000`）。
- `PqVerifyDemo.sol`：检查用 `ac-wallet evm sign-message` 生成的签名，计算 BLAKE3 哈希，并记录签名验证通过的消息。

```bash
cd contracts
forge build
forge test          # solc 0.8.28，EVM 版本 cancun
```

用钱包部署和调用（钱包用 ML-DSA 签名；这里 Foundry 从不签名），用 `cast` 经 `ac-eth-rpc` 读取；见 [ac-wallet README](../clients/wallet-cli/README.zh-CN.md)。

## 验收工程：官方 Uniswap V2

`contracts/acceptance/lib/` 中是由 `scripts/vendor-uniswap-v2.sh` 逐字节复制的 `Uniswap/v2-core`（tag v1.0.1）、`Uniswap/v2-periphery` 和 `@uniswap/lib` 4.0.1-alpha；`lib/SOURCE.md` 记录了来源 URL、版本和每个文件的 SHA-256，`--check` 可以复现这些文件。它们保留上游许可证（GPL-3.0-or-later），并按上游设置编译：solc 0.5.16（v2-core）和 0.6.6（v2-periphery），优化器 999999 次，EVM 版本 istanbul。

对上游唯一的改动是 `UniswapV2Library.pairFor` 中的配对合约初始化代码哈希：`UniswapV2Pair` 字节码中嵌入的元数据哈希与源文件路径有关，因此该常量取本工程编译出的初始化代码的哈希（记录在 `SOURCE.md` 中）。

```bash
cd contracts/acceptance
forge test          # 部署、createPair（以 CREATE2 创建在 pairFor 计算的地址上）、添加流动性、兑换
```

`script/DeployV2.s.sol` 分阶段在链上执行同样的流程：Foundry 模拟每个阶段（`forge script script/DeployV2.s.sol --sig "stageWeth()" --sender <ac-wallet evm 地址> --rpc-url <ac-eth-rpc>`），再由 `ac-wallet evm broadcast` 签名并发送。原生部署会使发送者的 nonce 增加 2（交易本身一次，`pallet-revive` 一次），因此一次模拟只能正确预测其中第一笔合约创建：每个阶段至多创建一个合约（`stageWeth`、`stageFactory`、`stageRouter`、两次 `stageToken`），后续阶段以参数接收先前的地址；最后 `stageExercise` 创建配对、添加流动性并兑换。EVM 端到端测试正是按这些步骤执行。
