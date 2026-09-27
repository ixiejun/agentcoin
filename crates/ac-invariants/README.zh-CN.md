> 🌐 [English](README.md) | **简体中文**

# ac-invariants

以纯函数实现的 AgentCoin 宪法第 1 层（决策 D24、D30；MVP 技术方案 §6）。节点客户端对导入或产出的每个区块执行这些检查，任一失败即拒绝该区块。检查只读取已发布的固定存储键和链规格中固定的参数，从不依赖 runtime，因此任何 runtime 升级都绕不过去。

**修改这里的任何规则或存储键都是硬分叉**：需要发布新的节点版本、单独的 OpenSpec 变更，并经用户明确确认（AGENT.md §8）。

## 规则

- **总量上限**：总发行量 ≤ 21,000,000 ATC。
- **铸币受排放曲线约束**，铸币量 = 发行量变化 + 累计销毁量变化：
  - 不结算排放纪元的区块不能铸币；
  - 结算纪元 `e` 的区块最多铸造 `2 × S(e)`（计划量，加上最多同样数额的储备取用）；
  - 自创世起的铸币总量不超过已结算纪元的累计计划量。
- **PoA→PoS 切换**（`check_transition`；决策 D19、D24）：
  - 阶段不能从 PoS 回到 PoA，PoS 阶段 PoA 名单必须为空；
  - 在 PoA 纪元边界之外，阶段和连续达标的起始高度都不能变化；
  - 在 PoA 纪元边界，节点用 `ac_primitives::staking::transition_step` 从父状态（账本中全部有效质押之和、合格候选人数、发行量、高度）独立复算检查点；区块写入的阶段和起始高度必须与之完全一致，因此切换既不能提前，也不能拖延。
- **fail-closed**：固定存储键缺失或无法解码时拒绝区块。
- **创世**（启动时）：排放纪元长度存在且能整除 4 年的区块数；发行量不超过上限；切换参数和验证人纪元长度存在；正式链不分配任何 ATC（无预挖，D9），设置了 PoA 管理成员，且切换参数等于宪法值（10%、21 个候选人、63,115,200 个区块、604,800 个区块）。

## 固定存储键（`keys`）

| 常量 | 存储项 | 编码 |
|---|---|---|
| `TOTAL_ISSUANCE` | `Balances::TotalIssuance` | `u128` |
| `TOTAL_BURNED` | `Emission::TotalBurned` | `u128`，只增不减 |
| `EMISSION_EPOCH_LENGTH` | `Emission::EpochLength` | `u64`，仅在创世写入 |
| `SYSTEM_ACCOUNT_PREFIX` | `System::Account` | 账户信息，余额为 `u128` |
| `POA_COUNCIL_MEMBERS` | `PoaCouncil::Members` | `Vec<AccountId32>` |
| `PHASE` | `ValidatorSet::Phase` | `u8` 枚举：`0` 为 PoA，`1` 为 PoS |
| `QUALIFIED_SINCE` | `ValidatorSet::QualifiedSince` | `Option<u64>`，始终存在 |
| `POA_AUTHORITIES` | `ValidatorSet::PoaAuthorities` | 带标签公钥的列表 |
| `TRANSITION_PARAMS` | `ValidatorSet::TransitionParams` | `(u32, u32, u64, u64)`，仅在创世写入 |
| `VALIDATOR_EPOCH_LENGTH` | `ValidatorSet::EpochLength` | `u64`，仅在创世写入 |
| `STAKING_LEDGER_PREFIX` | `StakingPos::Ledger`（`Identity` 账户） | 账本，第一个字段为 `u128` 有效质押 |
| `STAKING_CANDIDATES_PREFIX` | `StakingPos::Candidates`（`Identity` 账户） | `CandidateRecord` |

## Feature

| Feature | 默认 | 用途 |
|---|---|---|
| `std` | 是 | 标准库，以及错误类型的 `std::error::Error` 实现。 |

## 示例

示例代码见英文版 [README.md](README.md)（作为 doctest 运行）：纪元长度 10 时，第 5 块不结算任何纪元，铸造哪怕 1 个最小单位也被拒绝；第 11 块结算纪元 0，最多可以铸造其计划量；上线第一年就切换到 PoS 的 runtime 无论质押多少都被拒绝。
