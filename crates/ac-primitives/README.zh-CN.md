> 🌐 [English](README.md) | **简体中文**

# ac-primitives

AgentCoin 的链上共享类型，供 runtime、节点和客户端使用。

- `Blake3Hasher`：把 BLAKE3-256 适配到 Polkadot SDK 的哈希 trait。它是链的 `Hashing` 类型，因此区块哈希、外部交易根和状态根都使用 BLAKE3（决策 D35）。
- 地址：`encode_address` / `decode_address` 在 32 字节账户 ID 与 `atc1…` 形式的 bech32m（BIP-350）字符串之间互转；错一个字符一定能被检测出来。
- `NoClassicSignature`：一个不可能有值的类型，用来占住 SDK 旧式 `Signed` 交易的签名位置，保证任何经典签名算法都无法授权账户（决策 D36）。
- `ChainProfile` 与 `ChainProfileApi` runtime API：客户端可以借此核对链的哈希方案。

## Feature

| Feature | 默认 | 用途 |
|---|---|---|
| `std` | 是 | 为节点、钱包和测试提供标准库支持。WASM runtime 构建时关闭。 |

## 示例

示例代码见英文版 [README.md](README.md)（英文版中的示例作为 doctest 运行）：对字节串计算 BLAKE3 哈希得到 32 字节输出；把账户 ID 编码为 `atc1…` 地址并解码回原值。
