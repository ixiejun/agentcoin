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
| **TOPLOC 激活值提取** | **薄 Python 插件**，挂在推理引擎内部，只负责把隐藏层激活值交给 Rust 代理（通过本地 socket） | TOPLOC 需要读取模型内部的激活值，只能在引擎进程里完成；插件保持极小、无业务逻辑。实际实现（m5-engine-toploc）：`plugins/vllm` 发送每步的 top-k 候选，由 `ac-toploc` 构造证明 |
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

- **禁用**以太坊原生交易格式（RLP + secp256k1）：runtime 的调用过滤器拒绝 `eth_transact` 及所有以太坊签名路径，RLP 字节也永远无法解码为 AgentCoin 交易。
- **交易格式**：合约交易就是由 `PqAuthorize`（ML-DSA）授权的普通 AgentCoin v5 交易，其调用为 `Revive::call`，或携带 EVM 字节码的 `Revive::instantiate_with_code`（构造参数附加在初始化代码之后）；`SetEvmPayer` 扩展让签名者支付合约账户的费用，因此部署永不增发。EVM 链 ID 为 4403。
- EVM 地址 = `keccak256(AccountId)[12..]`（`pallet-revive` 的账户映射，自动建立）；合约中需要唯一性的场景（例如 CREATE2 冲突检测）以 32 字节为准。
- **预编译**（地址一经发布永不更改）：
  - `0x000000000000000000000000000000000A010000` `pq_verify(uint8 alg, bytes publicKey, bytes message, bytes signature) -> bool`：在固定上下文 `agentcoin/evm-verify/v1` 下验证 ML-DSA-44/65/87 签名，任何未通过验证的输入都返回 false；
  - `0x000000000000000000000000000000000A020000` `blake3(bytes) -> bytes32`；
  - `0x000000000000000000000000000000000A030000` `poseidon2(bytes) -> bytes32`：Goldilocks 域上的 Poseidon2，Plonky3 的宽 12 实例（m4-evm 中的用户决定：没有经过审查、支持 `no_std` 的库实现作者参考实例，因此使用 Plonky3 自己的向量）；编码见 [ac-crypto README](../../crates/ac-crypto/README.zh-CN.md)；
  - **[预留]** `0x000000000000000000000000000000000A100000` `stark_verify(vk_id, proof, public_inputs)`：为 L2 通用私有合约预留（D27），目前任何调用都回滚。
- `pallet-revive` 内置的以太坊预编译仍对应用开放，但**不是后量子的**：`ecrecover`（0x01）、`bn128` 加法/乘法/配对（0x06–0x08）、`point_eval`（0x0a，KZG）和 `p256_verify`（0x100）。它们仅为应用兼容而保留，永远不能用于账户授权，不建议使用。
- **eth-RPC 适配器**（`ac-eth-rpc`，[README](../../services/eth-rpc/README.zh-CN.md)）：为 Foundry 等工具提供以太坊 JSON-RPC：`web3_clientVersion`、`net_version`、`eth_chainId`、`eth_syncing`、`eth_blockNumber`、`eth_accounts`、`eth_gasPrice`、`eth_maxPriorityFeePerGas`、`eth_feeHistory`、`eth_getBalance`、`eth_getTransactionCount`、`eth_getCode`、`eth_getStorageAt`、`eth_call`、`eth_estimateGas`、`eth_getBlockByNumber`、`eth_getBlockByHash`、`eth_getTransactionByHash`、`eth_getTransactionReceipt`、`eth_getLogs`、`eth_sendRawTransaction`。`safe`/`finalized` 即 AC-BFT 终局性。`eth_sendRawTransaction` 只转发 ML-DSA 签名的 AgentCoin 合约交易；`ac-wallet evm` 是外部签名器（部署、调用、raw、广播 `forge script` 模拟结果、为 `pq_verify` 签名消息）。

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
- **M2 的实现**（协议形式 D38，违规处置 D39）：
  - 按轮次而非高度投票：第 `r` 轮的领导者（第 `r mod n` 个成员）提议自己的最佳区块；成员按锁定规则投 prepare 票，看到 prepare 证书（`q = ⌊2W/3⌋ + 1`）后投 commit 票并立即进入下一轮；commit 证书最终确定目标区块及其全部祖先。超时从两个时隙开始，按 ×1.5 退避，上限 30 秒；落后的节点根据权重超过 `W − q` 的成员已到达的轮次追赶。
  - 消息带版本号，用 ML-DSA-65 在上下文 `agentcoin/bft-vote/v1` 下签名，绑定创世哈希与集合编号，通过 `/<创世哈希>/acbft/1` 泛洪。投票先持久化再发送。
  - 授权集合只在纪元边界变更（`ScheduledChange` 摘要，引擎 `acbf`），变更区块由旧集合最终确定。每个变更区块以及至少每 64 个区块保存一份终局性证明（带版本号的 commit 票集合），导入和同步时都会验证。
  - 违规：出块双签（同一时隙两个区块头）和投票双签（同一集合同一轮同一类型的两条消息）通过无签名的授权交易自动举报。M2 对同一集合内的每个违规者每类违规最多记录一条，在第一条记录时把违规者移出下一纪元的集合（不会移空），处置只执行一次——罚没将按记录中最重的一类计算，不累加；暂不罚没质押，罚没在引入 PoS 时接入预留的 `SlashHandler`。M3 PoS 起由 `pallet-staking-pos` 罚没违规者自质押（投票 100%、出块 10%）并销毁。
  - 在一台 4 核机器上测量（release 构建，本机网络）：见 `m2-finality` 设计文档的“测量结果”；4、7、10 个验证人时终局性延迟的 P95 都远低于 3 秒。

