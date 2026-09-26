> 🌐 [English](mvp-technical-plan.md) | **简体中文**

# AgentCoin MVP 技术方案 v0.1

> 状态：初版，待评审。日期：2026-09。
> 依据：`docs/decisions.md`（D1–D37）。全量版方案见 `full-technical-plan.md`。
> 约定：“**[预留]**” 表示 MVP 不实现，但接口或数据结构必须在 MVP 阶段就定义好，供全量版使用。

---

## 0. 目标与范围

### 0.1 MVP 要跑通的完整故事

> 用户用 PQ 钱包持有 ATC，存入屏蔽池，铸造匿名推理额度。然后通过兼容 OpenAI 的 API，匿名调用 DeepSeek / Qwen 等开源模型，获得流式输出。Agent 用同样的额度按次付费。数据中心和消费级 GPU 提供者无许可注册、质押、接单，获得费用和排放。审计员以“神秘顾客”方式抽查服务质量，作弊者被罚没。空闲算力去做 DAO 公共任务。开发者用 Solidity 部署 DApp。整条链的签名和加密全部抗量子。

### 0.2 分期

| 版本 | 网络 | 内容 |
|---|---|---|
| **α** | 测试网 | 链核心、PQ 账户、代币经济、EVM、推理市场（透明额度）、网关、审计、公共任务（精简）、PoA |
| **β** | 测试网 → 审计 | 屏蔽池 + 匿名推理凭证、代币投票治理、PoA → PoS 切换逻辑、护栏 |
| **主网 Beta** | 主网 | β 通过外部审计后上线；**主网只有匿名凭证这一种付费路径**（D20） |

### 0.3 不在 MVP 范围内（见全量版）

TEE 机密层、训练和 RL、存储层、L1 私有功能（投票、竞价、兑换）、工作加权选举、两院制、混合网络、跨链桥、稳定币支付、500ms 出块、STARK 签名聚合。

### 0.4 成功标准（主网 Beta）

| 指标 | 目标 |
|---|---|
| 出块 / 最终性 | 1s / ≤3s（P95） |
| 首 token 延迟的额外开销（相对直连提供者） | ≤150ms（P50） |
| 推理吞吐 | 与提供者直连持平（链不在数据路径上） |
| 审计 | 每个提供者每小时至少被“神秘顾客”抽查 1 次；换模型、降精度的作弊检出率 ≥99% |
| 安全 | 零 sudo；节点不变式全部启用；匿名凭证电路通过外部审计 |

---

## 1. 总体架构

```
┌──────────────────────────────────────────────────────────────────────────┐
│ 客户端层                                                                  │
│  ac-wallet (CLI)  ·  ac-sdk (Rust 核心 + 绑定)  ·  OpenAI 兼容客户端 / Agent 框架 │
└───────────────┬─────────────────────────────────────┬────────────────────┘
                │ 链上交易（PQ 签名）                  │ HTTPS 推理请求（附额度凭证）
                ▼                                     ▼
┌───────────────────────────────┐     ┌──────────────────────────────────────┐
│ AgentCoin 节点 (ac-node)       │     │ 推理网关 (ac-gateway)，无许可、需质押    │
│  ├ 共识：Aura-PQ + AC-BFT      │◄────┤  OpenAI API · 流式 · 路由 · 凭证核验    │
│  ├ 节点不变式检查器（宪法第1层）│ 结算 │  不记日志 · 批量结算                    │
│  ├ Runtime (WASM)              │     └───────────────┬──────────────────────┘
│  │  ├ 系统 / PQ 账户 / 费用销毁 │                     │ 转发（提供者看不到付款人）
│  │  ├ 排放 / 金库 / PoA→PoS    │                     ▼
│  │  ├ 模型注册 / 提供者注册     │     ┌──────────────────────────────────────┐
│  │  ├ Credit（透明 | 屏蔽）    │     │ 提供者代理 (ac-provider)               │
│  │  ├ 工作结算 / 审计 / 罚没    │◄────┤  包装 vLLM / SGLang · TOPLOC 证明      │
│  │  ├ 公共任务 / 参考汇率       │ 注册 │  回执签名 · 心跳                      │
│  │  ├ 治理 / 护栏              │     └──────────────────────────────────────┘
│  │  └ pallet-revive (EVM) + PQ 预编译                ▲
│  └ eth-RPC 适配器             │     ┌────────────────┴─────────────────────┐
└───────────────────────────────┘     │ 审计员代理 (ac-auditor)                │
                ▲                     │  神秘顾客请求 · TOPLOC 复核 · 判定上链  │
                └─────────────────────┴──────────────────────────────────────┘
```

**核心原则**：链**不在**推理的数据路径上。推理走 HTTP 流，链只负责注册、额度、结算、审计和罚没。这对应 JAM 的 refine（链下）/ accumulate（链上）模式。

---

## 2. 技术栈

