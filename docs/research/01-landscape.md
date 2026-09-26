# 第一轮调研：去中心化 AI 算力 × 高性能 EVM × 隐私 × 抗量子

> 状态：讨论稿（Round 1），尚未形成设计决策。日期：2026-09。

## 0. 愿景复述

构建一个无许可的公链 / DAO：

1. **资源层**：数据中心 GPU、消费级显卡、CPU、存储无许可接入，像 BTC 矿工一样贡献资源、按工作量获得原生代币激励。
2. **AI 全链路**：预训练 → 后训练（SFT / RL）→ 推理服务，对标中心化云，体验不打折。
3. **应用层**：兼容 EVM，支持 DeFi、AI、Agent 等 DApp，引入需求方。
4. **性能**：达到商业高可用级别。
5. **抗量子**：密码学模块可插拔，初期用成熟方案，后期无缝切换 PQC。
6. **隐私**：天生隐私的公链。

## 1. 去中心化训练：现状与差距

| 项目 | 代表成果 | 关键技术 | 去中心化程度 |
|---|---|---|---|
| Templar (Bittensor SN3) | Covenant-72B，约 1.1T tokens，70+ 节点 | 低通信梯度压缩 | 无许可 |
| Nous Psyche | Consilience 40B | DisTrO，Solana 协调 | 许可制测试网 |
| Prime Intellect | INTELLECT-1 (10B)、INTELLECT-2 (32B 异步 RL)、INTELLECT-3 (106B MoE) | OpenDiLoCo、PRIME-RL、TOPLOC | 白名单；INTELLECT-3 实际在集中式 512 GPU 集群上训练 |
| Pluralis | Protocol Model 8B | 模型并行 + 协议学习 | 策展制 |
| Gensyn | 2026-04 主网（OP Stack L2） | Verde / RepOps 位级可复现验证 | 无许可 |

**结论**
- Epoch AI 估计：最大的去中心化预训练（6e22–6e23 FLOP）比前沿模型**少约 1000 倍**算力。瓶颈是互联网带宽、慢节点（straggler）和验证成本。
- **RL 后训练天然适合去中心化**：rollout（生成）本质是推理，可大规模分散；只有策略更新需要集中同步（INTELLECT-2 已经验证）。
- 消费级显卡显存（24–32GB）放不下前沿 MoE（600B+），只能做小模型、rollout、嵌入、数据处理，或者参与流水线 / 专家并行（延迟很高）。

## 2. 去中心化 GPU 市场（DePIN）：需求才是瓶颈

- 2026 年初整个赛道年化协议收入约 2 亿美元；Aethir 约 1.5 亿 ARR（主要来自企业），io.net 约 2000 万，Akash 约 430 万。
- **申报供给远大于付费利用**：io.net 清理后，32.7 万台注册设备中只有 5,350 台通过验证；Akash 2026 年 Q1 为 334 块 GPU 里 84 块在用。
- 启示：光靠发币补贴供给，只会招来刷量 / 女巫矿工。**激励必须锚定“被验证的、有付费需求的有用工作”**。

## 3. Bittensor 的启示

- dTAO（2025-02）：每个子网发行 Alpha 代币，由市场决定排放分配。子网覆盖推理（SN19）、训练（SN3 / SN9 / SN56）等。
- 优点：无许可、市场化分配。问题：验证者打分主观、被操纵（weight copying）、子网质量参差、底层链（Substrate）性能一般，并且不支持原生 EVM 隐私。

## 4. 可验证计算：AI 工作量证明的核心难题

| 方案 | 开销 | 保证 | 适用 |
|---|---|---|---|
| zkML（zkLLM、OpenLLM 2026） | 2000 token 生成估计需数周证明 | 密码学级 | 小模型 / 关键步骤抽查 |
| TOPLOC（局部敏感哈希） | 约 0.26ms/token、8B/token | 能检测换模型、换精度、篡改 | 推理 |
| Verde / RepOps（Gensyn） | 需要确定性算子，性能有损 | 裁判式委托 + 争议二分 | 训练、推理 |
| 冗余执行 + 抽样 + 罚没 | 抽样比例 × 1 | 经济安全 | 通用 |
| TEE 远程证明 | 2–5% | 信任硬件厂商 | 推理 / 隐私 |

**结论**：AI 工作**无法**像 SHA256 那样廉价、确定地验证（浮点不确定性、异构硬件），所以不能直接当共识的出块证明。主流做法是 **“PoS/BFT 共识 + 有用工作层（乐观验证 + 争议 + 罚没）”**。

## 5. 高性能 EVM

- Monad（L1，2025-11 主网）：400ms 出块、约 800ms 最终性，乐观并行执行，MonadBFT。
- MegaETH（L2）：10ms 块，理论 10 万 TPS（单排序器）。
- Sei V2：并行 EVM，约 1.25 万 TPS。
- 启示：链上 TPS 已经不是瓶颈。**推理的每个 token 不应该上链**，链只负责注册、调度承诺、支付通道、结算、罚没。

## 6. 隐私