### 4.2 链上随机数（审计抽样用）

- `pallet-randomness-cr`：验证者每个 epoch 先提交 `H(secret)`，下个 epoch 再揭示 `secret`。随机数 = `BLAKE3(所有揭示值)`。不揭示的验证者会被扣除排放。
- 已知的弱点：最后一个揭示者可以选择不揭示，从而对结果产生 1 比特的影响。对审计抽样来说可以接受。全量版加入基于哈希的 VDF。
- **M2 的实现**：出块者在区块中放入强制 inherent `note_randomness`（纪元 `e` 的承诺、纪元 `e − 1` 的揭示）；秘密值由验证人种子、创世哈希和纪元派生，因此重启后仍能揭示。`R(e) = derive_key("agentcoin 2026-09 randomness v1", u64_le(e) ‖ 按账户 ID 排序的揭示值)` 在纪元 `e + 2` 的第一个区块公布，可以由公开的揭示值复算。未揭示次数按验证人计数。runtime 提供 `RandomnessApi` 和 FRAME 的 `Randomness`。M3 PoS 起，未揭示的纪元该验证人不计奖励工作分，连续 3 次则暂停参选。

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
- **M3 的实现（`m3-pos`）**：
  - 条件（修订 D19）：全部有效质押（自质押加提名，不含解绑中）≥ **总发行量**的 10%；自质押 ≥ 发行量 0.1% 且未暂停参选的候选人至少 21 个；区块高度 ≥ 63,115,200（上线满 2 年）。每个验证人纪元边界都是检查点；`ValidatorSet::QualifiedSince` 记录一段连续达标的首个检查点，连续达标满 604,800 个区块（7 天）的纪元边界永久切换到 PoS。在此之前多签可以增删 PoA 授权节点，切换时失去该权力，PoA 授权节点全部退出。7 天缓冲期内每个纪元公布一次预演选举。
  - 质押（`pallet-staking-pos`）：候选人注册 ML-DSA-65 验证人公钥并附持有证明（`agentcoin/validator-pop/v1`），佣金 5–100%（调高 7 天后生效，调低下一纪元生效）；提名人以不低于发行量 0.001% 的金额提名 1–16 个候选人；候选人最多 500 个、提名人最多 750 个，名单满时挤出最小的一笔。自质押解绑 28 天；提名通过全网队列解绑，2–28 天（参照 Polkadot RFC-0097）。
  - 选举：每个纪元最后一个区块运行带平衡的顺序 Phragmén（`sp-npos-elections`），正式链 `K` = 100（参数；存储上限 1,000，问题记录 I-004）；AC-BFT 权重为 `max(1, 支撑额 / 10^12)`。最坏情况能在一个区块内算完（问题记录 I-005）。
  - 奖励（R1）：安全预算按出块数分配，与质押多少无关；每个验证人的份额先付佣金，其余按支撑额比例分配，从奖励池自动发放。未揭示随机数的纪元不计工作分，连续 3 次则暂停参选。
  - 罚没（U3）：只罚验证人自质押（锁定中和解绑中）——投票双签 100%，出块双签 10%，同一集合按最重一类，经 `Emission` 销毁。提名人永不被罚，掉线不罚本金。
  - 节点从固定存储键独立复算每个检查点（§6），任何 runtime 升级都无法提前切换、拖延切换或退回 PoA。