| 层 | 选型 | 说明 |
|---|---|---|
| 语言 | **Rust 为主要开发语言**（D31）：链、runtime、共识、密码学、电路、网关、提供者代理、审计员代理、钱包、SDK 核心全部用 Rust | 唯一例外：vLLM / SGLang 引擎内部用于提取 TOPLOC 激活值的**薄 Python 插件**（约 200 行，见 §2.1） |
| 链框架 | **Polkadot SDK**（最新的 stable 发布版），solochain 模板 | 无分叉 runtime 升级 |
| EVM | `pallet-revive`（REVM 后端） | Solidity 工具链兼容 |
| PQ 签名 | ML-DSA-44 / ML-DSA-65（FIPS 204）；预留 SLH-DSA（FIPS 205）、FN-DSA（Falcon） | RustCrypto `ml-dsa`（M0 选定，纯 Rust、支持 no_std）；以 NIST ACVP 向量验证 |
| PQ 加密 | ML-KEM-768 + X25519 混合（X-Wing 式组合） | 用于 P2P 传输、网关 TLS 之外的端到端加密、屏蔽池笔记加密 |
| 哈希 | BLAKE3（链上通用）、SHA3-256（互操作）、Poseidon2（电路内） | 输出一律 256 位 |
| ZK（β） | 基于 FRI 的 STARK（候选：Plonky3 / Stwo / Winterfell，第 7 个月做选型基准测试） | **禁止使用** Groth16 / KZG / BN254 / BLS12 |
| P2P | libp2p（Polkadot SDK 自带）+ PQ 混合握手 | 见 §3.6 |
| 推理引擎 | vLLM / SGLang（提供者自选） | |
| 可验证推理 | TOPLOC | |
| 网关 | Rust（axum + tokio），SSE 流式 | |

### 2.1 Rust 优先原则与边界（D31）

| 组件 | 语言 | 说明 |
|---|---|---|
| 节点、runtime、pallet、共识、不变式检查器 | Rust | Polkadot SDK 原生 |
| `ac-crypto`、STARK 电路（Plonky3 / Stwo / Winterfell 都是 Rust） | Rust | 同一份代码同时编译成 native、WASM runtime 和客户端 WASM |
| 网关、提供者代理、审计员代理、eth-RPC 适配器 | Rust（tokio + axum） | |
| 钱包、SDK 核心 | Rust | Python / TS 只是绑定 |
| TOPLOC | Rust 移植（`ac-toploc`）：证明的编码、校验、比对 | 审计员的复核逻辑是纯 Rust |
| 推理引擎 | 不自研；调用 vLLM / SGLang 的 OpenAI 兼容接口 | 引擎本身是第三方 Python 程序 |
| **TOPLOC 激活值提取** | **薄 Python 插件**，挂在推理引擎内部，只负责把隐藏层激活值交给 Rust 代理（通过本地 socket） | TOPLOC 需要读取模型内部的激活值，只能在引擎进程里完成；插件保持极小、无业务逻辑 |
| **[可选] Rust 原生推理后端** | mistral.rs / candle | T2 消费级节点可选纯 Rust 路线，此时不需要 Python 插件；性能不及 vLLM 时仍推荐使用 vLLM |

工程约定：Rust stable 工具链（Polkadot SDK 要求的版本）；`#![forbid(unsafe_code)]` 为默认（密码学库和 FFI 例外，需逐条注明原因）；`cargo fmt` + `clippy -D warnings` + `cargo deny`（许可证与漏洞）+ `cargo audit` 进 CI；runtime 与 pallet 保持 `no_std` 兼容。

---

## 3. 密码学抽象层（`ac-crypto`）——可插拔的核心

### 3.1 设计原则

1. **所有**签名、公钥、密文、证明都带 `AlgId` 前缀；不存在“隐式算法”。
2. 验证逻辑放在 runtime（可以无分叉升级）；性能敏感的原语通过 host function 暴露（新增算法需要节点升级，但不需要硬分叉）。
3. 账户和算法解耦：账户 ID 不随换钥改变。
4. 新增算法 = 新增一个 `AlgId` 变体 + 一个实现 + 一次 runtime 升级，**不改变任何已有数据**。

### 3.2 数据结构

```rust
// 每个类别有独立的 1 字节 AlgId 空间；AlgId 同时就是 SCALE 枚举序号（D34）。
// 0x00 不分配；0xFF 在每个类别中保留为扩展标记。
#[repr(u8)]
pub enum SigAlg {
    MlDsa44   = 0x01,  // MVP 默认（用户账户）
    MlDsa65   = 0x02,  // MVP（验证者 / 高价值账户）
    MlDsa87   = 0x03,
    SlhDsaSha2_128s = 0x10,  // [预留] 纯哈希，作为格密码被攻破时的后备
    FnDsa512  = 0x20,        // [预留] Falcon，签名更小
    XmssLean  = 0x30,        // [预留] 共识签名 + STARK 聚合（全量版）
}

#[repr(u8)]
pub enum KemAlg { XWing /* ML-KEM-768 + X25519, draft-06 */ = 0x01, MlKem1024 = 0x02 }

// 带标签类型是枚举：变体序号 = AlgId，载荷 = 定长字节。
// 规范编码 = SCALE 编码 = 链上存储编码 = TypeInfo 描述的格式。
pub enum PqPublicKey { #[codec(index = 0x01)] MlDsa44(Box<[u8; 1312]>), /* 0x02, 0x03 … */ }
pub enum PqSignature { #[codec(index = 0x01)] MlDsa44(Box<[u8; 2420]>), /* 0x02, 0x03 … */ }

/// 账户 ID：32 字节，与算法无关、换钥后保持不变
/// 创建时： AccountId = BLAKE3-derive_key("agentcoin 2026-09 account-id v1", alg_id ‖ pk)
pub struct AccountId([u8; 32]);
```

