> 🌐 [English](README.md) | **简体中文**

# pallet-staking-pos

AgentCoin 的提名式权益证明（方案 §4.3；决策 D19、D24；`m3-pos` 设计 D1–D4）：为 PoA→PoS 切换、验证人选举和罚没提供质押账本。质押、解绑和取回都不铸币也不销毁：质押以 `StakingPos` 锁定原因锁在账户上。

## 角色

一个账户要么是**候选人**，要么是**提名人**，不能同时是两者；更换角色需要账本为空（之前的质押已全部取回）。

- **候选人**注册一个 ML-DSA-65 验证人公钥，并附持有证明：该公钥在上下文 `agentcoin/validator-pop/v1` 下对 SCALE(`"agentcoin/validator-pop-statement"`, 创世哈希, 账户, 公钥) 的签名。这把密钥用于出块、AC-BFT 投票和随机数。公钥永久绑定到一个账户，候选人退出后也不能被他人使用。
- **提名人**锁定一笔金额并提名 1 到 16 个候选人。更换提名对象不需要解绑，从下一次选举起生效。

## 最低额与上限

| | 最低额 | 上限（正式链） |
|---|---|---|
| 候选人自质押 | 总发行量的 0.1% | 500 个候选人 |
| 提名 | 总发行量的 0.001% | 2,000 个提名人 |

最低额随发行量变化，向上取整。因发行量增长而低于最低额的候选人保留质押，但不算合格（不计入切换条件、不参加选举），直到补足为止。名单已满时，新加入者必须高于名单中最小的一笔；该笔被移出并开始解绑。

## 解绑

申请解绑的质押立即不再计入，但在解锁前仍被锁定：

- 候选人的自质押固定 28 天（2,419,200 个区块）；
- 提名通过全网队列退出，队列能在 28 天内退完全部有效质押：每笔等待 2 到 28 天，同时退出的质押越多等待越长（参照 Polkadot RFC-0097）。

PoA 阶段与 PoS 阶段适用同一规则。候选人用 `retire` 退出；提名人用 `unnominate`（或把质押全部解绑）退出。

## 佣金

5% 到 100%。调高 7 天（604,800 个区块）后生效；调低在下一个验证人纪元生效。生效前可在候选人记录中查到待生效的值。

## 固定存储键

由节点的 PoA→PoS 检查读取，永不改名、不改编码：

| 键 | 值 |
|---|---|
| `StakingPos::Ledger`（`Identity` 账户） | `{ active: u128, unlocking: Vec<{ value: u128, unlock_at: u64 }> }` |
| `StakingPos::Candidates`（`Identity` 账户） | `{ key, commission_bps: u32, pending_commission: Option<(u32, u64)>, chilled: bool }` |

## 调用

`register_candidate`、`bond_extra`、`set_validator_key`、`retire`、`nominate`、`set_nominations`、`unnominate`、`unbond`、`withdraw_unbonded`、`set_commission`、`chill`、`validate`。全部权重来自基准测试；注册和提名按扫描满名单的最坏情况收费，未扫描时退还差额。

## 查询

runtime API `StakingApi`：`stake`（账户的有效、解绑中和可取回金额）、`candidate`（候选人记录、当前生效的佣金和收到的提名）、`total_active` 与 `minimums`。

## Feature

| Feature | 默认 | 用途 |
|---|---|---|
| `std` | 是 | 原生构建和测试。WASM runtime 需关闭。 |
| `runtime-benchmarks` | 否 | 全部调用的基准测试。 |
| `try-runtime` | 否 | SDK 的 try-runtime 支持。 |

## 示例

示例代码见英文版 [README.md](README.md)（作为 doctest 运行）：正式链参数，以及总发行量为 10,000,000 ATC 时的最低额（自质押 10,000 ATC、提名 100 ATC）。