---

## 5. Runtime 模块（pallets）

| Pallet | 版本 | 职责 |
|---|---|---|
| `pallet-pq-accounts` | α | 公钥注册、换钥、`PqAuthorize` 交易授权 |
| `pallet-emission` | α | 计划排放、滚存储备、四份分配、按 epoch 结算 |
| `pallet-treasury-dual` | α | 社区赠款 + 持币人金库；5% 保底线性归属 |
| ~~`pallet-fee-burn`~~ | α | 已合并到 `pallet-emission`，由其记录所有销毁（m3-economics） |
| `pallet-poa-admin` + `pallet-collective` | α | PoA 多签：ML-DSA 成员按门限以 Root 身份执行（D41） |
| `pallet-validator-set` | α | 纪元、PoA 名单、单向的 PoA → PoS 状态机、安装选举产生的集合 |
| `pallet-staking-pos` | α | 候选人、提名、解绑队列、佣金、Phragmén 选举、按工作量发放奖励、罚没（m3-pos） |
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
// 常量（已发布；同时也是节点不变式的输入）
const UNITS: u128             = 10u128.pow(18);
const CAP: u128               = 21_000_000 * UNITS;
const FIRST_PERIOD_TOTAL: u128 = 10_500_000 * UNITS;
const BLOCKS_PER_PERIOD: u64  = 126_230_400;        // 4 × 365.25 天，1 秒出块
// 排放纪元长度 L：创世参数，必须整除 BLOCKS_PER_PERIOD
EpochLength: u64                                    // 正式链 3,600；开发链不超过 20

fn scheduled(e: u64) -> u128 {                      // S(e)
    let n = e * L / BLOCKS_PER_PERIOD;              // 已减半次数；减半只发生在纪元边界
    (FIRST_PERIOD_TOTAL >> n) * L as u128 / BLOCKS_PER_PERIOD as u128
}

// 存储（TotalBurned 是已发布的固定存储键）
Reserve, TotalMinted, TotalBurned, LastSettled

// 纪元 e = 第 e·L+1 ..= (e+1)·L 块，在第 (e+1)·L+1 块的 on_initialize 中结算：
S        = scheduled(e)
avail    = S + min(Reserve, S)
security = S * 10%                                  // PoA：不发放，留在储备中（D19）
market   = min(avail * 50%, verified_market_work)
public   = min(avail * 20%, verified_public_work)
prop     = (market + public) * 20 / 70
treasury = max(prop, S * 5%)                        // 取大，不相加（红线 5）
total    = security + market + public + treasury   // 超过 avail 时按比例缩减（向下取整）
drawn    = max(0, total - S)
Reserve  = Reserve - drawn + max(0, S - total)
```

- 全部运算为 `u128` 向下取整；余数留在储备，因此 `Reserve' + total = Reserve + S`，不会凭空创造。同一个函数（`ac_primitives::emission::settle`）在 runtime、节点不变式和经济模拟（`tests/sim`，两个四年期）中运行。
- **国库**：`treasury` 与实际的工作排放成比例，需求满载时恰好是 10/50/20/20 中的 20%，需求不足时按比例缩小（研究 04 §2），但不低于保底 `5% × S`。比例份额 40% 进入社区资助、60% 进入持币人国库（创世参数 `community_share`，取整余数归持币人国库）；持币人国库在链上持币人投票（M8，D42）之前没有任何出口。保底差额进入保底账户，按 30 天分批，每批从批次结束起 2 年线性解锁，只能用于审计和冷启动。
- M5 之前没有已核验工作量，PoA 阶段不发放安全预算，所以每个纪元只铸造 5% 的保底。
- **市场排放**（m5-work-settlement，D58）：纪元 `e` 的 `verified_market_work` 是在 `e` 到期的报告的工作量（`WorkSource` = `pallet-work`）。`market` 铸入 `Work` 的领取账户（`MarketPayout`），提供者按工作量比例与份额一起领取。领取账户永久留存一个存在性押金（取自它收到的第一笔排放），节点不变式看到的仍恰好是铸币减销毁。
- **销毁**（原独立的 `pallet-fee-burn`，现已合并到本模块）：所有销毁——交易手续费和小费的 80%、被回收账户的尘埃，以及之后的罚没和推理费——都经 `Emission` 丢弃 `Credit` 并累加到 `TotalBurned`。
- `k`（排放倍数）和销毁比例 `b` 受护栏约束：`0 ≤ k − b ≤ 0.5`（研究 03 §1.5）。

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
- **实际实现**（m5-market-registry，D54）：透明额度采用**按通道累计的凭证**，而不是每个请求一张、带 nonce 的凭证。用户把 ATC 托管给网关，记入（用户, 网关）通道，并以 `agentcoin/voucher/v1` 签署 `{创世哈希, 用户, 网关, 通道号, 累计美元额}`；链上每个通道只保存一个已兑付额计数，因此重放不付款，也没有不断增长的 nonce 集合。兑付时按参考汇率把增量向下取整换算；取回托管和更换凭证密钥都要等待一天，期间此前的凭证仍可兑付。`Credit::redeem` 只由结算（M5 的下一个变更）调用。
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
- **实际实现**（m5-market-registry，D55）：质押门槛按参考汇率向上取整换算（T1 1,000 美元、T2 100 美元、网关 1,000 美元，网关费率上限 5%）；提供者处于活跃状态、质押不低于门槛且保持心跳（两次心跳间隔不超过两个 600 块的周期；按时心跳免手续费）时为*可服务*；提供者和网关的质押解绑期为 7 天。罚没与禁闭接口留给 M6，任何交易都调用不到，PoA 管理多签也不能（D6）。参考汇率由 PoA 管理多签设置，每天最多调整 ±20%（D56）。提供者的加密公钥只接受 X-Wing。

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