- **链层隐私**：Zcash（Orchard；Tachyon 升级面向无状态、可扩展和后量子隐私）、Aztec（2026-04 Alpha 主网，私有智能合约，块时间 36–72s，已披露存在严重漏洞）、Oasis Sapphire / Secret（TEE 机密 EVM）。
- **推理隐私**：
  - GPU TEE：H100 / H200 / B200 / GB200 支持机密计算，开销 2–5%（Blackwell 调优后 1–3%）。Phala 已经在 OpenRouter 上提供服务。**消费级显卡不支持机密计算。**
  - FHE / MPC：对 LLM 慢几个数量级，短期不可用。
  - 风险：TEE 侧信道、厂商信任根、2026 年已发现部分 TEE 屏蔽推理存在密钥复用漏洞。
- 张力：**EVM 可组合性要求公开状态**，隐私要求加密状态。二者需要分层设计（公开 / 私有双状态，参考 Aztec），或者用 TEE 机密 EVM（性能好，但信任硬件）。

## 7. 抗量子

- NIST 2024 定稿：ML-KEM（FIPS 203）、ML-DSA（FIPS 204）、SLH-DSA（FIPS 205）；FN-DSA（Falcon）标准化进行中。
- 尺寸对比：Ed25519 签名 64B / 公钥 32B；ML-DSA-44 签名约 2.4KB / 公钥约 1.3KB；Falcon-512 签名约 666B / 公钥约 897B；SLH-DSA 签名 7.8–17KB。对带宽和 TPS 有直接影响。
- 以太坊路线：以账户抽象作为迁移主路径（EIP-8141，Hegotá 分叉）；共识层用 leanXMSS（哈希签名）+ zkVM 聚合；核心 PQ 基础设施目标约 2029 年。
- **“只换一个模块”的真实边界**：
  1. 签名可以通过“算法 ID + 账户抽象”做到可插拔，但**已有账户必须主动迁移**，沉睡账户和丢失私钥的账户是历史遗留风险。
  2. **加密（隐私数据）存在“先存储、后解密”（HNDL）风险**：今天用 ECDH 加密的链上隐私数据，将来会被量子计算机回溯解密。所以隐私链的加密从第一天就必须用 **ML-KEM + X25519 混合**，不能后补。
  3. ZK 证明系统：基于配对 / KZG 的 SNARK 不抗量子，应从一开始就选基于哈希的 STARK / FRI 系列。
  4. 共识签名聚合：BLS 不抗量子，需要预留哈希签名 + 证明聚合的路径。

## 8. 初步判断（待讨论）

1. 预训练前沿基座模型是最难、离现实最远的一环，建议放在路线图最后；**推理 → RL 后训练 → 微调 → 预训练**依次推进。
2. “普惠前沿智能”短期内等于“最好的开源权重模型 + 网络自训模型”，不可能包括闭源模型。
3. 共识和有用工作解耦：快速 BFT/PoS 共识负责安全和最终性；有用工作层负责排放奖励。
4. 分级服务：机密级（数据中心 GPU + TEE）/ 标准级（冗余验证）/ 批处理级（消费级 GPU）。
5. 抗量子：加密从第一天就混合 PQ，签名做成可插拔，证明系统选抗量子的。

## 来源

- Epoch AI：How far can decentralized training over the internet scale? https://epoch.ai/gradient-updates/how-far-can-decentralized-training-over-the-internet-scale
- Spheron：Pluralis / Prime Intellect / Nous Psyche 2026 https://www.spheron.network/blog/decentralized-llm-training-pluralis-prime-intellect-nous-psyche/
- Covenant-72B https://bex.co/blog/2026/03/13/templar-covenant-72b-bittensor-largest-decentralized-llm-pretraining
- Pink Brains：State of Decentralized AI 2026 https://pinkbrains.io/blogs/the-state-of-decentralized-ai
- Gensyn Verde https://www.gensyn.ai/articles/verde · https://arxiv.org/pdf/2502.19405 · https://ownyourmind.ai/projects/gensyn/
- Bittensor dTAO https://www.coingecko.com/learn/top-bittensor-subnets-dtao
- DePIN 收入 https://blockeden.xyz/blog/2026/03/12/depin-compute-revenue-pivot-akash-ionet-aethir/ · https://ownyourmind.ai/tokenomics/render-vs-akash-vs-ionet/
- TOPLOC https://arxiv.org/pdf/2501.16007 · OpenLLM https://eprint.iacr.org/2026/1578 · Equilibrium https://equilibrium.co/writing/state-of-verifiable-inference
- Monad / MegaETH https://blockeden.xyz/blog/2026/04/18/monad-vs-megaeth-high-performance-evm-mainnet-battle-2026/
- GPU 机密计算 https://www.spheron.network/blog/confidential-gpu-computing-nvidia-tee-encrypted-vram/ · https://arxiv.org/pdf/2608.26575 · https://phala.com/posts/GPU-TEEs-is-Alive-on-OpenRouter
- Aztec https://www.kucoin.com/news/flash/aztec-network-launches-alpha-mainnet-first-ethereum-l2-with-full-privacy-smart-contracts · https://bex.co/blog/2026/03/13/aztec-network-tge-noir-language-privacy-l2-mainnet
- Zcash Tachyon https://tachyon.z.cash/ · https://www.coindesk.com/research/building-the-zcash-machine-tachyon-and-quantum-readiness
- 以太坊 PQ https://ethereum.org/roadmap/security/quantum-resistance/ · https://pq.ethereum.org/
- PoUW https://arxiv.org/html/2606.24942 · https://arxiv.org/pdf/2606.06700