### 3.3 交易签名方案

- **v5 General 交易 + `PqAuthorize` 交易扩展**（D36）。该扩展位于扩展流水线的首位，验证 ML-DSA 签名（上下文 `agentcoin/tx/v1`），签名对象为 `derive_key("agentcoin 2026-09 tx-payload v1", SCALE(继承的隐含数据))`，即扩展版本、调用以及其后所有扩展的显式与隐式数据（spec 版本、交易格式版本、创世哈希、有效期、nonce）。SDK 的 `Verify` trait 只拿得到账户 ID、拿不到链上状态，无法查登记表；因此旧式 `Signed` 交易的签名位置用一个不可能有值的类型（`NoClassicSignature`）封死，任何经典签名算法都无法授权账户。
- **公钥注册表**（`pallet-pq-accounts`）：ML-DSA 公钥有 1.3KB，不适合每笔交易都携带。账户的首笔交易携带公钥（该公钥必须派生出所声明的账户 ID），交易被执行时完成登记；之后的交易只带 `AccountId` 和签名，由链上查找公钥。每笔交易因此节省约 1.3KB。
- **换钥交易**：`rotate_key(new_pk, proof)`，由当前密钥通过 `PqAuthorize` 授权；`proof` 是新密钥（上下文 `agentcoin/key-rotation/v1`）对（创世哈希、账户、轮换计数、新公钥）的签名。账户 ID 不变，一个公钥只能属于一个账户。以后迁移到新算法就是用这个交易。
- **防重放**：nonce + genesis hash + runtime 版本号都纳入签名载荷（Substrate 标准做法）。

### 3.4 EVM 集成（D13：100% PQ）

- **禁用**以太坊原生交易格式（RLP + secp256k1）。`pallet-revive` 只接受链原生 extrinsic。
- EVM 地址 = `AccountId` 的某个 20 字节投影（沿用 `pallet-revive` 的账户映射机制）；合约中需要唯一性的场景（例如 CREATE2 冲突检测）以 32 字节为准。
- **预编译**：
  - `0x…0A01 pq_verify(alg, pk, msg, sig) -> bool`：合约内做 PQ 验签；
  - `0x…0A02 blake3(data)`、`0x…0A03 poseidon2(data)`；
  - **[预留]** `0x…0A10 stark_verify(vk_id, proof, public_inputs)`：为 L2 通用私有合约预留（D27）。
- `ecrecover` 保留，但只作为应用层工具，**不能**用于账户授权。
- **eth-RPC 适配器**（`ac-eth-rpc`）：对外暴露以太坊 JSON-RPC（`eth_call`、`eth_getLogs`、`eth_estimateGas` 等只读接口）；`eth_sendRawTransaction` 改为接收“未签名的交易 + 外部 PQ 签名”，由 `ac-wallet` 作为 Foundry / Hardhat 的外部签名器。

### 3.5 共识密钥

| 用途 | 算法 | 说明 |
|---|---|---|
| 出块（Aura-PQ） | ML-DSA-65 | 每块 1 个签名 |
| 最终性投票（AC-BFT） | ML-DSA-65 | 逐个广播；100 个验证者 × 2 轮 × 1 块/s ≈ 660KB/s |
| 会话密钥轮换 | 每个 era | 沿用 `pallet-session` 的流程，密钥类型换成自定义的 PQ 类型 |

### 3.6 网络层

- libp2p Noise 握手加入 **ML-KEM-768 + X25519 混合**密钥交换，防止“先截获、后解密”。
- 节点身份密钥用 ML-DSA-65。这需要修改 libp2p 的身份层；**如果 α 阶段工作量过大，允许先用 Ed25519 节点身份 + 混合 KEM 加密**。节点身份只关系到路由，不涉及资产，风险可控。必须在 β 阶段之前完成替换。

### 3.7 链上哈希（D35）

- **所有完整性承诺都使用 BLAKE3-256**：区块头哈希（即区块 ID）、父区块引用、外部交易根、状态根以及状态证明中的节点。链的 `Hashing` 类型为 `ac_primitives::Blake3Hasher`，它只是 `ac-crypto` 的一层适配；runtime 内计算的树根直接使用它，因为 SDK 只以 host function 形式提供 BLAKE2 和 Keccak 的树根计算。
- **存储键哈希器是有界的例外**：SDK 自带模块（`frame-system`、`pallet-balances` 等）用 128 位的 `Blake2_128Concat` / `Twox` 哈希器排列存储键。这些哈希只负责把键分散到树中，不承担完整性承诺（完整性由 BLAKE3 状态根保证），因此保留。AgentCoin 自有模块以账户为键的映射使用 `Identity`：账户 ID 本身就是 BLAKE3 输出。
- **随创世固定**：哈希方案通过 `ChainProfileApi` runtime API 对外声明；更换哈希方案意味着新链或硬分叉，不能通过普通 runtime 升级完成。

---

## 4. 共识

### 4.1 α 版：Aura-PQ + AC-BFT