- **实际实现**（m5-work-settlement，D57–D59）：
  - **收据**不带 `voucher_ref`。收据为 `{genesis, gateway, provider, kind, model, request_id, in_tokens, out_tokens, fee, toploc_commit, ttft_ms, total_ms}`，由提供者和网关分别以 `agentcoin/receipt/v1` 做 ML-DSA 签名，并携带双方公钥。费用按提供者登记价格计算，向上取整到微美元。报告通过 BLAKE3 Merkle 根承诺收据。
  - **报告**按（提供者, 模型）给出合计，至多 128 项，附至多 16 张凭证（基准测得满额报告的权重低于普通交易上限的一半）。只接受推理。合计必须**恰好等于**凭证兑付的美元增量，不再有单独的额度批次。
  - **款项分配**：兑付得到的 ATC `g` 中，`b` = 20% 经 `Emission` 销毁，其次是网关费（按登记费率），其余按美元比例分给提供者，向下取整，余数销毁。网关费与份额**冻结在网关账户上**，直到报告到期。每个提供者的工作量为 `G_i × k`，`k` = 50%。
  - **挑战期**为两个排放纪元：在纪元 `s` 提交的报告计为纪元 `s + 2` 的已核验工作量。M5 中挑战期只是延迟，没有任何调用能挑战报告。失去待领取款项的唯一途径是被禁闭（M6）：被禁闭提供者的份额销毁，排放不发放。
  - **领取**：纪元结算后，任何人都可以调用 `claim(who, [(纪元, 网关), …])`：份额从网关转给 `who`，`who` 在该纪元的市场排放份额从 `Work` 领取账户转出。
  - **记录**：报告与各纪元记录保留 720 个纪元，由 `WorkApi` 提供查询。
  - **TOPLOC**（`ac-toploc`）是参考实现（提交 `7ab7bcd`）的逐字节兼容移植，向量由 `scripts/gen-toploc-vectors.sh` 重新生成。承诺的复核属于 M6。

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

