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

质押门槛以美元设定——活链为 **T1 1,000 美元、T2 100 美元**（草案值）——按参考汇率（`pallet-ref-rate`）向上取整换算。

提供者的**所需质押**取其档位门槛与**威慑额**中的较大者；威慑额为 `κ` 乘以该提供者近期每纪元的营收（`Config::Revenue`，由工作结算模块按最近 24 个纪元求平均）。`κ = (g × n × H − C) / f` 个纪元（m6-audit-hardening design D3，m6-multi-model-deterrence），由下列参数计算：

- `g`：假设的作弊收益占营收的比例（50%）；
- `H`：服务一个已校准模型的提供者的暴露期，以纪元计（11，取自统计判定的模拟）；
- `n`：提供者登记的模型中有审计档案的个数（`Config::Calibration`；至少为 1）。审计员每次审计从中随机选一个，所以每个模型的检出约慢 `n` 倍；模拟表明 `n` = 1、2、4、16 时 `n × H` 都能覆盖。这里假设审计员能复核提供者登记的每个已校准模型；
- `C`：挑战期（`Config::ChallengeEpochs`，2）；
- `f`：争议确认时罚没的质押比例（`Config::ConfirmedSlash`，10%）。

活链上服务一个已校准模型时 `κ` 为 35 个纪元，所以每纪元结算 100 美元的提供者需要 3,500 美元质押；两个时为 90 个纪元、9,000 美元。按这个质押，确认作弊时的损失不小于被发现前的作弊所得。`g` 与 `H` 在创世时设定（`deterrence`，护栏分别为 10%–100% 与 1–720 个纪元）。`kappa_bps_for(n)`、`calibrated_count(models)`、`required_usd(who, tier, models)` 与 `required_stake(who, tier, models)` 给出这些计算结果。

登记和解绑时检查所需质押；没有汇率时两者都失败。使所需质押上升且高于现有质押的新模型列表（`update`）会被拒绝（`BelowThreshold`）；不提高所需质押的列表即使在质押不足时也能修改。汇率、营收或模型的校准状态变动后质押低于所需质押的提供者保持登记，但在追加质押前不可服务。

## 可服务

提供者**可服务**当且仅当：状态为 `Active`、质押不低于所需质押、距最近心跳不超过两个心跳间隔（活链每个间隔 600 个区块）。`serviceable_providers(model, start_after, limit)` 按账户顺序分页返回某模型的可服务提供者（runtime 经 `MarketApi` 提供）。

## 交易

| 交易 | 作用 |
|---|---|
| `register(registration)` | 以 `Registration { tier, endpoint, kem_pk, models, stake, attestation }` 登记调用者并绑定 `stake`；`attestation` 必须为 `None`。 |
| `update(endpoint?, kem_pk?, models?)` | 修改其中任意项（规则同登记）；退出中不能修改。层级不能修改。使所需质押上升且高于现有质押的模型列表会失败。 |
| `heartbeat()` | 记录心跳。距上次心跳至少半个间隔时**免费**，否则收费。 |
| `bond_extra(amount)` | 追加质押（退出中不能追加）。 |
| `unbond(amount)` | 把 `amount` 转入解绑；剩余绑定的质押必须不低于所需质押。 |
| `exit()` | 停止服务：全部质押开始解绑，提供者从所有模型列表中移除。 |
| `withdraw_unbonded()` | 释放已到期的解绑质押（活链 7 天）；关于该提供者的审计争议未关闭时不能释放（`Config::DisputeLock`，错误 `DisputeOpen`）。已退出且无剩余的提供者被删除，之后可以重新登记。 |

解绑列表已满（8 段）时，新的一段合并到最后一段。

## 罚没（审计）

`ProviderPenalty::slash(who, ratio)` 按比例扣除全部质押——先扣已绑定部分，再按解锁先后扣解绑中的部分——并经 `Config::Slash` 销毁（runtime 中为 `Emission`，计入 `TotalBurned`）。`ProviderPenalty::jail(who)` 使提供者永久不可服务。只有 `pallet-audit` 在争议被确认时调用它们（m6-audit-chain）；没有任何交易能直接调用，PoA 管理多签也不能（D6：不设黑名单）。被禁闭的提供者仍可退出，并在解绑期后取回剩余的质押。`ProviderAudit` 向审计模块提供“是否为提供者”与其对某模型的当前价格。

## 功能开关

| 开关 | 默认 | 用途 |
|---|---|---|
| `std` | 是 | 原生构建与测试。WASM runtime 须关闭。 |
| `runtime-benchmarks` | 否 | 所有交易的基准测试（`Config::BenchmarkHelper` 负责登记模型和设置汇率）。 |
| `try-runtime` | 否 | SDK 的 try-runtime 支持。 |

## 示例

英文版 README 中的示例作为 doctest 运行：以 1 ATC = 2 美元计算 T2 门槛（50 ATC），以及两个心跳间隔（20 分钟）无心跳即不可服务。
