> 🌐 [English](02-polkadot-consensus-privacy-tee.md) | **简体中文**

# 第二轮调研：Polkadot/JAM、共识机制、隐私边界、TEE

> 状态：讨论稿（Round 2）。日期：2026-09。

## 已确认的决策（来自第一轮讨论）

| # | 决策 |
|---|---|
| D1 | 为未来而建：去中心化预训练现在落后约 1000 倍，但协议必须从架构上支持训练，并持续演进 |
| D2 | 奖励只挂在“被网络实际使用、有人付费、并且通过验证”的工作上，不奖励“在线”或“接入” |
| D3 | 无许可接入 + 分层：消费级 GPU、TEE GPU、CPU、存储都能“挖矿”，按能力领取不同的任务等级 |
| D4 | 抗量子从第一天开始，实现可以不完美，但必须能平滑替换 |
| D5 | 路线：先服务最好的开源权重模型，再逐步自研，目标是媲美闭源 |
| D6 | 协议中立；内容策略由各服务提供者自己决定 |
| D7 | 自建 L1，倾向 Substrate（Polkadot SDK）或 JAM |
| D8 | 第一年场景：任何人用钱包匿名调用顶级开源模型 API；给 Agent 提供链上可支付的推理 |
| D9 | 无预挖；可能有融资和基金会；BTC 式固定总量上限 |
| D10 | 一人 + AI 开发，从零自研 |

---

## 1. Polkadot / JAM / Polkadot Cloud 调研

### 1.1 JAM（Join-Accumulate Machine）

- **定位**：把 Polkadot 从“中继链 + 平行链”泛化为“去中心化计算机”。平行链只是其中一种服务（CoreChains），另外还有 CoreVM（通用 VM）、CorePlay（actor 模型）。
- **计算模型**：
  - **Refine**（链下、在“核”上并行、重计算、无状态）→ 产出 work report
  - **Accumulate**（链上、轻量、有状态）→ 把结果并入全局状态
  - 执行环境是 **PVM**（基于 RISC-V），确定性、可计量 gas
- **安全模型 ELVES**：一小组验证者（backing group）先执行并担保；随后全网验证者**随机、隐蔽**地抽查（auditing）；有人报告不一致时升级为全员复核，作恶方被罚没。数据可用性靠纠删码分片（DA 层）。
- **出块**：SAFROLE（由 SASSAFRAS 简化而来），基于 Ring-VRF 和 zkSNARK 的匿名出块者抽签，几乎无分叉，**时隙 6 秒**。最终性由 GRANDPA 提供，约 3 个块，也就是 18 秒左右。
- **规模**：设计上限 1023 个验证者、341 个核；实测 1023 节点网络，PolkaJAM 压测用满 64 个核；Gray Paper 估算约 1500 亿 gas/s。
- **进度**：Gray Paper v0.8（2025 年底），2026 年初出预审计终稿；43 个实现团队、15 份提交；2026 年 7 月上了公开测试网；**平行链迁移到 JAM 的目标是 2027 年量产**，并且需要 OpenGov 公投通过。

### 1.2 Polkadot Hub / Polkadot Cloud

- **Polkadot Hub**：系统平行链，支持双 VM，即 **REVM**（Rust 实现的 EVM，Solidity 工具链原样可用）和 **PVM**（RISC-V，面向计算密集型合约），模块名是 `pallet-revive`。
- **出块时间**：2026-01 Hub 从 6 秒降到 2 秒；通过 elastic scaling（一条平行链同时占用多个核）目标 500ms（需要约 12 个核）。Yapchain 已经端到端演示过 500ms 出块。
- **Polkadot Cloud**：官方把 Polkadot 整体（coretime、Hub、JAM 服务）重新包装成“去中心化云”的品牌叙事，本质是出售 **coretime（核时间）**。需要注意，它卖的是**确定性 CPU 计算和数据可用性**，**不是 GPU 算力**。
- **2026 年 3 月**：DOT 首次“减半”，年增发削减约 54%，并设定 21 亿枚硬顶。这与你的 BTC 式硬顶思路一致。

