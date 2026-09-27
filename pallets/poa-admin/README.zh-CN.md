> 🌐 [English](README.md) | **简体中文**

# pallet-poa-admin

PoA 阶段的链管理权限（决策 D41）：创世设定的一组 ML-DSA 成员账户，达到门限 `t`（`1 ≤ t ≤ 成员数`）后共同行动。管理范围包括 runtime 升级、国库支出和成员自身的变更，之后移交给链上治理。

## 决议流程

提案、投票和关闭由 SDK 的 `pallet-collective`（实例 `Instance1`，runtime 中名为 `PoaCouncil`）完成：

1. 成员调用 `PoaCouncil::propose(threshold, call, length_bound)`；决议由调用内容的 BLAKE3 哈希标识；
2. 成员调用 `PoaCouncil::vote`（提议者也要投票：提议本身不算投票）；
3. 批准数足够后，或超过决议期限（正式链 7 天，开发链 20 块）后，任何人调用 `PoaCouncil::close`；超期时未投票的成员视为反对。

通过的决议以集体来源 `Members(yes, total)` 执行其调用。`EnsureCouncilThreshold` 只在 `yes ≥ Threshold` 时接受该来源，所以成员无法用较低的决议门限绕过管理门限。非成员不能提案，也不能投票。

## 调用

| 调用 | 来源 | 作用 |
|---|---|---|
| `dispatch_as_root(call)` | 管理权限 | 经 `Config::RootCallFilter` 允许后以 Root 执行 `call`（例如 `System::set_code`）；发出带执行结果的 `DispatchedAsRoot` 事件。 |
| `set_threshold(t)` | 管理权限 | `1 ≤ t ≤ 成员数`。 |
| `set_members(members, t)` | 管理权限 | 同时替换成员与门限；从未结决议中移除被替换成员的投票。collective 自身的 `set_members` 已关闭（`SetMembersOrigin = EnsureNever`）。 |

成员与门限只能一起变更并经过检查，所以管理权限永远不会把自己锁死。

## 创世

`PoaCouncil.members` 与 `PoaAdmin.threshold`。门限不在 `1..=成员数` 内时创世构建失败；没有成员且门限为 0 表示未设置管理权限，节点在正式链上会拒绝（`ac-invariants::check_genesis`）。预设：`dev` 为 alice / 1，`local` 为 alice、bob、charlie / 2。

## 为什么不用 sudo 或 pallet-multisig

sudo 让单一私钥控制全链。SDK 的 `pallet-multisig` 用 BLAKE2-256 派生多签账户，这是承载完整性的标识符，不在决策 D35 的例外之内；`pallet-collective` 只使用链的哈希函数。

## Feature

| Feature | 默认 | 用途 |
|---|---|---|
| `std` | 是 | 原生构建和测试。WASM runtime 需关闭。 |
| `runtime-benchmarks` | 否 | 三个调用的基准测试。 |
| `try-runtime` | 否 | SDK 的 try-runtime 支持。 |

## 示例

collective 实例与正式链的默认决议期限：

```rust
use pallet_poa_admin::{CouncilInstance, DEFAULT_MOTION_DURATION};

assert_eq!(DEFAULT_MOTION_DURATION, 604_800); // 1 秒出块的 7 天
let _instance: Option<CouncilInstance> = None;
```
