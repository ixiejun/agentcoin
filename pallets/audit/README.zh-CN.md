> 🌐 [English](README.md) | **简体中文**

# pallet-audit

推理市场的链上审计（MVP 方案 §5.5；OpenSpec 变更 `m6-audit-chain`；规格 `market/audit`）。审计员在链下用 `ac-auditor` 复核自己发起的推理；本模块决定谁审计谁、记录裁决、处理争议、执行处罚与支付。prompt、回答与证明从不上链（红线 6）：不通过的裁决只带证据的承诺。

## 工作方式

1. **审计员**以美元计的质押登记（按参考汇率向上取整换算）。提供者与网关账户不能登记。质押按固定期限解绑，解绑期内仍可被罚没。
2. **轮次。** 每轮第一个区块记录本轮名单（质押不低于门槛、未在退出中的审计员，按账户排序）与由链上提交-揭示随机数派生的种子。提供者在该轮的审计员为 `ac_primitives::market::audit` 中的 `sample(名单, 种子, 提供者)`，任何人都能复算（`AuditApi::assignment`）。
3. **裁决。** 被分配的审计员每轮对每个提供者提交一次裁决：通过、不通过（原因与证据承诺）或无法判定（原因），附阈值版本与提供者和网关双签的收据。链检查收据（本链创世哈希、被审计的提供者、登记的公钥、两个 ML-DSA 签名、按提供者当前价格的费用），并检查其请求 ID 从未被使用过。
4. **争议。** 同一提供者在本轮与上一轮内收到两名不同审计员的“不通过”即开启争议：抽出 `N` 名复核人（排除提出者、提供者与收据的网关），在链下复核证据并投票。一方先达到 `Q` 票即决定结果：
   - **确认**：罚没并销毁提供者的质押，并经 `ProviderPenalty` 将其禁闭（其未结算的工作量由 `pallet-work` 作废）；
   - **驳回**：罚没并销毁每名提出者的质押，并使其退出；
   - 期满后任何人都能把未决的争议关闭，不处罚任何一方。
5. **支付。** 每条被接受的裁决、每张获胜一方的票，从审计资金池（`PalletId(*b"ac/audit")`）支付固定的美元金额（向下取整）；资金池由国库保底池以“审计”用途注资。资金池不足时跳过支付，从不铸币。

## 参数

| 参数 | 活链草案 | 测试预设 | 护栏 | 修改方式 |
|---|---|---|---|---|
| 轮次长度 | 1,800 个区块 | 20 | > 0 | 创世 |
| 每提供者每轮审计员数 | 2 | 2 | 1–8 | 创世 |
| 复核人数 / 票数门槛 | 5 / 3 | 3 / 2 | 1 ≤ Q ≤ N ≤ 15，2Q > N | 创世 |
| 投票期限 | 600 个区块 | 20 | > 0 | 创世 |
| 提供者 / 审计员罚没比例 | 10% / 10% | 10% / 10% | 1–100% | 创世 |
| 审计员解绑期 | 604,800 个区块 | 20 | > 0 | 创世 |
| 审计员质押门槛 | 1,000 美元 | 1,000 美元 | 100–100,000 美元 | 管理权限 |
| 每条裁决或每票的支付 | 0.05 美元 | 0.05 美元 | 0–1 美元 | 管理权限 |
| 接受的阈值版本 | `AUDIT_THRESHOLDS` | 同左 | 只能增大 | 管理权限 |

管理权限不能罚没、禁闭、开启或决定任何事情（D6）。

## 调用

`register`、`bond_extra`、`unbond`、`exit`、`withdraw_unbonded`、`submit_verdict`、`vote`、`close_dispute`、`set_params`（管理权限）。钱包以 `ac-wallet audit …` 封装这些调用。

## 示例

创世参数的护栏：

```rust
use ac_primitives::market::audit::AuditParams;
use pallet_audit::AuditGenesis;

let (fixed, adjustable) = AuditGenesis::LIVE.split();
assert_eq!(fixed, AuditParams::LIVE);
assert!(fixed.check().is_ok());
assert!(adjustable.within_bounds());
let broken = AuditParams { quorum: 2, ..AuditParams::LIVE }; // 5 人中 2 票不是多数
assert!(broken.check().is_err());
```

## 特性

`std`（默认）、`runtime-benchmarks`、`try-runtime`。
