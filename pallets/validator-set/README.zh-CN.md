> 🌐 [English](README.md) | **简体中文**

# pallet-validator-set

Aura-PQ 出块和 AC-BFT 终局性共用的纪元与授权节点集合，以及单向的 PoA→PoS 切换（方案 §4.1、§4.3；决策 D19、D24；`m3-pos` 设计 D6、D7）。

- **纪元**：区块按 `epoch_length` 个一组划分为纪元。纪元长度在创世时确定，且不小于 PoS 活跃集合规模 `K` 与 PoA 集合规模中较大者的 2 倍，使每个验证人在每个纪元内都有多个出块时隙。高度 `b ≥ 1` 的区块属于纪元 `⌊(b − 1) / L⌋`；每个纪元的第一个区块是纪元边界区块。
- **授权节点集合**：按顺序排列的 ML-DSA-65 公钥，每个成员带权重（PoA 阶段为 1；PoS 阶段为 `max(1, 支撑额 / 10^12)`），以及从 0 开始、每次变更加 1 的集合编号。出块时隙按成员顺序轮流分配，与权重无关；AC-BFT 按权重计票。
- **只在纪元边界变更**，原因有三：管理权限修改了 PoA 名单；因双签被记录的验证人（由 `pallet-ac-offences` 调用 `ValidatorSetInterface::disable`）离开；PoS 阶段选举结果变化。边界区块为出块写入新列表（从下一个区块起生效），为 AC-BFT 写入带新集合编号和成员列表的 `acbf` 摘要，并发出 `AuthorityDisabled`（违规者）和 `NewSet` 事件。没有变更的边界区块不带摘要。集合永远不会被清空：如果全部成员都违规，保留集合顺序中的第一个。
- **PoA 名单**：`add_poa_authority` 与 `remove_poa_authority`（仅管理权限）修改名单，名单在下一个纪元边界成为集合。名单不会被清空，也不会超过纪元长度的一半。违规者永久离开名单。
- **切换到 PoS**：PoA 阶段的每个纪元边界都是一个检查点。当全部有效质押不低于总发行量的 10%、合格候选人至少 21 个、区块高度不低于 63,115,200 时检查点达标（正式链取值；开发预设使用更小的值）。`QualifiedSince` 记录一段连续达标的首个检查点，任一检查点不达标即清除。连续达标满 604,800 个区块的那个纪元边界切换到 PoS：名单清空，选举产生的验证人组成集合，管理权限从此不能再修改集合。连续达标期间，每个纪元的最后一个区块公布一次预演选举。切换是单向的。
- **PoS 阶段**：每个纪元的最后一个区块为 `K` 个席位运行选举（`set_validator_count`，仅管理权限，取值介于切换所需的候选人数与纪元长度的一半之间）；纪元边界安装选举结果，并剔除被禁用的验证人。
- **固定存储键**，由节点读取；节点独立复算每个检查点，拒绝提前、拖延或退回的切换：`ValidatorSet::Phase`（`0` 为 PoA，`1` 为 PoS）、`ValidatorSet::QualifiedSince`（`Option<u64>`）、`ValidatorSet::PoaAuthorities`（公钥列表）与 `ValidatorSet::TransitionParams`（`stake_bps: u32, min_candidates: u32, min_height: u64, sustain_blocks: u64`）。这些键永不改名、不改编码。
- **历史集合**：最近 `HistoryEpochs` 个纪元内有效过的集合仍可查询，供证据验证使用（`ValidatorSetInterface::historical`、`is_recent_member`）。
- **Runtime API**：`ac_primitives::validator_set::ValidatorSetApi`（当前集合、纪元长度、历史集合）；切换进度由 `ac_primitives::staking::StakingApi` 提供。

## Feature

| Feature | 默认 | 用途 |
|---|---|---|
| `std` | 是 | 原生构建与测试。WASM runtime 构建时关闭。 |
| `runtime-benchmarks` | 否 | 纪元边界处理与名单调用的基准测试。 |
| `try-runtime` | 否 | SDK 的 try-runtime 支持。 |

## 示例

示例代码见英文版 [README.md](README.md)（作为 doctest 运行）：runtime 和节点共用的纪元计算——4 个授权节点要求纪元长度至少为 8；纪元长度为 20 时，高度 21 的区块开启纪元 1；上线满 2 年、质押 12%、25 个候选人时，检查点开始连续达标计时。