- **出块**：`sc-consensus-aura` 的分叉版，把 `AuthorityId` 换成 ML-DSA-65 应用密钥；1s 时隙，按验证者列表轮换，**不使用 VRF**（目前没有标准化的 PQ VRF）。
- **最终性：AC-BFT**，一个自研的 HotStuff-2 式 finality gadget，作为节点客户端组件运行：
  - 每个区块由验证者进行 2 轮投票（prepare 和 commit），达到 ≥2/3 权重后调用 `client.finalize_block()`；
  - 流水线化：每个块的 commit 轮与下一个块的 prepare 轮合并；
  - 超时后更换视图，采用指数退避；
  - 投票消息 = `(round, block_hash, height, validator_id, PqSignature)`；最终性证明 = ≥2/3 投票的集合，可以被轻客户端验证；
  - **不用 GRANDPA** 的原因：GRANDPA 的签名类型在 `sp-consensus-grandpa` 中写死为 Ed25519，改造成本接近重写，而且它的最终性延迟偏高。
- **规模**：MVP 验证者 ≤100 个。
- **罚没证据**：同一高度的双签、同一轮的双投，都可以提交链上证据，罚没 100% 质押。

### 4.2 链上随机数（审计抽样用）

- `pallet-randomness-cr`：验证者每个 epoch 先提交 `H(secret)`，下个 epoch 再揭示 `secret`。随机数 = `BLAKE3(所有揭示值)`。不揭示的验证者会被扣除排放。
- 已知的弱点：最后一个揭示者可以选择不揭示，从而对结果产生 1 比特的影响。对审计抽样来说可以接受。全量版加入基于哈希的 VDF。

### 4.3 PoA → PoS 自动切换（D19）

```
pallet-validator-set 的状态机：
  Bootstrap(PoA)  ──[条件全部满足，且在下一个 era 边界]──►  PoS
条件：
  (a) Σ 已质押 ≥ 10% × 流通量
  (b) 满足质押门槛的候选验证者数 ≥ N_min（创世参数，建议 21）
  (c) 距离创世 ≥ T_min（防止被瞬间满足，建议 30 天）
PoA 阶段：验证者安全预算不发放，计入滚存储备；PoA 验证者名单由创世配置 + 多签增删
```

- 切换逻辑同时写在 runtime 中和**节点不变式检查器**中（§6），任何 runtime 升级都无法绕过。
- PoS 阶段的 MVP 选举：按质押排序取前 K 名（简单 NPoS 的替代）；**[预留]** `ValidatorElection` trait 的输入是 `(stake, workscore)`，全量版替换为工作加权选举。

---

## 5. Runtime 模块（pallets）

| Pallet | 版本 | 职责 |
|---|---|---|
| `pallet-pq-accounts` | α | 公钥注册、换钥、`PqAuthorize` 交易授权 |
| `pallet-emission` | α | 计划排放、滚存储备、四份分配、按 epoch 结算 |
| `pallet-treasury-dual` | α | 社区赠款 + 持币人金库；5% 保底线性归属 |
| `pallet-fee-burn` | α | 交易费和推理费按比例销毁 |
| `pallet-validator-set` | α | PoA / PoS 状态机、会话密钥、罚没 |
| `pallet-randomness-cr` | α | 承诺-揭示随机数 |
| `pallet-model-registry` | α | 模型登记、血统声明 |
| `pallet-providers` | α | 提供者注册、层级、质押、价格、心跳 |
| `pallet-gateways` | α | 网关注册、质押、托管额度 |
| `pallet-credits` | α / β | `Credit` trait；透明实现（α）+ 屏蔽实现（β） |
| `pallet-work` | α | 工作报告（批量回执）→ 结算 → 工作分 |
| `pallet-audit` | α | 审计员注册、任务派发、判定、争议、罚没 |
| `pallet-public-jobs` | α（精简） | DAO 公共任务队列 |
| `pallet-ref-rate` | α | ATC / USD 参考汇率（治理设定，带调整幅度上限） |
| `pallet-guardrails` | β | 参数边界检查（宪法第 2 层） |
| `pallet-constitution` | β | 宪法文本哈希、修改流程 |
| `pallet-referenda` + `pallet-conviction-voting` | β | 代币院治理（Polkadot SDK 自带模块，配置轨道） |
| `pallet-shielded` | β | 屏蔽池：笔记承诺树、nullifier、STARK 验证 |
| `pallet-revive` | α | EVM |

下面列出关键模块的细节。

### 5.1 `pallet-emission`