### 1.3 需要澄清的认知：速度

| 指标 | Polkadot Hub | JAM | Monad | Solana Alpenglow |
|---|---|---|---|---|
| 出块 / 软确认 | 2s，目标 500ms | 6s 时隙 | 400ms | 约 400ms 时隙 |
| **最终性** | 取决于中继链 GRANDPA，**十几秒** | 约 18s | 约 800ms | **100–150ms**（测试网） |
| 优势 | 吞吐（多核并行） | 吞吐（341 核） | 低延迟 + EVM | 最低最终性延迟 |

**结论**：Polkadot 和 JAM 的强项是**吞吐量和并行**，不是**最终性延迟**。500ms / 600ms 指的是平行链的出块间隔，不是最终确认。JAM 的时隙是 6 秒，并不比现在的 Polkadot 更短。如果追求亚秒最终性，应参考 Monad（MonadBFT）和 Alpenglow（Votor），并用 HotStuff-2 / Jolteon 系列 BFT 替换 GRANDPA。

**对我们的意义**：推理体验并不依赖链的最终性。推理走链下的预付额度或支付通道，链只做结算。链的延迟主要影响 DeFi 和 Agent 之间的原子结算，目标定在“500ms 出块、≤1s 最终性”已经足够领先。

### 1.4 Substrate、JAM 还是 Polkadot 平行链？

| 选项 | 优点 | 缺点 | 结论 |
|---|---|---|---|
| **A. Polkadot SDK 独立链（solochain）** | 成熟的生产级框架；**无分叉 runtime 升级**（WASM runtime 链上替换，天然支持“可插拔、无痛升级”）；共识可插拔；有 `pallet-revive`（EVM）；密码学抽象（`MultiSignature` / `sp_application_crypto`）可以扩展新算法 | GRANDPA/BABE 的最终性偏慢，需要自己替换成快速 BFT；代码量大 | **推荐作为起点** |
| B. 做 Polkadot 平行链 | 共享安全、可用 elastic scaling | 依赖 DOT 和 coretime；加密原语受中继链约束，**抗量子进度由 Polkadot 决定**；与“自建 L1、主权”相悖 | 不推荐 |
| C. 分叉 JAM | 架构最先进，refine/accumulate 与 AI 工作高度契合 | 2027 年才量产；规范仍在收敛；实现复杂，一人团队无法维护一个 JAM 节点；SAFROLE 依赖 Bandersnatch Ring-VRF（椭圆曲线，**不抗量子**） | **现在不做**，但**借鉴其设计模式** |

**建议**：用 Polkadot SDK 构建独立 L1，把 AI 工作层按 JAM 的 **refine（链下重计算）/ accumulate（链上结算）+ ELVES（随机审计 + 升级复核 + 罚没）** 模式来设计。等 JAM 成熟后，可以评估迁移，或者成为 JAM 的一个服务。

另外，**Quantus Network**（基于 Substrate 的抗量子链，使用 Dilithium / ML-DSA 签名和 STARK 类证明）可以作为“在 Substrate 上做 PQ 签名”的参考实现，细节待进一步核实。

---

## 2. 共识机制深度分析

### 2.1 核心约束

1. AI 工作**不能**作为出块证明：验证成本高、浮点结果不确定、异构硬件结果不一致、存在延迟。
2. D2 要求奖励只给“付费 + 验证”的工作，这就带来**自刷（self-dealing）问题**：矿工自己付钱给自己，套取排放。Filecoin 的 Fil+ “验证交易 10 倍算力加成”就被假客户大规模套利过。
3. D9 要求 BTC 式固定上限，但 BTC 是**无论需求如何都按时间表排放**，这和 D2 冲突，需要调和。

### 2.2 候选方案

