> 🌐 [English](README.md) | **简体中文**

# pallet-treasury-dual

AgentCoin 的国库（方案 §5.1；决策 D12、D18、D42）。国库只持有排放结算转入的资金，从不铸币或销毁：支出就是普通转账。

## 账户

三个由已发布 `PalletId` 派生、没有私钥的账户（`"modl" ‖ id` 后补零，不经哈希），任何人都能算出，没有人持有其私钥：

| 账户 | `PalletId` | 收入 | 支出方 |
|---|---|---|---|
| 社区资助 | `ac/trcom` | 国库比例份额的 40%（创世参数 `community_share`，基点） | 管理权限，`spend` |
| 持币人国库 | `ac/trhld` | 比例份额的其余部分，含取整余数 | **M8 之前无人可支出** |
| 保底 | `ac/trflr` | 补足到计划量 5% 的差额 | 管理权限，`spend_floor`，仅限已解锁部分 |

比例份额为每个纪元工作量排放的 `(market + public) × 20 / 70`；国库取它与 5% × S 中的较大者，不相加（红线 5）。

## 保底解锁

保底入账按 30 天（2,592,000 块）分批。每批从批次**结束**时起在 2 年（63,115,200 块）内线性解锁，所以任何入账都不会早于入账后两年全部可用。可支出金额为 `Σ vested(batch) − spent`。任一时刻仍在解锁中的批次最多 26 个；已全部解锁的批次合并为一个已到期总额。

`spend_floor` 须注明用途：`Audit`（审计）或 `ColdStart`（冷启动）；用途记录在 `Spent` 事件中。

## 持币人国库锁定

在链上持币人投票（M8）上线之前，持币人国库只进不出，由三层保证：

1. 其账户没有私钥（`PalletId` 派生）；
2. 本模块没有从中支出的调用；
3. runtime 的 `HolderTreasuryLock` 调用过滤器拒绝涉及它的强制转账、强制改余额和直接写存储——既作为基础调用过滤器，也在 `PoaAdmin::dispatch_as_root` 内部再检查一次，因为 Root 会绕过基础过滤器。

runtime 升级可以改变任何规则；升级是管理权限的公开决议，并且仍受节点不变量约束。

## 管理权限

`spend` 与 `spend_floor` 要求 `Config::AdminOrigin`：在 runtime 中即达到门限的 PoA 理事会决议，可直接执行，也可经 `PoaAdmin::dispatch_as_root` 执行。签名账户会被拒绝。

## 查询

runtime API `TreasuryApi`：`community`、`holder`、`floor`（账户与余额）和 `floor_spendable`。

## Feature

| Feature | 默认 | 用途 |
|---|---|---|
| `std` | 是 | 原生构建和测试。WASM runtime 需关闭。 |
| `runtime-benchmarks` | 否 | `spend` 与 `spend_floor` 的基准测试。 |
| `try-runtime` | 否 | SDK 的 try-runtime 支持。 |

## 示例

批次结束一年后，该批次的一半可以支出：

```rust
use ac_primitives::emission::{FLOOR_VESTING_BLOCKS, split_treasury, vested};
use pallet_treasury_dual::{DEFAULT_COMMUNITY_SHARE, HOLDER_PALLET_ID};

assert_eq!(&HOLDER_PALLET_ID.0, b"ac/trhld");
assert_eq!(split_treasury(1_000, u128::from(DEFAULT_COMMUNITY_SHARE)), (400, 600));
let batch_end = 2_592_000;
assert_eq!(vested(1_000, batch_end, batch_end + FLOOR_VESTING_BLOCKS / 2), 500);
assert_eq!(vested(1_000, batch_end, batch_end + FLOOR_VESTING_BLOCKS), 1_000);
```
