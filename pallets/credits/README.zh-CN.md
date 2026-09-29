> 🌐 [English](README.md) | **简体中文**

# pallet-credits

透明推理额度：统一 `Credit` 接口的 α 实现（决策 D20；方案 §5.2）。测试网用它为推理付费；β 阶段新增匿名凭证作为同一接口的第二个实现，主网只接受匿名凭证。

## 通道与累计式凭证

用户向一个活跃网关托管 ATC（`deposit`）。托管额冻结在用户账户上，并记入该（用户, 网关）的**通道**：

| 字段 | 含义 |
|---|---|
| `escrow` | 仍在托管的 ATC。 |
| `number` | 通道号；通道重置时加一。 |
| `redeemed` | 已兑付的累计美元额。 |
| `key` | **凭证密钥**的指纹：通道开通时用户登记的公钥。 |
| `pending_withdrawal`、`pending_key` | 已申请的取回与密钥更换，以及其生效区块。 |

用户用**累计式凭证**付费，而不是每个请求一张、带 nonce 的凭证：

```text
VoucherBody { genesis, user, gateway, channel, cumulative（微美元） }
payload   = BLAKE3-derive_key("agentcoin 2026-09 voucher-payload v1", SCALE(body))
signature = ML-DSA.sign(凭证密钥, payload, 上下文 "agentcoin/voucher/v1")
```

每张凭证提高网关可收取的总额；网关只需保存最新一张。链上每个通道只存一个计数，因此重放的或较旧的凭证不付款，也不会有不断增长的 nonce 集合。

## 兑付（仅供结算调用）

`Credit::redeem(gateway, voucher, payee)`——没有兑付交易；由结算（M5 的下一个变更）调用：

1. 凭证必须写明本链创世哈希、兑付的网关、通道当前的通道号，用通道的凭证密钥签名且验签通过；
2. 增量 = 累计额 − 已兑付额（为零时不付款，也不是错误）；
3. 增量按参考汇率**向下取整**换算；`min(换算额, 托管余额)` 从托管转给 `payee`（必须是已存在的账户）；
4. 即使托管不足，`redeemed` 也更新为凭证的累计额；不足部分作为**短缺**报告，由网关承担。

`check(voucher)`（以及 `MarketApi::check_voucher`）以完全相同的规则校验而不改变任何状态，网关在提供服务前即可知道凭证能否兑付、托管是否足够。两者都调用 `ac_primitives::market::voucher::check_voucher`。

## 交易

| 交易 | 作用 |
|---|---|
| `deposit(gateway, amount)` | 向活跃网关托管 `amount`；首次托管以调用者当前公钥开通通道。 |
| `request_withdrawal(gateway, amount)` | 申请在等待期（活链 1 天）后取回。与未完成的申请合并，等待期重新计算。等待期内网关仍可兑付。 |
| `withdraw(gateway)` | 等待期后释放 `min(申请额, 托管余额)`。取空的通道被**重置**：通道号加一，`redeemed` 归零，此前的凭证全部作废。 |
| `request_key_change(gateway)` | 等待期后，凭证必须用调用者当前的公钥签名。单纯轮换账户密钥不会改变凭证密钥，因此用户无法让网关已接受的凭证失效。 |

## 功能开关

| 开关 | 默认 | 用途 |
|---|---|---|
| `std` | 是 | 原生构建与测试。WASM runtime 须关闭。 |
| `runtime-benchmarks` | 否 | 所有交易与一次兑付（ML-DSA-87）的基准测试；会打开 `ac-crypto` 的确定性签名，以便在 runtime 内构造凭证。 |
| `try-runtime` | 否 | SDK 的 try-runtime 支持。 |

## 示例

英文版 README 中的示例作为 doctest 运行：签发一张累计 0.5 美元的凭证，并对已兑付 0.2 美元、托管 10 ATC 的通道做校验，得到增量 0.3 美元（0.3 ATC）且托管足够。