- **实际实现**（m6-toploc-verify，即第 3 步的复核；链上部分即第 1、4、5 步，随 `m6-audit-chain` 与 `m6-auditor-agent` 实现）：
  - **复核**：`ac-auditor recheck` 读入一个案例（请求的 messages、回答、用量、签名收据与证明），检查收据承诺了这些证明与 token 数量，用引擎自己的分词器与对话模板重现 prompt 与回答的 token，并在 TOPLOC 插件处于复核模式（`AGENTCOIN_TOPLOC_MODE=verify`，本机协议 v2）的 vLLM 中作为**一次预填充**运行。插件发出每个 token 行的 top-k 候选；prompt 之后的各行代表提供者的各个解码步骤。`ac-toploc::compare_from_candidates` 把它们按证明的分块重组并比对，得到与用全量激活值比对相同的指标。
  - **判定**：每块按 `ac-market-proto` 中的 `AUDIT_THRESHOLDS` 判定（指数不一致次数、尾数误差均值与中位数的整数上限；预填充块由审计员以与提供者相同的方式计算，用严格阈值，解码块用较宽的阈值）；每一块都通过，推理才通过。没有证明（承诺全零）或证明与承诺不符判为**不通过**（I-008）。注册为 bfloat16 以外精度的模型、无法重现 token 的回答以及引擎错误判为**无法判定**，绝不判为不通过。
  - **校准**：阈值来自在 CPU 上用 vLLM 生成并复核的诚实样本与四类作弊（换模型、int8 与 int4 权重、隐藏的 system prompt）（`scripts/calibrate-toploc.py`，工作流 `toploc-calibration`；[报告](../guides/toploc-calibration.zh-CN.md)）；CI 每次推送都运行一个小规模版本。α 测试网之前必须在 GPU 上校准（I-012）。
  - **隐私**：审计员的日志只含案例名、结果与指标，绝不含 prompt 或回答。
- **实际实现**（m6-audit-chain，链上的第 1、4、5 步；`pallet-audit`）：
  - **审计员**质押 1,000 美元（活链草案；提供者与网关不能登记），解绑期 7 天。每 30 分钟一轮（1,800 个区块），链记录名单与由提交-揭示随机数派生的种子，为每个提供者抽 2 名审计员；任何人都能复算抽样。
  - **裁决**（通过；不通过，附原因与证据承诺；无法判定，附原因）带阈值版本与提供者和网关双签的收据，收据在链上校验；每个请求 ID 只能用一次。证据本身（prompt、回答、证明）留在链下；不通过的裁决以 `agentcoin 2026-10 audit-evidence v1` 承诺证据。
  - **争议**：本轮与上一轮内 2 名不同审计员判为不通过即开启争议；抽出 5 名复核人复核证据，3 票决定。确认：罚没提供者 10% 质押（销毁）并禁闭，其未结算的工作量作废。驳回：每名提出者罚没 10% 并被迫退出。600 个区块后仍未决的争议关闭，不处罚任何一方。
  - **支付**：每条被接受的裁决、每张获胜一方的票，从审计资金池支付 0.05 美元；资金池由国库保底池以“审计”用途注资。管理权限只能在护栏内调整质押门槛、支付金额与接受的阈值版本。
  - “无法判定”按提供者计数（I-013）；自动发起请求、复核并提交的神秘顾客代理在 `m6-auditor-agent` 中实现。

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

写在节点客户端（native 代码，crate `ac-invariants`）里，**不在 runtime 里**，因此任何 runtime 升级都无法修改它。

```rust
// ac-invariants（纯函数），由 ac-node 的 InvariantBlockImport 对每个区块调用
fn check_block(params: &GenesisParams, n: u64, pre: Ledger, post: Ledger) -> Result<(), Violation> {
    ensure!(post.issuance <= CAP);                              // 宪法 1：2100 万
    let minted = (post.issuance + post.burned) - (pre.issuance + pre.burned);
    match settled_epoch(n) {
        None => ensure!(minted == 0),                           // 只有结算区块可以铸币
        Some(e) => ensure!(minted <= 2 * scheduled(e)),
    }
    let since_genesis = post.issuance + post.burned - params.genesis_issuance;
    ensure!(since_genesis <= cumulative_scheduled(settled_epochs(n)));
    Ok(())
}
```