| 方案 | 描述 | 优点 | 缺点 |
|---|---|---|---|
| **A. 纯 PoS + 快速 BFT** | 质押者出块和投票；AI 工作只影响排放 | 最终性快，最成熟 | “矿工”与“出块者”分离，不符合 BTC 情怀；资本决定安全 |
| **B. 资源加权共识（Filecoin EC 式）** | 出块资格按“已验证的有用工作量”加权 | 贡献资源即参与共识 | 算力是“流量”而不是“存量”（存储可以持续证明，算力做完就没了），难以持续证明；自刷可以直接买到共识权 |
| **C. 混合：PoS-BFT 保证安全 + 有用工作决定排放** | BFT 验证者需要质押；排放大部分给 AI 工作者，小部分给验证者 | 安全和激励解耦，各自可以独立优化和升级 | 需要两套经济模型 |
| **C+. 混合 + 工作加权验证者选择** | 验证者权重 = f(质押, 近期已验证工作分) | 让“矿工”也能进入共识层 | 复杂度高，二期再做 |
| D. 纯 PoW（Quantus 式） | 哈希谜题 | 最接近 BTC | 浪费算力，违背“有用工作”初衷 |

**建议：方案 C 起步，预留 C+。** 共识层用 HotStuff-2 / Jolteon 类 BFT（参考 MonadBFT 和 Alpenglow），约 100 个验证者，目标 500ms 出块、1s 以内最终性。签名用 ML-DSA，由于 PQ 签名无法 BLS 聚合，先接受 O(n) 的签名开销，后续用 STARK 聚合（参考以太坊 leanSig / leanMultisig）。

### 2.3 用“有用工作”调和 BTC 式上限：一个草案

- **总量 21 亿枚**（数字待定），有**最大排放曲线** `E_max(t)`，按 BTC 式减半。
- 每个 epoch 实际排放 `E(t) = min(E_max(t), k × 已验证付费工作的费用)`。
- **没排出去的部分不销毁、也不给基金会，而是滚入“未来排放池”**，推迟释放。这样总量上限不变，排放严格跟随真实需求。
- **防自刷**：
  - 用户付费的一部分**销毁**（类似 EIP-1559），使“自己付钱给自己”永远亏损，要求 `k × fee < fee`，或者设计成对自刷者期望收益为负。
  - 工作奖励按 `min(贡献份额, 付费方多样性系数)` 计算，同一付费方集中度越高，奖励递减。
  - 用随机审计 + 质押罚没（ELVES 式）惩罚伪造工作。
- **验证者（安全预算）**：从排放中划出固定比例（例如 10–20%）给 BFT 验证者，外加交易费。
- **冷启动问题**：前期需求少，排放就少，矿工动力不足。可选缓解手段有两个：一是由基金会或 DAO 采购推理服务（真实需求），二是“开放模型训练任务”，由 DAO 作为付费方，用排放池出资训练社区模型。后者本身就是“为未来而建”的训练。

### 2.4 分层资源与任务矩阵

| 层级 | 硬件 | 可承担任务 | 验证方式 |
|---|---|---|---|
| T0 机密层 | H100 / H200 / B200 + CPU TEE | 隐私推理、闭源权重托管推理 | TEE 远程证明 + TOPLOC 抽查 |
| T1 数据中心层 | A100 / H100（不开 CC） | 大模型交互推理、训练节点 | TOPLOC + 冗余抽查 + 质押 |
| T2 消费级层 | RTX 3090 / 4090 / 5090、Mac | 小模型推理、**RL rollout**、嵌入、数据清洗、评测、去中心化训练的数据并行节点 | 冗余执行 + 抽样 + RepOps 式确定性 |
| T3 CPU 层 | 服务器和个人电脑 | 验证审计、ZK 证明生成、数据预处理、RPC | 确定性重执行 |
| T4 存储层 | 硬盘 | 模型权重、检查点、数据集分发 | 存储证明（PoRep / PoSt 类） |

---

## 3. 隐私边界：逐项分析

