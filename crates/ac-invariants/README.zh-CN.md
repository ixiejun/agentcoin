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
- **fail-closed**：固定存储键缺失或无法解码时拒绝区块。
- **创世**（启动时）：排放纪元长度存在且能整除 4 年的区块数；发行量不超过上限；正式链不分配任何 ATC（无预挖，D9），且设置了 PoA 管理成员。

## 固定存储键（`keys`）

| 常量 | 存储项 | 编码 |
|---|---|---|
| `TOTAL_ISSUANCE` | `Balances::TotalIssuance` | `u128` |
| `TOTAL_BURNED` | `Emission::TotalBurned` | `u128`，只增不减 |
| `EMISSION_EPOCH_LENGTH` | `Emission::EpochLength` | `u64`，仅在创世写入 |
| `SYSTEM_ACCOUNT_PREFIX` | `System::Account` | 账户信息，余额为 `u128` |
| `POA_COUNCIL_MEMBERS` | `PoaCouncil::Members` | `Vec<AccountId32>` |

## Feature

| Feature | 默认 | 用途 |
|---|---|---|
| `std` | 是 | 标准库，以及错误类型的 `std::error::Error` 实现。 |

## 示例

示例代码见英文版 [README.md](README.md)（作为 doctest 运行）：纪元长度 10 时，第 5 块不结算任何纪元，铸造哪怕 1 个最小单位也被拒绝；第 11 块结算纪元 0，最多可以铸造其计划量。