- `Ledger` 从已发布的固定存储键读取：`Balances::TotalIssuance` 与 `Emission::TotalBurned`（SCALE `u128`）。铸币量按“发行量 + 已销毁量”的变化计算，因此销毁无法掩盖铸币，runtime 虚增销毁计数只会让检查更严。键缺失或无法解码时拒绝区块（fail-closed）。
- `InvariantBlockImport` 位于 AC-BFT 区块导入与客户端之间：本节点产出、从网络接收或同步而来的区块都经过它。对其他节点的区块，它在父状态上执行区块并把存储变化交给客户端，因此每个区块只执行一次。不经执行就导入的区块会被拒绝，所以不支持 warp 同步和快速同步。
- 节点启动时检查链规格的创世：合法的 `Emission::EpochLength`、发行量不超过上限；正式链还要求零发行、无余额分配，且至少一名 PoA 管理成员（`PoaCouncil::Members`）。
- PoA → PoS 切换（宪法 4，`m3-pos`）：节点对每个区块读取前后两份状态中的 `ValidatorSet::{Phase, QualifiedSince, PoaAuthorities}`。在每个 PoA 纪元边界，用 runtime 同一个 `transition_step` 从父状态（`StakingPos::Ledger` 中有效质押之和、`StakingPos::Candidates` 中的合格候选人数、发行量、高度）复算检查点，阶段或 `QualifiedSince` 不一致即拒绝区块：不得提前切换、不得拖延、不得退回、PoS 阶段不得保留 PoA 名单、边界之外不得改动切换状态。启动时正式链的 `ValidatorSet::TransitionParams` 必须等于宪法值（10%、21、63,115,200、604,800）。
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

- **实际实现**（m5-gateway-provider，D60–D62）：
  - `ac-gateway` 与 `ac-provider` 是 Rust 服务；`ac-wallet market serve` 为未修改的 OpenAI SDK 提供本地接口。市场承载 `/v1/models` 与 `/v1/chat/completions`，流式与非流式均可。
  - **传输**：每一跳（代理 → 网关 → 提供者）都是密封通道。发送方对接收方的 X-Wing 公钥封装：提供者的公钥登记在链上，网关的公钥在其服务地址公布并由网关账户密钥签名。发送方以账户的 ML-DSA 密钥签署握手。每个方向用 ChaCha20-Poly1305 分块加密，握手必须新鲜（±120 秒）且不可重放。
  - **付款**：基于透明额度的后付费。请求附带的累计凭证恰好等于已计费总额，托管余额必须覆盖未兑付的账单加上所有在途请求的最大费用。代理按链上信息校验每条双签收据后，才付清新的总额。
  - **路由**：优先选择价格最低的可服务提供者，同价时选平滑首 token 延迟最低者；网关在首块之前自动切换提供者。
  - **报告**：每隔若干区块自动提交，间隔不超过一个排放纪元。
  - **收据**：网关与提供者都保存到挑战期结束后删除。提供者自动心跳与领取。
  - 请求内容不被记录或保存。
