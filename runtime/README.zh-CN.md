> 🌐 [English](README.md) | **简体中文**

# ac-runtime

AgentCoin 的 WASM runtime（M1：抗量子链）。

| 序号 | 模块 | 说明 |
|---|---|---|
| 0 | `System` | `Hashing = Blake3Hasher`：区块哈希、外部交易根和状态根都是 BLAKE3-256（D35） |
| 1 | `Timestamp` | 1 秒出块 |
| 2 | `AuraPq` | 来自创世配置的 ML-DSA-65 授权节点集合 |
| 3 | `Balances` | ATC，18 位小数，存在性押金 0.001 ATC；模块名是已发布的知名存储键（宪法第 1 层） |
| 4 | `TransactionPayment` | 权重费 + 长度费；M1 中手续费与小费全部销毁 |
| 5 | `PqAccounts` | 公钥登记表、`rotate_key` |

交易采用 v5 `General` 形式，首个扩展为 `PqAuthorize`（D36）；旧式 `Signed` 交易无法解码。`transaction` 模块为钱包和测试构造签名交易：`authorized_extensions`、`implicit_from`（由链上事实得到隐式数据）、`payload`（以 `agentcoin/tx/v1` 签名的 32 字节载荷）和 `assemble`。

创世预设 `development`（授权节点 alice）和 `local_testnet`（alice、bob、charlie）为公开的开发账户 alice、bob、charlie、dave 分配余额；其他任何预设都不得分配 ATC。

## Feature

| Feature | 默认 | 用途 |
|---|---|---|
| `std` | 是 | 原生 runtime 与 WASM 构建器。 |
| `runtime-benchmarks` | 否 | 所含模块的基准测试。 |

## 示例

示例代码见英文版 [README.md](README.md)（英文版中的示例作为 doctest 运行）：为一笔转账构造扩展与隐式数据，计算待签名的 32 字节载荷。