```rust
// 常量（创世写死，同时也是节点不变式的输入）
const CAP: u128            = 21_000_000 * 10u128.pow(18);
const ERA_YEARS: u32       = 4;
const EPOCH: BlockNumber   = 3_600;                 // 1s 出块时约 1 小时
const EPOCHS_PER_HALVING: u64 = 4 * 365 * 24 + 24;  // 约 4 年（35,064 个 epoch）
const FIRST_HALVING_TOTAL: u128 = CAP / 2;          // 第一个 4 年期共 1050 万

fn scheduled(epoch: u64) -> u128 {                  // 本 epoch 的计划额
    let n = epoch / EPOCHS_PER_HALVING;             // 第几次减半
    (FIRST_HALVING_TOTAL >> n) / EPOCHS_PER_HALVING as u128
}

// 存储
Minted: u128                      // 已铸造总量
Reserve: u128                     // 滚存储备（计划额中未排出的部分）
EpochWork: { market_fee_usd, market_units, public_units }

// 每个 epoch 结束时（on_initialize 钩子）：
S  = scheduled(e)
avail = S + min(Reserve, S)               // 储备每 epoch 最多动用 1 倍计划额
security = 0.10 * S （PoA 阶段 → 计入 Reserve）
market   = min(0.50 * avail, k * 已验证付费(ATC 计))
public   = min(0.20 * avail, 已验证的公共任务预算消耗)
treasury = max(0.05 * S, (0.20 / 0.70) * (market + public))  // 与工作排放成比例（满载时恰为 20%），不低于保底
minted_now = min(security + market + public + treasury, avail)   // 超出时按比例缩减
Reserve  += S - (minted_now - 从储备动用的部分)
assert!(Minted + minted_now <= CAP)       // runtime 层检查（节点层会再检查一次）
```

- **说明**：`treasury` 与实际的工作排放成比例，需求满载时恰好是 10/50/20/20 中的 20%，需求不足时按比例缩小（研究 04 §2），但不低于保底 `0.05 × S`。保底部分（即比例部分不足保底时补足的那一段）进入归属锁定账户，线性释放 2 年，只能用于审计和冷启动。
- `k`（排放倍数）和销毁比例 `b` 受护栏约束：`0 ≤ k − b ≤ 0.5`（§研究 03 §1.5）。

### 5.2 `pallet-credits`：统一额度接口（D20）

```rust
pub trait Credit {
    type Voucher: Encode + Decode;      // 一次性支付凭证
    type Batch:   Encode + Decode;      // 批量兑换数据

    /// 网关在链下核验凭证（不上链，毫秒级）
    fn verify_offchain(v: &Self::Voucher, ctx: &RequestCtx) -> Result<Amount, Error>;
    /// 网关批量上链兑换：防双花 + 转账给网关的结算账户
    fn redeem_batch(gateway: AccountId, batch: Self::Batch) -> DispatchResult;
}

// α 版：透明实现
TransparentVoucher { account, gateway, nonce, max_amount_usd, expiry, sig: PqSignature }
  // 用户账户在 pallet-gateways 中向某个网关托管预付额度；凭证由用户签名
  // 网关批量兑换时，从用户托管额扣款

// β 版：屏蔽实现
ShieldedVoucher { nullifier, value_commitment, gateway_id, expiry, stark_proof }
  // 从屏蔽池的笔记中花费；nullifier 防双花；网关和提供者都无法得知来源
```

- 市场、网关、结算模块**只依赖 `Credit` trait**。β 版上线屏蔽实现时，市场代码零改动。
- 主网创世时，**透明实现不开放给推理付费**（D20 第 4 条）。

### 5.3 `pallet-providers` 和 `pallet-model-registry`

```rust
Model {
    id: H256,                          // = BLAKE3(规范化后的权重清单)
    name, arch, quant: QuantType,      // 例如 FP8 / INT4-AWQ
    shard_hashes: BoundedVec<H256>,    // 每个权重分片的哈希
    license_tag: LicenseTag,           // 仅作信息展示，协议不校验（D6）
    lineage: Option<(H256, LineageKind)>,  // 父模型 + 派生方式（D28）
    royalty: Option<RoyaltySpec>,      // [预留] 社区模型分润（D22），全量版启用
}

Provider {
    owner: AccountId, tier: Tier /* T1 | T2 | [预留] T0 */,
    endpoint: BoundedVec<u8>,          // 网关用于连接的地址（可以是中继地址）
    models: BoundedVec<(ModelId, PriceUsd /* 每百万 token，输入和输出分开计价 */)>,
    stake: Balance, status: Active | Jailed | Exiting,
    kem_pk: (KemAlg, Bytes),           // 网关到提供者的端到端加密
    metrics: SlaMetrics,               // 由审计员测量写入
    [预留] attestation: Option<AttestationRef>,   // T0 TEE
}
```

- **最低质押**（以 USD 等值计，按参考汇率换算）：T1 为 $1,000，T2 为 $100（草案）。
- **零质押入口**：没有质押的新节点只能领取 `pallet-public-jobs` 的任务（研究 05 B3）。

### 5.4 `pallet-work`：按 refine / accumulate 模式结算

1. **Refine（链下）**：提供者每完成一次请求，就生成一条回执：
   ```
   Receipt {
     gateway, provider, model_id, voucher_ref,
     in_tokens, out_tokens, price_usd,
     toploc_commit: H256,
     t_first_token, t_done,
     sig_provider, sig_gateway,
   }
   ```
   双方签名，由网关聚合。
2. **Accumulate（链上）**：网关每个 epoch 提交一次 `WorkReport { merkle_root(receipts), totals, credit_batch }`：
   - 调用 `Credit::redeem_batch` 兑换额度；
   - 按回执给提供者结算费用（扣除网关费率和销毁部分）；
   - 更新提供者的工作分（`EpochWork`）；
   - 进入**挑战期**（例如 2 个 epoch）；挑战期内审计员可以提交欺诈证明，挑战期结束后排放才发放。
