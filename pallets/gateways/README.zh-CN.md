> 🌐 [English](README.md) | **简体中文**

# pallet-gateways

推理网关的链上登记（方案 §7）。网关提供 OpenAI 兼容 API、校验凭证、把请求路由给提供者并结算批量收据；用户向自己选择的网关托管额度（`pallet-credits`）。

## 网关记录

| 字段 | 含义 |
|---|---|
| `endpoint` | 用户连接的地址（UTF-8，最多 256 字节；不检查可达性）。 |
| `fee_bps` | 网关费率（基点），不超过 **500（5%）**（方案 §8；M8 把该上限登记到护栏）。 |
| `stake`、`unlocking` | 已绑定质押与至多 8 段解绑中的质押，以 `Stake` 冻结原因冻结。 |
| `status` | `Active` 或 `Exiting`。 |

活链的质押门槛为 **1,000 美元**（草案值），按参考汇率（`pallet-ref-rate`）向上取整换算；没有汇率时登记和解绑都会失败。

## 交易

| 交易 | 作用 |
|---|---|
| `register(endpoint, fee_bps, stake)` | 登记调用者并绑定 `stake`（不低于门槛）。 |
| `update(endpoint?, fee_bps?)` | 修改地址和/或费率（仍受上限约束）；退出中不能修改。 |
| `bond_extra(amount)` | 追加质押（退出中不能追加）。 |
| `unbond(amount)` | 把 `amount` 转入解绑；剩余绑定的质押必须不低于门槛。 |
| `exit()` | 不再接受新的托管；全部质押开始解绑。 |
| `withdraw_unbonded()` | 释放已到期的解绑质押（活链 7 天）；已退出且无剩余的网关被删除。 |

## 与其他模块的关系

`GatewayLookup`（`is_active`、`fee_bps`）让 `pallet-credits` 拒绝向非活跃网关托管，也让结算（M5 的下一个变更）据此支付网关费。

## 功能开关

| 开关 | 默认 | 用途 |
|---|---|---|
| `std` | 是 | 原生构建与测试。WASM runtime 须关闭。 |
| `runtime-benchmarks` | 否 | 所有交易的基准测试（`Config::BenchmarkHelper` 负责设置汇率）。 |
| `try-runtime` | 否 | SDK 的 try-runtime 支持。 |

## 示例

英文版 README 中的示例作为 doctest 运行：活链参数的 5% 费率上限，以及 1 ATC = 1 美元时网关至少绑定 1,000 ATC。