| 对象 | 默认隐私的好处 | 代价 / 风险 | 建议 |
|---|---|---|---|
| **支付关系**（谁为哪次推理付费） | 这是“匿名调用 API”的核心，无法把身份和 prompt 关联起来 | 需要屏蔽池 + ZK；客户端要生成证明 | **默认隐私**（第一年必做） |
| **转账金额与双方** | 与 Zcash 同级的金融隐私 | EVM 合约无法直接读取私有余额；交易所上币和合规压力；PQ 屏蔽池的证明更大（STARK 约 50–200KB） | **双状态**：公开账户（EVM）+ 原生屏蔽池，用户自选；支持查看密钥做选择性披露 |
| **合约状态** | 私有 DeFi、私有 Agent 策略 | 完全私有的 EVM 只能靠 TEE（Oasis 方案）或 FHE（慢几个数量级）；Aztec 需要新语言 Noir，而且 2026 年 Alpha 仍有严重漏洞 | **第一年公开**；私有合约作为二期研究（Noir 或 PVM 私有执行） |
| **Prompt / 输出** | 用户最关心的隐私 | 纯密码学方案不可用；靠 TEE 有信任假设；非 TEE 节点必然能看到明文 | **按层选择**：T0 机密（TEE）或标准（节点可见明文，但与身份无法关联）；链上**永远不存** prompt |
| **模型权重** | 允许闭源或商业权重上架，扩大供给 | 只能靠 TEE；而且权重一旦泄露不可撤回 | 开源权重公开；私有权重只能上 T0 |
| **网络元数据**（IP、时序） | 防流量关联去匿名化 | 混合网络（mixnet）会增加延迟 | 客户端可选 Nym 或 Tor 式中继；推理网关支持经由中继访问 |

**关键设计：匿名推理凭证（Anonymous Inference Credits）**

1. 用户从公开账户或屏蔽池存入代币，铸造一批**不可链接的推理额度**（基于 ZK 和 nullifier，类似 Privacy Pass / 盲签令牌）。
2. 调用推理时附上一次性凭证，提供者无法把它关联到存款人。
3. 提供者批量兑换凭证，在链上结算，nullifier 防止双花。
4. Agent 场景：Agent 持有凭证钱包，按次付费，不必每次都上链。

这一套直接服务 D8（第一年场景）。所有原语都可以用哈希（Poseidon2）+ STARK 实现，**天然抗量子**，笔记加密用 ML-KEM + X25519 混合。

---

## 4. TEE：优点与缺点

### 优点
1. **性能几乎无损**：H100 CC 开销 2–5%，Blackwell 调优后 1–3%，远程证明一次性约 1–3 秒。
2. **现有推理栈可以直接用**：vLLM / SGLang 放进机密 VM 即可运行，工程量最小。
3. **一石二鸟**：远程证明能证明“跑的是哪个模型、哪份代码”，同时兼顾**隐私和可验证性**。
4. **能保护模型权重**，让闭源或商业模型也能上网络，扩大前沿供给。
5. 已有先例：Phala 在 OpenRouter 上提供 GPU TEE 推理，Meta、微软、谷歌都在建设机密推理管线。

### 缺点
1. **信任根是厂商**：NVIDIA、Intel、AMD 掌握证明密钥和吊销权，而它们都是受美国出口管制的公司。**这正是你反对的中心化权力**：厂商可以吊销某地区设备的证明，而且 H100 / B200 本身就受出口管制，**T0 层在地理上天然集中**。
2. **威胁模型错位（最关键的一点）**：TEE 设计防的是“云上的其他租户和被攻破的系统软件”，**防不住有物理访问权的机器所有者**。在无许可网络里，**节点运营者恰恰就是有物理访问权的潜在攻击者**。
   - WireTap / Battering RAM（2025）：成本 50–1000 美元的 DDR4 内存总线插入器可以攻破 SGX。
   - **TEE.fail（2025-10）**：在 DDR5 上攻破 Intel TDX 和 AMD SEV-SNP，**提取证明密钥后可以伪造 TEE 报价，进而骗过 NVIDIA GPU 机密计算**，让攻击者在完全不受保护的环境里冒充 TEE。
   - DDRop（2026-09）：又一个攻破 TDX 和 SEV-SNP 的攻击。
   - 根源：服务器级 TEE 为了性能，采用确定性 AES-XTS 加密，并去掉了完整性和重放保护。
