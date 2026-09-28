> 🌐 [English](README.md) | **简体中文**

# ac-eth-rpc

AgentCoin 以太坊 JSON-RPC 适配器（m4-evm，规范 `evm/eth-rpc`）。它让 Foundry 等以太坊工具读取合约状态、区块、回执与日志，并转发已由 AgentCoin 钱包用 ML-DSA 签名的合约交易。它只通过节点的公开 JSON-RPC 与链交互，不持有任何密钥，也不代任何人签名。

许可证：`GPL-3.0-or-later`（服务程序，决策 D47）。

## 运行

```sh
ac-eth-rpc --node-url http://127.0.0.1:9944 --listen 127.0.0.1:8545
cast chain-id --rpc-url http://127.0.0.1:8545      # 4403
```

| 参数 | 默认值 | 含义 |
|---|---|---|
| `--node-url` | `http://127.0.0.1:9944` | 节点的 JSON-RPC |
| `--listen` | `127.0.0.1:8545` | HTTP 与 WebSocket 地址（默认只监听本机回环） |
| `--index-depth` | `10000` | 启动前纳入交易与回执索引的区块数 |
| `--max-log-range` | `1000` | 单次 `eth_getLogs` 的最大区块跨度 |
| `--max-connections` | `100` | 同时连接数 |
| `--log-level` | `info` | `error`、`warn`、`info` 或 `debug` |

## 方法

| 方法 | 返回 |
|---|---|
| `web3_clientVersion`、`net_version`、`eth_chainId` | 适配器版本；链 ID 4403（`0x1133`） |
| `eth_syncing`、`eth_accounts` | 恒为 `false`；恒为 `[]` |
| `eth_blockNumber` | 最佳区块 |
| `eth_gasPrice`、`eth_maxPriorityFeePerGas`、`eth_feeHistory` | 一个 gas 单位（10,000 ps 权重）的手续费；优先费为 0；每块返回同一基础费 |
| `eth_getBalance`、`eth_getTransactionCount`、`eth_getCode`、`eth_getStorageAt` | 该区块的合约状态 |
| `eth_call`、`eth_estimateGas` | 在该区块状态上模拟执行，调用者任意；回滚返回错误码 3 与回滚数据 |
| `eth_getBlockByNumber`、`eth_getBlockByHash` | 区块的以太坊视图；`transactions` 列出其中的合约交易 |
| `eth_getTransactionByHash`、`eth_getTransactionReceipt` | 索引内的合约交易，否则为 `null` |
| `eth_getLogs` | 按区块范围或区块哈希、地址（单个或多个）与主题过滤的合约事件 |
| `eth_sendRawTransaction` | 转发 ML-DSA 签名的 AgentCoin 合约交易 |
| `eth_sendTransaction`、`eth_sign*`、`personal_*` | 拒绝，并指引使用 `ac-wallet evm` |

其他方法返回 `-32601`（方法不存在）。

## 区块标签

`latest` 与 `pending` 指最佳区块；`safe` 与 `finalized` 指 AC-BFT 最新的最终确定区块；`earliest` 指第 0 块。区块的 `hash` 等于合约对该块执行 `blockhash` 得到的值；用这个哈希或原生区块哈希都能查到该块。

## 交易

只转发调用为合约调用或 EVM 部署、且由 ML-DSA 签名的 AgentCoin 原生交易；返回的哈希就是节点的（BLAKE3）交易哈希。任何类型的 RLP 以太坊交易、其他调用和无法解码的字节都在到达节点前被拒绝。合约交易用 `ac-wallet evm deploy|send` 构造并签名，或用 `ac-wallet evm raw` 离线签名后把字节提交到这里。

交易对象中的 `v`、`r`、`s` 为零：授权来自 ML-DSA 签名。回执的 `status` 取自 `ExtrinsicSuccess`/`ExtrinsicFailed`，`contractAddress` 取自 `Instantiated`，日志取自 `ContractEmitted`；`gasUsed = ceil(手续费 / gasPrice)`，`effectiveGasPrice = gasPrice`，因此 `gasUsed × effectiveGasPrice` 比实付手续费多出的部分小于一个 gas 单位的价格。

```rust
use ac_eth_rpc::chain::{REF_TIME_PER_GAS, Refusal, decode_contract_tx, gas_of};

// EIP-1559 交易（类型字节加 RLP 列表）不是 AgentCoin 交易。
let eip1559 = [0x02, 0xf8, 0x50, 0x82, 0x11, 0x33];
assert_eq!(decode_contract_tx(&eip1559), Err(Refusal::NotAgentCoin));

// gas 数值由权重换算，向上取整。
assert_eq!(gas_of(REF_TIME_PER_GAS + 1), 2);
```

## 索引

适配器在内存中保存从启动前 `--index-depth` 块开始的全部区块：原生与以太坊区块哈希，以及每笔合约交易的位置。它跟随最佳链，链重组时回滚，重启后全部重建（不持久化）。索引范围外的交易返回 `null`。

## 错误码

| 错误码 | 场合 |
|---|---|
| `3` | `eth_call` 或 `eth_estimateGas` 回滚；`data` 为回滚数据 |
| `-32000` | 拒绝的原始交易或签名类方法 |
| `-32005` | `eth_getLogs` 的区块跨度超过 `--max-log-range` |
| `-32601` | 不支持的方法 |
| `-32602` | 参数无效或区块不存在 |
| `-32603` | 节点无法应答 |

## 日志

只记录方法名、耗时与错误码，从不记录参数或返回内容。

## 非目标

- 以太坊签名（secp256k1）交易、`eth_sendTransaction` 与节点代签。
- 过滤器与订阅（`eth_newFilter`、`eth_subscribe`）、追踪与调试方法。
- 持久化索引，以及超出节点自身状态裁剪范围的归档查询。