3. 回执原文由网关保留到挑战期结束后再删除，不上链；**prompt 和输出永不上链**。

### 5.5 `pallet-audit`：神秘顾客审计（兼顾隐私）

**问题**：TOPLOC 复核需要 prompt 和输出，但审计真实用户请求会让审计员看到用户内容。

**方案：只审计审计员自己发起的请求。**
1. 每个 epoch，链上随机数为每个提供者抽选 m 个审计员。
2. 审计员通过普通网关（或中继）**以普通用户身份**发起请求，使用自己的额度和凭证，并用来自公开题库或随机生成的 prompt。提供者**无法区分**这是审计还是真实用户。
3. 审计员拿到输出和 TOPLOC 证明后，在本地（T1/T2 审计员）用声明的模型复算 prefill，比对 TOPLOC 证明，并测量延迟指标。
4. 提交判定 `Verdict { provider, model_id, pass | fail(evidence), metrics }`：
   - `fail` 需要附带证据（输出、TOPLOC 证明、签名回执），任何人都可以复核；
   - 同一提供者被 ≥2 个独立审计员判定为 `fail` → 进入争议期 → 升级为 5 个审计员复核 → 确认后**罚没质押的 x%** 并禁用（jail）。提供者也可以在争议期内申诉。
5. 审计员奖励来自市场工作排放；如果审计员的判定被推翻，罚没审计员的质押。

**效果**：用户内容永远不会进入审计流程；提供者既然无法区分审计和真实请求，就只能对所有请求都诚实服务。

### 5.6 `pallet-public-jobs`（精简版）

- 任务类型（MVP）：模型评测（跑公开基准并提交分数）、数据清洗和去重、嵌入计算。
- 验证方式：冗余执行（同一任务派发给 3 个节点，多数一致即通过）+ 抽样复算。
- 预算：公共工作排放份额（20%）；任务由持币人金库或治理发布。**[预留]** 全量版接入训练任务。

### 5.7 `pallet-ref-rate`（D29）

- 存储 `ATC_per_USD`；只能通过治理轨道修改；**每次调整幅度 ≤ ±20%**，间隔 ≥ 1 天（由护栏强制）。
- **[预留]** `PriceSource` trait：全量版接入多源预言机（中位数 + TWAP）。

### 5.8 `pallet-shielded`（β）

- **笔记** = `(value, owner_pk_hash, rho, rcm)`；承诺 = `Poseidon2(...)`；存放在深度 32 的增量 Merkle 树中。
- **花费**：STARK 证明“我知道某个存在于树中的笔记，它的 nullifier = PRF(sk, rho)，并且金额守恒”。
- **笔记加密**：ML-KEM-768 + X25519 混合，接收方用查看密钥（view key）扫描。
- **凭证铸造**：从笔记中花费，同时生成 N 个一次性推理凭证。每个凭证都是一张绑定特定网关、金额上限和有效期的小额笔记。
- **证明大小与性能**：预计 50–200KB 每个证明，客户端生成需要 1–10 秒。**优化手段**：一次铸造一批凭证（摊薄证明成本）；网关在链下验证后，每个 epoch 批量上链。
- **[预留]** `ShieldedAction` 枚举：`Transfer | MintVoucher | Vote | Bid | Swap`，全量版加入 L1 功能。

### 5.9 `pallet-guardrails`（β）

```rust
// 每个受治理的参数都登记一条边界规则
Guardrail { param: ParamId, min, max, max_step_per_change, min_interval }
// 例：TreasuryShare ∈ [0, 20%]；RefRate 单次调整 ≤ ±20%；BurnRatio ∈ [10%, 90%]
```

- 所有参数修改都要经过 `guardrails::check()`；runtime 升级必须附带 `constitution_version`，与链上宪法哈希匹配后才能执行。

---

## 6. 节点不变式检查器（宪法第 1 层，D30）

写在节点客户端（native 代码）里，**不在 runtime 里**，因此任何 runtime 升级都无法修改它。

```rust
// ac-node/src/invariants.rs —— 每个区块导入时，在执行后调用
fn check_block(pre: &State, post: &State, header: &Header) -> Result<(), Reject> {
    let issued = read_well_known(post, TOTAL_ISSUANCE_KEY)?;   // 键不存在或格式错误 → 拒绝（fail-closed）
    ensure!(issued <= CAP);                                     // 宪法 1：2100 万
    let minted = issued - read_well_known(pre, TOTAL_ISSUANCE_KEY)? + burned_in_block(post)?;
    ensure!(minted <= max_mint_allowed(header.number, pre));    // 宪法 3：无预挖、不超出排放曲线
    ensure!(poa_switch_respected(pre, post, header));           // 宪法 4：PoA → PoS
    Ok(())
}
```

- 创世时，`TOTAL_ISSUANCE_KEY` 等“约定存储键”写入节点代码。如果 runtime 升级后读不到这些键，节点会**拒绝**该区块（fail-closed）。
- 修改这些规则 = 发布新的节点客户端 = **硬分叉**。
- “协议中立”（宪法 2）属于第 2 层，在客户端中另外检查：runtime 中不得出现名为 `Blacklist`、`Blocklist`、`GeoFence` 等的存储前缀（这只是辅助检查，主要靠第 2 层审议）。

