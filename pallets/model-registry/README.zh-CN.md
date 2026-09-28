> 🌐 [English](README.md) | **简体中文**

# pallet-model-registry

推理模型的无许可登记（方案 §5.3）。网关、提供者和审计者都用同一个按内容寻址、任何人都能复算的 ID 指称一个模型。

## 模型 ID

模型的**清单**包含：名称与架构（非空 UTF-8，各不超过 128 字节）、权重格式（`QuantType`：`Bf16 = 1`、`Fp16 = 2`、`Fp8 = 3`、`Int8 = 4`、`Int4 = 5`，编号永不复用），以及按加载顺序排列的各权重分片的 BLAKE3-256 哈希（1 到 1,024 个分片）。模型 ID 为

```text
model_id = BLAKE3-derive_key("agentcoin 2026-09 model-id v1", SCALE(清单))
```

由链根据提交的清单计算。`ac_primitives::market::ModelManifest::id` 在链下算出同样的值；分片顺序不同，ID 就不同。

## 交易

| 交易 | 来源 | 作用 |
|---|---|---|
| `register(manifest, lineage, license_tag, royalty)` | 签名账户 | 以计算出的 ID 登记模型，并冻结调用者的一份存储押金。 |

以下情况登记失败：清单已被（任何人）登记；清单或许可证标签不是合法 UTF-8；声明的父模型未登记或就是模型自身；填写了版税（全量版预留，D22）；调用者付不起押金。记录不可修改，MVP 阶段也不可删除。

- **血缘**（D28）：可选的父模型与派生方式（`Finetune`、`Quantize`、`Distill`、`Merge`）。协议记录声明，不验证其真实性。
- **许可证标签**：仅作展示（最多 64 字节），协议不据此做任何事（D6）。
- **押金**：`DepositPerItem + DepositPerByte × 记录编码长度`，以本模块的 `Deposit` 冻结原因冻结（runtime 中为每条目 0.01 ATC、每字节 0.0001 ATC，与合约存储押金同一公式）。冻结不改变总发行量。

## 与其他模块的关系

`pallet-providers` 通过 `ac_primitives::market::traits::ModelLookup`（本模块实现）检查提供者所服务的模型是否存在。

## 功能开关

| 开关 | 默认 | 用途 |
|---|---|---|
| `std` | 是 | 原生构建与测试。WASM runtime 须关闭。 |
| `runtime-benchmarks` | 否 | `register` 的基准测试（随分片数线性增长）。 |
| `try-runtime` | 否 | SDK 的 try-runtime 支持。 |

## 示例

英文版 README 中的示例作为 doctest 运行：计算 `Qwen2.5-0.5B-Instruct`（`qwen2`、INT4、两个分片）清单的模型 ID，并与公布的回归向量比对。
