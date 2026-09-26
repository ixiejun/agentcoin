> 🌐 [English](README.md) | **简体中文**

# pallet-validator-set

Aura-PQ 出块和 AC-BFT 终局性共用的纪元与授权节点集合（方案 §4.1、§4.3）。这是全量方案 §11 中验证人集合模块在 M2 的部分；M3 会在同一个模块中加入 PoA → PoS 状态机、质押权重和会话密钥轮换。

- **纪元**：区块按 `epoch_length` 个一组划分为纪元。纪元长度在创世时确定，且不小于创世授权节点数的 2 倍，使每个验证人在每个纪元内都有多个出块时隙。高度 `b ≥ 1` 的区块属于纪元 `⌊(b − 1) / L⌋`；每个纪元的第一个区块是纪元边界区块。
- **授权节点集合**：按顺序排列的 ML-DSA-65 公钥，每个成员带权重（PoA 阶段为 1，为按质押加权预留），以及从 0 开始、每次变更加 1 的集合编号。
- **只在纪元边界变更**：因双签被记录的验证人（由 `pallet-ac-offences` 调用 `ValidatorSetInterface::disable`）在下一个纪元边界离开集合。边界区块为出块写入新列表（从下一个区块起生效），为 AC-BFT 写入带新集合编号和成员列表的 `acbf` 摘要，并发出 `AuthorityDisabled` 和 `NewSet` 事件。没有移除时，边界区块不做任何变更，也不带摘要。集合永远不会被清空：如果全部成员都违规，保留集合顺序中的第一个。
- **历史集合**：最近 `HistoryEpochs` 个纪元内有效过的集合仍可查询，供证据验证使用（`ValidatorSetInterface::historical`、`is_recent_member`）。
- **Runtime API**：`ac_primitives::validator_set::ValidatorSetApi`（当前集合、纪元长度、历史集合）。

## Feature

| Feature | 默认 | 用途 |
|---|---|---|
| `std` | 是 | 原生构建与测试。WASM runtime 构建时关闭。 |
| `runtime-benchmarks` | 否 | 纪元边界处理的基准测试。 |
| `try-runtime` | 否 | SDK 的 try-runtime 支持。 |

## 示例

示例代码见英文版 [README.md](README.md)（作为 doctest 运行）：runtime 和节点共用的纪元计算——4 个授权节点要求纪元长度至少为 8；纪元长度为 20 时，高度 21 的区块开启纪元 1。
