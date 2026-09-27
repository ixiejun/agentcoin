> 🌐 [English](README.md) | **简体中文**

# pallet-emission

ATC 的计划排放（方案 §5.1；决策 D11、D14–D16、D18、D19）。这是 runtime 中唯一的铸币位置，节点另行独立检查其上限（宪法第 1 层，`ac-invariants`）。

## 计划曲线

- 常量：总量上限 21,000,000 ATC；第一个四年期在 `BLOCKS_PER_PERIOD = 126,230,400` 个区块（4 × 365.25 天，1 秒出块）内排放 10,500,000 ATC；之后每期是上一期的一半。
- 区块按 `EpochLength` 分成**排放纪元**。纪元长度是创世参数（正式链 3,600；开发链不超过 20），必须整除 `BLOCKS_PER_PERIOD`，所以减半只发生在纪元边界。取值非法时创世构建失败。
- 纪元 `e` 位于第 `n = e × L / BLOCKS_PER_PERIOD` 期，计划量 `S(e) = (10,500,000 ATC >> n) × L / BLOCKS_PER_PERIOD`，向下取整。
- 第 `1..=L` 块为纪元 0。纪元 `e` 在第 `(e + 1) × L + 1` 块的 `on_initialize` 中结算；其他区块不铸币。

## 结算

`ac_primitives::emission::settle` 是唯一的实现，节点不变量和经济模拟（`tests/sim`）共用它：

```text
avail    = S + min(reserve, S)
security = 10% × S                          （只在 PoS 发放；PoA 阶段滚存）
market   = min(50% × avail, 已核验市场工作量)
public   = min(20% × avail, 已核验公共工作量)
treasury = max((market + public) × 20 / 70, 5% × S)   （取大，不相加）
total    = security + market + public + treasury，超过 avail 时按比例缩减
reserve' = reserve + S − total
```

所有运算都是 `u128` 向下取整，余数留在储备。国库部分交给 `Config::Treasury`（`pallet-treasury-dual`）：比例份额进入社区资助和持币人国库，补足到 5% × S 的差额进入线性解锁的保底账户。M5 之前没有已核验工作量（`Config::WorkSource = ()`），PoA 阶段不发放安全预算（`Config::SecurityBudget = PoaPhase`），所以每个纪元只铸造 5% 的保底，其余累积在储备中。货币模块拒绝铸造的部分（例如新账户低于存在性押金的份额）同样回到储备。

## 销毁

runtime 中所有销毁（目前是交易手续费和小费的 80%；之后还有罚没和推理费）都经过本模块的 `OnUnbalanced` 实现：丢弃 `Credit` 使总发行量减少，同时把金额累加到 `TotalBurned`。

## 固定存储键

已发布，由节点不变量读取；永不更名、不改编码，runtime 中的模块名必须保持为 `Emission`：

| 存储项 | 键 | 编码 |
|---|---|---|
| `Emission::TotalBurned` | `twox128("Emission") ‖ twox128("TotalBurned")` | SCALE `u128`，创世时写入 |
| `Emission::EpochLength` | `twox128("Emission") ‖ twox128("EpochLength")` | SCALE `u64`，创世参数 |

## 查询

runtime API `EmissionApi`：`epoch_length`、`current_epoch`、`scheduled(epoch)`、`reserve`、`total_minted`、`total_burned`。每次结算发出 `EpochSettled` 事件，包含各部分金额。

## 将来的护栏

M3 中以下参数是常量或创世参数；护栏模块（M8）上线后改为有界的可调参数：

| 参数 | 当前 | 将来的护栏范围 |
|---|---|---|
| 安全 / 市场 / 公共 / 国库 分配比例 | 10 / 50 / 20 / 20 | 围绕当前值的有界范围 |
| 每纪元储备取用上限 | 1 × S | [0.5, 2] × S |
| 国库比例份额中社区资助的比例 | 40%（创世） | 有界范围 |

## Feature

| Feature | 默认 | 用途 |
|---|---|---|
| `std` | 是 | 原生构建和测试。WASM runtime 需关闭。 |
| `runtime-benchmarks` | 否 | 结算钩子的基准测试。 |
| `try-runtime` | 否 | SDK 的 try-runtime 支持。 |
| `test-overmint` | 否 | **仅测试**：结算时额外铸造两倍计划量，用来证明节点会拒绝这样的 runtime。真实构建中永不打开。 |

## 示例

PoA 阶段、没有工作量时，正式链第一个纪元的结算只铸造 5% 的保底：

```rust
use ac_primitives::emission::{EmissionSchedule, EpochInput, Phase, UNITS, settle};

let schedule = EmissionSchedule::new(3_600)?;
let s = schedule.scheduled(0);
assert_eq!(s, 10_500_000 * UNITS * 3_600 / 126_230_400);
let out = settle(&EpochInput::new(s, 0, (0, 0), Phase::Poa));
assert_eq!(out.total, s * 5 / 100);
assert_eq!(out.reserve, s - out.total);
# Ok::<(), ac_primitives::emission::EmissionError>(())
```
