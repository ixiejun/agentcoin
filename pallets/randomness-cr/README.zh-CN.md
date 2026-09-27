> 🌐 [English](README.md) | **简体中文**

# pallet-randomness-cr

由验证人共同产生的 commit–reveal 纪元随机数（方案 §4.2），用于审计抽样（M6）等低价值用途。

- **承诺**：在每个纪元 `e`，每个验证人在自己产出的区块中提交 `derive_key("agentcoin 2026-09 randomness-commit v1", secret(e))`。秘密值由验证人私钥、创世哈希和 `e` 派生（`ac_crypto::randomness_secret`），因此他人无法预测，验证人重启后也仍能揭示。
- **揭示**：在纪元 `e + 1`，验证人在自己的某个区块中揭示 `secret(e)`，链会将其与承诺核对。
- **公布**：在纪元 `e + 2` 的边界区块，链公布 `R(e) = derive_key("agentcoin 2026-09 randomness v1", u64_le(e) ‖ 按账户 ID 排序的揭示值)`。任何人都可以用 `ac_primitives::randomness::epoch_randomness` 从公开的揭示值复算。没有任何揭示的纪元没有随机数。
- 承诺和揭示都放在 inherent `note_randomness` 中，由出块人的节点填写。出块人取自 Aura-PQ 记录的本区块时隙，因此任何人都无法替他人承诺或揭示。同一纪元的第二个承诺、与承诺不符的揭示都会被忽略。
- 提交了承诺却没有揭示的验证人计入 `MissedReveals`；每次结论还会记下本次未揭示的名单（`LastMissed`），`pallet-staking-pos` 在得出结论的区块中通过 `RevealTracker` 读取。PoA 阶段未揭示没有其他后果。PoS 阶段该验证人在未揭示的那个纪元不计工作分，连续 3 个纪元未揭示则被暂停参选，直到其申请恢复（规格 economics/validator-rewards）。
- **查询**：`RandomnessApi` runtime API（最新值、按主题派生的值、各纪元的值与揭示值），以及供其他模块使用的 FRAME `Randomness` trait。按主题派生的值为 `derive_key("agentcoin 2026-09 randomness-subject v1", R ‖ subject)`，不同主题之间互相独立。

## 已知偏置

最后一个揭示的验证人可以选择不揭示，从而在两个结果之间二选一；多个验证人串通时可选择的结果更多。**该随机数只适用于审计抽样一类低价值用途**，不得用于偏置结果的价值超过扣留揭示者所得奖励的场景。全量版在相同接口下用哈希 VDF 消除这一偏置（全量方案 §2.4）。

## Feature

| Feature | 默认 | 用途 |
|---|---|---|
| `std` | 是 | 原生构建与测试。WASM runtime 构建时关闭。 |
| `runtime-benchmarks` | 否 | 记录与纪元结算的基准测试。 |
| `try-runtime` | 否 | SDK 的 try-runtime 支持。 |

## 示例

示例代码见英文版 [README.md](README.md)（作为 doctest 运行）：由揭示值复算纪元随机数，揭示值的顺序不影响结果；不同主题派生出不同的值。
