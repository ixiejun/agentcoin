> 🌐 [English](README.md) | **简体中文**

# pallet-pq-accounts

AgentCoin runtime 的抗量子账户模块（方案 §3.3，决策 D36）。

- **公钥登记表**：每个发过交易的账户都有一个当前 ML-DSA 公钥（带 AlgId）和一个轮换计数。账户 ID 由账户的**首个**公钥派生，因此账户在上链之前就可以接收转账。
- **`PqAuthorize`**：授权 v5 `General` 交易的交易扩展，必须排在扩展流水线首位。账户以上下文 `agentcoin/tx/v1` 对 32 字节载荷 `derive_key("agentcoin 2026-09 tx-payload v1", SCALE(继承的隐含数据))` 签名（见 `signing_payload`）。首笔交易携带公钥，之后的交易不得携带。
- **`rotate_key(new_key, proof)`**：替换当前公钥（可以换成另一种算法），账户 ID 不变。`proof` 是新密钥（上下文 `agentcoin/key-rotation/v1`）对 `rotation_statement(创世哈希, 账户, 轮换计数, 新公钥)` 的签名。一个公钥只能属于一个账户。
- **Runtime API** `PqAccountsApi::current_key(account)`，供钱包查询。

## Feature

| Feature | 默认 | 用途 |
|---|---|---|
| `std` | 是 | 原生构建与测试。WASM runtime 构建时关闭。 |
| `runtime-benchmarks` | 否 | `rotate_key` 与 `PqAuthorize`（按算法）的基准测试。 |
| `try-runtime` | 否 | SDK 的 try-runtime 支持。 |

## 示例

示例代码见英文版 [README.md](README.md)（英文版中的示例作为 doctest 运行）：由种子生成 ML-DSA-44 密钥并派生账户 ID，对交易隐含数据的签名载荷签名。
