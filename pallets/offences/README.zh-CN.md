> 🌐 [English](README.md) | **简体中文**

# pallet-ac-offences

双签证据上链（方案 §4.1）。两类证据本身就能证明违规；链会验证其中每个 ML-DSA 签名，不信任任何举报者：

- **出块双签**：同一 ML-DSA-65 密钥在同一时隙封印的两个不同区块头（上下文 `agentcoin/aura-seal/v1`），该密钥属于最近的某个授权节点集合，且时隙没有超过 `MaxEvidenceAge`。
- **投票双签**：同一集合的同一成员在同一轮签出的两条内容不同、类型相同（提议、prepare 或 commit）的 AC-BFT 消息。超时消息永远不算违规。

证据格式和验证函数（`ac_primitives::offences::verify_evidence`）与节点共用；节点在导入区块和接收 AC-BFT 消息时检测双签，并通过 `OffencesApi` runtime API 自动提交举报。

## 无需签名和手续费的提交

`report_equivocation(evidence)` 自带授权规则：借助 runtime 的 `AuthorizeCall` 交易扩展，不带账户签名的交易可以在证据有效且尚未记录时调用它。交易池在接纳之前就完成这项检查，因此无效证据既进不了交易池，也进不了区块；任何账户都不需要付手续费，也不消耗 nonce。

## 记录与后果

- 每次违规只记录一次；同一违规者在同一授权节点集合内每类违规（出块双签、AC-BFT 投票双签）最多记录一次，此后针对它在该集合内同一类违规的证据视为过期，另一类仍会被记录。每个集合的记录数不超过集合成员数的 2 倍；集合离开验证人集合模块的历史窗口后，其记录随之清除。两类分开记录，是因为严重程度不同（投票双签威胁终局性，出块双签只造成分叉），而且证据有有效期。
- 违规者在某集合内的第一条记录使它在下一个纪元边界退出出块和 AC-BFT 投票（由 `pallet-validator-set` 执行，该模块同时保证集合永不为空）。另一类违规的记录只发出事件，不再追加处罚。
- 每条记录都会调用 `SlashHandler`，并传入该违规者在本集合已记录的违规类型；实现方按最重的一类罚没，不得累加。
- **M2 不做罚没**：PoA 阶段验证人没有质押（决策 D9、D19），因此 M2 的 `SlashHandler` 是 `()`，不罚没任何金额，所有余额和总发行量都不变。M3 接入质押后罚没 100% 并销毁。
- 事件：`OffenceReported { offender, key, set_id, slashed }`。

## Feature

| Feature | 默认 | 用途 |
|---|---|---|
| `std` | 是 | 原生构建与测试。WASM runtime 构建时关闭。 |
| `runtime-benchmarks` | 否 | 证据验证与记录的基准测试。 |
| `try-runtime` | 否 | SDK 的 try-runtime 支持。 |

## 示例

示例代码见英文版 [README.md](README.md)（作为 doctest 运行）：违规键能标识具体的违规，节点在提交前可以据此区分重复举报。
