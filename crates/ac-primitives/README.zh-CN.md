> 🌐 [English](README.md) | **简体中文**

# ac-primitives

AgentCoin 的链上共享类型，供 runtime、节点和客户端使用。

- `Blake3Hasher`：把 BLAKE3-256 适配到 Polkadot SDK 的哈希 trait。它是链的 `Hashing` 类型，因此区块哈希、外部交易根和状态根都使用 BLAKE3（决策 D35）。
- 地址：`encode_address` / `decode_address` 在 32 字节账户 ID 与 `atc1…` 形式的 bech32m（BIP-350）字符串之间互转；错一个字符一定能被检测出来。
- `NoClassicSignature`：一个不可能有值的类型，用来占住 SDK 旧式 `Signed` 交易的签名位置，保证任何经典签名算法都无法授权账户（决策 D36）。
- `ChainProfile` 与 `ChainProfileApi` runtime API：客户端可以借此核对链的哈希方案。
- `aura_pq`：Aura-PQ 的时隙、预运行摘要和封印摘要工具。
- `ac_bft`：AC-BFT 终局性类型：带版本号的已签名消息（提议、prepare / commit 投票、超时，签名上下文 `agentcoin/bft-vote/v1`）、终局性证明与 `verify_finality_proof`，以及授权节点集合变更摘要（引擎号 `acbf`）。未知的格式版本直接解码失败，不会被当作其他格式解析。
- `offences`：双签证据（同一时隙封印的两个区块，或同一轮中互相冲突的两条 AC-BFT 消息）与 `verify_evidence`，runtime 和节点共用。
- `epoch`：纪元编号（`epoch_of`、`is_boundary`）与纪元长度下限。
- `emission`：ATC 的排放曲线与纪元结算（方案 §5.1）。runtime、节点不变量检查器和经济模拟都使用它，因此三者算出的数完全一致。

AC-BFT 各种格式的字节级回归向量位于 `tests/vectors/`（见 `SOURCES.md`）。

## Feature

| Feature | 默认 | 用途 |
|---|---|---|
| `std` | 是 | 为节点、钱包和测试提供标准库支持。WASM runtime 构建时关闭。 |

## 示例

示例代码见英文版 [README.md](README.md)（英文版中的示例作为 doctest 运行）：对字节串计算 BLAKE3 哈希得到 32 字节输出；把账户 ID 编码为 `atc1…` 地址并解码回原值。

## 独立验证终局性证明

验证终局性证明只需要创世哈希和证明所属的授权节点集合，因此轻客户端无需信任任何节点就能确认终局性。

示例代码见英文版 [README.md](README.md)（作为 doctest 运行）：用 alice、bob、charlie、dave 四个开发密钥组成集合，其中 3 个成员（q = ⌊2·4/3⌋ + 1 = 3）对同一目标签署 commit 票，组成版本 1 的证明，`verify_finality_proof` 返回被最终确定的区块。

## 排放曲线

- 首个 4 年期（1 秒出块时为 126,230,400 个区块）排放 10,500,000 ATC，此后每期减半，整条曲线的总量低于 21,000,000 ATC 的上限（决策 D14、D15）。
- 排放按长度为 `L` 个区块的“排放纪元”结算；`L` 是创世参数，必须整除 126,230,400（正式链为 3600）。纪元 `e` 在第 `(e + 1) × L + 1` 块结算，只有这些区块铸币。
- `settle` 对一个纪元做分配：安全预算为计划量 `S` 的 10%（只在 PoS 阶段发放）；市场工作最多为 `S + min(储备, S)` 的 50%，公共工作最多为其 20%；国库为 `max(5% × S, 20/70 × 工作排放)`，取大、不相加。未铸造的部分滚入储备，每个纪元最多从储备取用 `S`。比例以基点整数计算，向下取整。

示例代码见英文版 [README.md](README.md)（作为 doctest 运行）：长度 3600 的排放曲线下，一个没有工作量的 PoA 纪元只铸造 5% 的国库保底，其余滚入储备；第 3601 块结算纪元 0。
