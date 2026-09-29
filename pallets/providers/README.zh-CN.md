> 🌐 [English](README.md) | **简体中文**

# pallet-providers

推理算力提供者的链上登记（方案 §5.3）。网关从这里读取某个模型的可服务提供者，并把请求路由给它们。

## 提供者记录

| 字段 | 含义 |
|---|---|
| `tier` | `T1`（数据中心）或 `T2`（消费级）。`T0`（TEE）为预留，登记时拒绝。 |
| `endpoint` | 网关连接的地址（UTF-8，最多 256 字节；不检查可达性）。 |
| `kem_pk` | 带 AlgId 的 X-Wing（ML-KEM-768 + X25519）公钥，网关用它加密请求。其他 KEM 算法一律拒绝。 |
| `models` | 1–16 个已登记模型，不得重复，每个带正数的每百万输入与输出 token 价格（微美元）。 |
| `stake`、`unlocking` | 已绑定质押与至多 8 段解绑中的质押，均以 `Stake` 冻结原因冻结。 |
| `status` | `Active`、`Exiting` 或 `Jailed`。 |
| `last_heartbeat` | 最近一次心跳的区块。 |
| `metrics`、`attestation` | 预留：审计指标（M6）与 TEE 证明（T0）。 |

## 以美元计的质押

质押门槛以美元设定——活链为 **T1 1,000 美元、T2 100 美元**（草案值）——按参考汇率（`pallet-ref-rate`）向上取整换算。登记和解绑时检查门槛；没有汇率时两者都失败。汇率变动后质押低于门槛的提供者保持登记，但在追加质押前不可服务。

## 可服务

提供者**可服务**当且仅当：状态为 `Active`、质押不低于当前门槛、距最近心跳不超过两个心跳间隔（活链每个间隔 600 个区块）。`serviceable_providers(model, start_after, limit)` 按账户顺序分页返回某模型的可服务提供者（runtime 经 `MarketApi` 提供）。

## 交易

| 交易 | 作用 |
|---|---|
| `register(registration)` | 以 `Registration { tier, endpoint, kem_pk, models, stake, attestation }` 登记调用者并绑定 `stake`；`attestation` 必须为 `None`。 |
| `update(endpoint?, kem_pk?, models?)` | 修改其中任意项（规则同登记）；退出中不能修改。层级不能修改。 |
| `heartbeat()` | 记录心跳。距上次心跳至少半个间隔时**免费**，否则收费。 |
| `bond_extra(amount)` | 追加质押（退出中不能追加）。 |
| `unbond(amount)` | 把 `amount` 转入解绑；剩余绑定的质押必须不低于门槛。 |
| `exit()` | 停止服务：全部质押开始解绑，提供者从所有模型列表中移除。 |
| `withdraw_unbonded()` | 释放已到期的解绑质押（活链 7 天）。已退出且无剩余的提供者被删除，之后可以重新登记。 |

解绑列表已满（8 段）时，新的一段合并到最后一段。

## 罚没（供 M6 使用）

`ProviderPenalty::slash(who, ratio)` 按比例扣除全部质押——先扣已绑定部分，再按解锁先后扣解绑中的部分——并经 `Config::Slash` 销毁（runtime 中为 `Emission`，计入 `TotalBurned`）。`ProviderPenalty::jail(who)` 使提供者永久不可服务。没有任何交易能调用它们，PoA 管理多签也不能（D6：不设黑名单）；由审计模块（M6）接入。

## 功能开关

| 开关 | 默认 | 用途 |
|---|---|---|
| `std` | 是 | 原生构建与测试。WASM runtime 须关闭。 |
| `runtime-benchmarks` | 否 | 所有交易的基准测试（`Config::BenchmarkHelper` 负责登记模型和设置汇率）。 |
| `try-runtime` | 否 | SDK 的 try-runtime 支持。 |

## 示例

英文版 README 中的示例作为 doctest 运行：以 1 ATC = 2 美元计算 T2 门槛（50 ATC），以及两个心跳间隔（20 分钟）无心跳即不可服务。