3. 侧信道和固件漏洞会持续出现，每次都需要吊销和补丁。
4. 消费级 GPU 不支持，T0 层只能是高端数据中心卡。

### 结论
TEE 在我们的网络里只能定位为 **“提高攻击成本的一层”，而不是“隐私保证”**。缓解手段（纵深防御）：
- T0 节点需要**高额质押**，攻破行为可证明时罚没（例如同一证明密钥在多处出现）；
- 维护证明密钥和固件版本的**链上吊销列表**，由 DAO 快速响应；
- T0 节点可以选择公开数据中心的运营信息（信誉层，非强制）；
- 用户端**把身份隐私和内容隐私分开**：即使 prompt 泄露，也无法关联到钱包或身份（依靠匿名凭证和中继）；
- 长期追踪 GPU 端完整性保护、开源 TEE（Keystone、OpenTitan 系）和 FHE 加速进展，作为可插拔的隐私后端。

---

## 5. 一人 + AI 的第一年 MVP（草案）

1. **链**：Polkadot SDK 独立链；先用 Aura + GRANDPA 跑通，再替换为快速 BFT；账户签名从一开始就支持 `ML-DSA` + `Ed25519` 混合（带算法 ID）；接入 `pallet-revive` 提供 EVM。
2. **原生屏蔽池 + 匿名推理凭证**：STARK 证明 + Poseidon2 + ML-KEM 混合加密。
3. **推理市场 pallet**：提供者注册（层级、模型、价格、质押）→ 凭证兑换结算 → 随机审计（TOPLOC）→ 罚没。
4. **推理网关**：兼容 OpenAI API，支持流式输出，按延迟和价格路由，客户端 SDK 自动管理凭证。
5. **排放 pallet**：硬顶 + 按需排放 + 销毁 + 集中度衰减。

## 来源

- JAM：https://wiki.polkadot.com/learn/learn-jam-chain/ · https://blockeden.xyz/blog/2026/01/16/polkadot-jam-architecture-blockchain-virtual-machine-paradigm-shift/ · https://blockeden.xyz/blog/2025/10/28/jam-chain-polkadot-s-paradigm-shift-toward-the-decentralized-global-computer/ · https://www.hokanews.com/2026/09/polkadot-prepares-major-jam-transition.html · https://coinbureau.com/review/polkadot-dot · https://github.com/openguild-labs/learn-jam
- Polkadot Hub：https://docs.polkadot.com/reference/polkadot-hub/smart-contracts/ · https://openguild.wtf/blog/polkadot/polkadot-introducing-about-dual-vm-architecture-polkadot-hub · https://blockchain.news/flashnews/polkadot-hub-to-add-evm-pvm-smart-contracts-and-2-second-blocks-on-jan-20-2026
- Elastic scaling / 500ms：https://forum.polkadot.network/t/elastic-scaling-wen-500ms-blocks/11971 · https://wiki.polkadot.com/learn/learn-elastic-scaling/ · https://medium.com/polkadot-network/polkadot-roundup-2025-3c3c71c7e9c4
- 共识：https://wiki.polkadot.com/learn/learn-consensus/ · https://spec.filecoin.io/algorithms/expected_consensus/ · https://www.helius.dev/blog/alpenglow · https://solana.com/alpenglow
- Polkadot PQ 路线：https://medium.com/@gwrx2005/post-quantum-roadmaps-for-blockchain-ecosystems-af9e77a6fe8b
- TEE 攻击：https://www.bleepingcomputer.com/news/security/teefail-attack-breaks-confidential-computing-on-intel-amd-nvidia-cpus/ · https://thehackernews.com/2026/09/new-ddrop-attack-breaks-intel-tdx-and.html · https://hacken.io/insights/wiretap-and-battering-ram-risks/ · https://arxiv.org/pdf/2507.02770