---

## 7. 链下组件

| 组件 | 职责 | 关键点 |
|---|---|---|
| **ac-gateway** | OpenAI 兼容 API（`/v1/chat/completions`、`/v1/models`）；SSE 流式；凭证核验；按价格、延迟、信誉路由；故障自动切换；回执聚合与批量结算 | **无许可，任何人都可以运行**，需要质押；**不记录请求日志**（开源代码 + 公开承诺；用户可以选择自建网关）；网关 → 提供者之间用 ML-KEM 混合加密 |
| **ac-provider** | 包装 vLLM / SGLang；生成 TOPLOC 证明；签名回执；心跳；自动注册 | 一条命令启动：`ac-provider --model Qwen/... --tier T2` |
| **ac-auditor** | 按链上派发执行神秘顾客请求；本地复算 TOPLOC；提交判定 | 可以运行在 T1 / T2 / T3 节点上（T3 只测延迟和一致性） |
| **ac-wallet** | CLI 钱包：生成和管理 PQ 密钥、转账、质押、换钥；β 版加入屏蔽池和凭证；作为 Foundry 外部签名器 | 助记词 → 种子 → ML-DSA 确定性密钥派生（遵循 FIPS 204 的种子生成接口） |
| **ac-eth-rpc** | 以太坊 JSON-RPC 适配 | §3.4 |
| **ac-sdk** | **Rust 核心**（凭证管理、PQ 签名、STARK 证明生成、链交互）；通过 PyO3 生成 Python 绑定、通过 wasm-bindgen 生成浏览器 / TS 绑定，绑定层只做薄封装 | 密码学只有一份 Rust 实现，避免多语言实现不一致 |
| **区块浏览器** | 最小化版本 | 可以用 Substrate 生态的开源浏览器改造 |

### 7.1 一次推理请求的完整流程（β 版）

```
1. 客户端 SDK 从本地凭证池取一张凭证 V（绑定网关 G，金额上限 ≥ 预估费用）
2. POST https://G/v1/chat/completions  Header: X-AC-Voucher: V
3. G 在链下验证 V（STARK 验证约几毫秒，并检查 nullifier 未被使用）
4. G 按路由策略选择提供者 P，用 ML-KEM 混合加密转发请求（P 只知道请求来自 G）
5. P 流式返回 token；G 转发 SSE 给客户端
6. 请求完成：P 签名回执 R（token 数、TOPLOC 承诺）；G 联签；多付部分找零为新凭证，返还客户端
7. 每个 epoch：G 提交 WorkReport（回执 Merkle 根 + 凭证批次）→ 链上兑换、结算、进入挑战期
8. 挑战期结束：P 获得费用（扣除网关费率和销毁部分）+ 市场工作排放
```

---

## 8. 代币与费用参数（MVP 默认值）

| 参数 | 值 | 是否受护栏约束 |
|---|---|---|
| 总量 | 21,000,000 ATC（18 位小数） | 宪法第 1 层 |
| 第一个 4 年期计划排放 | 10,500,000，之后每 4 年减半 | 宪法第 1 层 |
| 储备动用上限 | 每 epoch 1 倍计划额 | 护栏 [0.5, 2] |
| 分配 | 10 / 50 / 20 / 20（安全 / 市场 / 公共 / 金库） | 护栏；金库 ≤ 20% |
| 金库保底 | 5% 计划额，线性锁定 2 年 | 护栏 |
| 推理费销毁比例 `b` | 20% | 护栏 [10%, 90%] |
| 排放倍数 `k` | 0.5 | 护栏：`k − b ≤ 0.5` |
| 网关费率上限 | 5% | 护栏 |
| 训练者分润 | 5%（全量版启用） | 护栏 [0, 10%] |
| 交易费 | 按权重计费，80% 销毁、20% 给出块者 | 护栏 |
| 出块时间 | 1s | runtime 常量 |
| 挑战期 | 2 个 epoch | 护栏 |

---

## 9. 仓库结构

```
agentcoin/
├── docs/
│   └── decisions.md · research/ · design/ · rust-guidelines/
├── openspec/                   # SDD：已归档规范 specs/ + 本地变更 changes/
├── crates/
│   ├── ac-crypto/              # AlgId、PQ 签名 / KEM 封装、测试向量
│   ├── ac-primitives/          # AccountId、Receipt、Voucher 等共享类型
│   ├── ac-invariants/          # 节点不变式（纯函数，可以单独做形式化验证）
│   └── ac-toploc/              # TOPLOC 的 Rust 绑定 / 移植
├── node/                       # ac-node：Aura-PQ、AC-BFT、不变式检查器、PQ libp2p
│   └── consensus/{aura-pq, ac-bft}/
├── runtime/                    # WASM runtime 组装
├── pallets/
│   ├── pq-accounts/ emission/ treasury-dual/ fee-burn/ validator-set/
│   ├── randomness-cr/ model-registry/ providers/ gateways/ credits/
│   ├── work/ audit/ public-jobs/ ref-rate/ guardrails/ constitution/
│   └── shielded/               # β
├── circuits/                   # β：STARK 电路（笔记花费、凭证铸造）
├── services/
│   ├── gateway/ provider/ auditor/ eth-rpc/
├── clients/
│   ├── wallet-cli/ sdk/ (Rust 核心) sdk-bindings/{py (PyO3), wasm (wasm-bindgen)}/
├── contracts/                  # 示例 Solidity 合约 + PQ 预编译接口
├── tests/
│   ├── e2e/                    # zombienet 式多节点测试
│   └── sim/                    # 经济模型仿真（排放、自刷、冷启动）
└── .github/workflows/          # CI：fmt、clippy、单元测试、runtime 构建、e2e
```