- **实际实现**（m5-engine-toploc，D63–D64）：
  - **插件**：`plugins/vllm`（Python，宽松许可区）包装 vLLM 0.30 的 V1 模型运行器。每个前向步骤之后，它取每个请求的最终归一化层激活值（TOPLOC 参考实现读取的就是这组激活值），在设备上按“幅值降序、相等时下标升序”取前 128 个，经本机 Unix 套接字发给提供者，从不阻塞推理。只有模型以 bfloat16 运行、且前缀缓存、投机解码与流水线并行都关闭时，插件才允许启动。
  - **证明**：由提供者用 `ac-toploc` 构造（top-k 128、解码每 32 步一块、预填充单独一块）。它们与由全量激活值构造的证明逐字节相同（并列时也一样），并由收据承诺。证明随收据交给网关与用户，二者都在计费与付款前校验。网关与提供者保存证明到挑战期结束。
  - **没有证明时**：在 M6 审计另行决定之前，承诺为全零的收据仍然有效（问题 I-007 至 I-011）。这包括没有插件、`n > 1` 与被抢占的请求。

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
| 排放纪元 | 正式链 3,600 块（创世参数，须整除 126,230,400 块） | 宪法第 1 层 |
| 储备动用上限 | 每 epoch 1 倍计划额 | 护栏 [0.5, 2] |
| 分配 | 10 / 50 / 20 / 20（安全 / 市场 / 公共 / 金库） | 护栏；金库 ≤ 20% |
| 金库保底 | 5% 计划额；按 30 天分批，每批从批次结束起 2 年线性解锁；仅用于审计和冷启动 | 护栏 |
| 国库比例份额 | 社区资助 40% / 持币人国库 60%（M8 前锁定） | 护栏（M8） |
| 推理费销毁比例 `b` | 20% | 护栏 [10%, 90%] |
| 排放倍数 `k` | 0.5 | 护栏：`k − b ≤ 0.5` |
| 网关费率上限 | 5% | 护栏 |
| 训练者分润 | 5%（全量版启用） | 护栏 [0, 10%] |
| 交易费 | 按权重 + 长度计费；每笔手续费和小费的 20%（向下取整）给出块者，其余销毁；份额不足新出块者账户存在性押金时一并销毁 | 护栏 |
| 存在性押金 | 0.001 ATC | runtime 常量 |
| PoA 管理权限 | ML-DSA 账户按门限多签；决议期限 7 天 | 创世设定，之后由多签自身变更 |
| PoA → PoS 切换 | 质押达发行量 10%、21 个合格候选人、高度 ≥ 63,115,200，连续保持 604,800 个区块 | 宪法第 1 层 |
| 活跃集合规模 `K` | 100（存储上限 1,000） | PoA 管理权限；护栏（M8） |
| 最低自质押 / 提名 | 总发行量的 0.1% / 0.001% | runtime |
| 候选人 / 提名人 | 最多 500 / 750 | 创世 |
| 解绑 | 自质押 28 天；提名 2–28 天（队列） | 创世 |
| 佣金 | 5%–100%；调高 7 天后生效 | 创世 |
| 罚没 | 投票双签 100%、出块双签 10% 的自质押；销毁 | runtime |
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
| **M6 审计** | 8–10 | 审计员代理、神秘顾客、TOPLOC 复核及其阈值（问题 I-007）、缺少证明的代价（I-008）、哪些精度可以出证明（I-011）、争议与罚没、公共任务（精简） | 故意换模型 / 降精度的提供者在 1 小时内被检出并罚没；诚实提供者误判率 <0.1% |
| **🚩 α 测试网** | 10 | 公开测试网 + 文档 + 水龙头 | 外部提供者能在 30 分钟内接入 |
| **M7 屏蔽池** | 9–14 | STARK 选型基准 → 电路 → `pallet-shielded` → 凭证铸造和兑换 → 钱包 / SDK 集成 | 凭证全流程跑通；客户端证明 ≤10s；链上验证 ≤50ms；**电路内部审查 + 模糊测试** |
| **M8 治理与宪法** | 12–15 | 代币投票（referenda + conviction）、护栏、宪法 pallet、移除 sudo 的流程 | 越界参数修改被拒绝；升级强制延迟生效 |
| **🚩 β 测试网** | 15 | 完整 MVP | 30 天稳定运行；激励测试（邀请外部矿工） |
| **M9 审计** | 15–18 | 外部安全审计（电路、共识、经济），修复问题 | 无未修复的严重 / 高危问题 |
| **🚀 主网 Beta** | ≈18 | 创世（PoA，无预挖），网关只接受匿名凭证 | §0.4 的全部指标达标 |

**状态**（2026-10）：M0–M5 已完成；M6 进行中。M5：市场登记（参考汇率、模型、提供者、网关、透明额度、钱包 `market` 命令）、工作结算（签名收据、工作报告、销毁与款项分配、挑战期延迟、市场排放与领取、TOPLOC 移植、钱包 `receipt` / `report` / `claim` 命令）以及网关与提供者服务（密封通道、面向 OpenAI SDK 的后付费本地代理、路由与故障切换、自动报告与领取，已用 OpenAI Python SDK 与模拟引擎做端到端测试）以及引擎内的 TOPLOC 证明（vLLM 插件，证明由网关与代理校验，已在 CI 中用真实的 CPU 版 vLLM 测试）均已完成：M5 完成。M6：TOPLOC 复核（`ac-auditor recheck`、插件的复核模式、`AUDIT_THRESHOLDS`；阈值的最终校准进行中）与链上审计（`pallet-audit`：审计员、随机分配、附双签收据的裁决、争议、罚没与禁闭、审计资金池支付）已完成；审计员代理的神秘顾客审计和公共任务随后进行。M4：带 PQ 预编译的 `pallet-revive`、eth-RPC 适配器和 Foundry 外部签名器，已用 ERC-20 与官方 Uniswap V2 合约（CREATE2 创建配对、添加流动性、兑换）做过端到端测试。合约权重：PQ 预编译（`pallet-evm-support`）使用在本 runtime 上实测的权重；`pallet-revive` 保留上游的 `SubstrateWeight`，因为它的基准要铸币来准备账户，而本链防增发的货币封装会拒绝铸币。runtime 测试检查了在这组权重下，最重的合约调用和部署仍放得进一笔普通交易。

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