---

## 10. 里程碑与验收标准

> 按“一人 + AI”估算。每个里程碑都以**可运行的演示 + 自动化测试**为验收标准。

| 里程碑 | 时间（月） | 交付 | 验收标准 |
|---|---|---|---|
| **M0 基础** | 0–1 | 仓库骨架、CI、solochain 模板跑通、`ac-crypto`（ML-DSA、ML-KEM 混合，含 NIST 测试向量） | CI 全绿；测试向量 100% 通过 |
| **M1 PQ 链** | 1–3 | `pallet-pq-accounts`、`PqAuthorize` 交易授权、Aura-PQ、换钥交易、CLI 钱包 | 3 节点本地网出块；ML-DSA 签名的转账成功；换钥后账户 ID 不变 |
| **M2 最终性** | 3–5 | AC-BFT、罚没证据、承诺-揭示随机数 | 4–10 节点：最终性 P95 ≤3s；杀掉 1/3 节点后恢复；双签被罚没 |
| **M3 经济** | 4–6 | 排放、金库、费用销毁、validator-set 状态机、**节点不变式** | 仿真 8 年排放与公式误差为 0；构造超发的 runtime 升级被节点拒绝；PoA → PoS 在条件满足时自动切换 |
| **M4 EVM** | 5–7 | `pallet-revive`、PQ 预编译、eth-RPC 适配器、Foundry 外部签名器 | 用 Foundry 部署并调用 ERC-20 和 Uniswap-V2 式合约 |
| **M5 推理市场** | 6–9 | 模型和提供者注册、网关（透明额度）、provider 代理、工作结算 | 端到端：OpenAI SDK 调用 Qwen，流式输出；首 token 额外开销 ≤150ms；回执批量结算正确 |
| **M6 审计** | 8–10 | 审计员代理、神秘顾客、TOPLOC 复核、争议与罚没、公共任务（精简） | 故意换模型 / 降精度的提供者在 1 小时内被检出并罚没；诚实提供者误判率 <0.1% |
| **🚩 α 测试网** | 10 | 公开测试网 + 文档 + 水龙头 | 外部提供者能在 30 分钟内接入 |
| **M7 屏蔽池** | 9–14 | STARK 选型基准 → 电路 → `pallet-shielded` → 凭证铸造和兑换 → 钱包 / SDK 集成 | 凭证全流程跑通；客户端证明 ≤10s；链上验证 ≤50ms；**电路内部审查 + 模糊测试** |
| **M8 治理与宪法** | 12–15 | 代币投票（referenda + conviction）、护栏、宪法 pallet、移除 sudo 的流程 | 越界参数修改被拒绝；升级强制延迟生效 |
| **🚩 β 测试网** | 15 | 完整 MVP | 30 天稳定运行；激励测试（邀请外部矿工） |
| **M9 审计** | 15–18 | 外部安全审计（电路、共识、经济），修复问题 | 无未修复的严重 / 高危问题 |
| **🚀 主网 Beta** | ≈18 | 创世（PoA，无预挖），网关只接受匿名凭证 | §0.4 的全部指标达标 |

**关键路径**：M1 → M2 → M5 → M7 → M9。M7（STARK 电路）风险最高，因此 M7 与 M5 / M6 并行启动，并在第 9 个月前完成选型。

---

## 11. 主要风险与对策

| 风险 | 影响 | 对策 |
|---|---|---|
| PQ 签名库不成熟或存在侧信道 | 私钥泄露 | 选用经过测试向量验证的实现；验证者使用常数时间实现；钱包预留 SLH-DSA 作为后备 |
| 替换 Substrate 共识组件工作量超出预期 | 延期 | AC-BFT 设计为独立 crate，只依赖 `client.finalize_block`；必要时 α 版先用 PoA 单签最终性过渡 |
| STARK 电路漏洞 = 增发漏洞 | 致命 | 电路最小化（只做两种操作）；形式化规范 + 差分测试 + 外部审计；屏蔽池总余额 ≤ 透明入金总额的链上不变式检查 |
| TOPLOC 在不同 GPU / 引擎上误判 | 冤枉诚实的提供者 | 阈值按硬件和引擎组合校准；两级争议；误判时罚没审计员 |
| 冷启动需求不足 | 矿工流失 | 公共任务队列；金库保底用于采购推理；零质押入口 |
| 网关成为中心化瓶颈或审查点 | 与理念冲突 | 网关无许可；SDK 支持多网关故障切换；用户可以自建网关 |
| 一人开发，存在单点风险 | 进度 | 先写规范、再写代码；所有模块都有测试；尽早开源、吸引贡献者；赠款资助外部开发者 |
| 监管（隐私币限制） | 上架困难 | 协议中立 + 选择性披露（查看密钥）；不依赖中心化交易所 |
