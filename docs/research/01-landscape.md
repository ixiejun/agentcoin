> 🌐 **English** | [简体中文](01-landscape.zh-CN.md)

# Research Round 1: Decentralized AI Compute × High-Performance EVM × Privacy × Post-Quantum

> Status: discussion draft (Round 1); no design decisions yet. Date: 2026-09.

## 0. Vision restated

Build a permissionless public chain / DAO:

1. **Resource layer**: data-center GPUs, consumer graphics cards, CPUs and storage join permissionlessly, contributing resources like Bitcoin miners and earning native tokens according to work done.
2. **Full AI pipeline**: pre-training → post-training (SFT / RL) → inference, on par with centralized clouds, with no compromise in experience.
3. **Application layer**: EVM compatible, supporting DeFi, AI, agents and other DApps that bring in demand.
4. **Performance**: commercial high-availability grade.
5. **Post-quantum**: pluggable cryptography — mature schemes at first, seamless switch to PQC later.
6. **Privacy**: a privacy-native public chain.

## 1. Decentralized training: state of the art and the gap

| Project | Flagship result | Key technique | Degree of decentralization |
|---|---|---|---|
| Templar (Bittensor SN3) | Covenant-72B, ~1.1T tokens, 70+ nodes | Low-communication gradient compression | Permissionless |
| Nous Psyche | Consilience 40B | DisTrO, coordinated on Solana | Permissioned testnet |
| Prime Intellect | INTELLECT-1 (10B), INTELLECT-2 (32B async RL), INTELLECT-3 (106B MoE) | OpenDiLoCo, PRIME-RL, TOPLOC | Whitelisted; INTELLECT-3 was actually trained on a centralized 512-GPU cluster |
| Pluralis | Protocol Model 8B | Model parallelism + protocol learning | Curated |
| Gensyn | Mainnet 2026-04 (OP Stack L2) | Verde / RepOps bitwise-reproducible verification | Permissionless |

**Conclusions**
- Epoch AI estimates that the largest decentralized pre-training runs (6e22–6e23 FLOP) used **about 1000× less** compute than frontier models. The bottlenecks are internet bandwidth, stragglers and verification cost.
- **RL post-training is naturally suited to decentralization**: rollouts (generation) are essentially inference and can be massively distributed; only policy updates need central synchronization (demonstrated by INTELLECT-2).
- Consumer GPU memory (24–32 GB) cannot hold frontier MoE models (600B+); consumer cards can do small models, rollouts, embeddings and data processing, or join pipeline / expert parallelism (with high latency).

## 2. Decentralized GPU markets (DePIN): demand is the bottleneck

- In early 2026 the whole sector's annualized protocol revenue was about $200M; Aethir about $150M ARR (mostly enterprise), io.net about $20M, Akash about $4.3M.
- **Declared supply far exceeds paid utilization**: after io.net's cleanup only 5,350 of 327,000 registered devices passed verification; in Q1 2026 Akash had 84 of 334 GPUs in use.
- Lesson: subsidizing supply with token emission alone attracts wash-trading / Sybil miners. **Incentives must be anchored to verified, paid, useful work.**

## 3. Lessons from Bittensor

- dTAO (2025-02): each subnet issues an Alpha token and the market decides emission allocation. Subnets cover inference (SN19), training (SN3 / SN9 / SN56) and more.
- Strengths: permissionless, market-based allocation. Problems: subjective and manipulable validator scoring (weight copying), uneven subnet quality, a base chain (Substrate) with mediocre performance, and no native EVM privacy.

## 4. Verifiable computation: the core problem of AI proof-of-work

| Approach | Overhead | Guarantee | Suitable for |
|---|---|---|---|
| zkML (zkLLM, OpenLLM 2026) | Weeks of proving estimated for a 2000-token generation | Cryptographic | Small models / spot checks of key steps |
| TOPLOC (locality-sensitive hashing) | ~0.26ms/token, 8 B/token | Detects model swaps, precision changes, tampering | Inference |
| Verde / RepOps (Gensyn) | Requires deterministic operators; performance cost | Refereed delegation + bisection disputes | Training, inference |
| Redundant execution + sampling + slashing | Sampling ratio × 1 | Economic security | General |
| TEE remote attestation | 2–5% | Trust in the hardware vendor | Inference / privacy |

**Conclusion**: AI work **cannot** be verified cheaply and deterministically like SHA256 (floating-point non-determinism, heterogeneous hardware), so it cannot serve directly as proof for block production. The mainstream approach is **"PoS/BFT consensus + a useful-work layer (optimistic verification + disputes + slashing)"**.

## 5. High-performance EVM

- Monad (L1, mainnet 2025-11): 400ms blocks, ~800ms finality, optimistic parallel execution, MonadBFT.
- MegaETH (L2): 10ms blocks, theoretical 100k TPS (single sequencer).
- Sei V2: parallel EVM, ~12.5k TPS.
- Lesson: on-chain TPS is no longer the bottleneck. **Individual inference tokens should never go on chain**; the chain only handles registration, scheduling commitments, payment channels, settlement and slashing.

## 6. Privacy

- **Chain-level privacy**: Zcash (Orchard; the Tachyon upgrade targets statelessness, scalability and post-quantum privacy), Aztec (Alpha mainnet 2026-04, private smart contracts, 36–72s blocks, disclosed serious vulnerabilities), Oasis Sapphire / Secret (TEE confidential EVM).
- **Inference privacy**:
  - GPU TEE: H100 / H200 / B200 / GB200 support confidential computing with 2–5% overhead (1–3% on tuned Blackwell). Phala already serves on OpenRouter. **Consumer GPUs do not support confidential computing.**
  - FHE / MPC: orders of magnitude too slow for LLMs, unusable in the short term.
  - Risks: TEE side channels, vendor roots of trust; in 2026 a key-reuse vulnerability was found in some TEE-shielded inference setups.
- Tension: **EVM composability needs public state** while privacy needs encrypted state. This calls for a layered design (public / private dual state, as in Aztec) or a TEE confidential EVM (fast, but trusts hardware).

## 7. Post-quantum

- NIST finalized in 2024: ML-KEM (FIPS 203), ML-DSA (FIPS 204), SLH-DSA (FIPS 205); FN-DSA (Falcon) standardization is in progress.
- Sizes: Ed25519 signature 64 B / public key 32 B; ML-DSA-44 signature ~2.4 KB / public key ~1.3 KB; Falcon-512 signature ~666 B / public key ~897 B; SLH-DSA signature 7.8–17 KB. This directly affects bandwidth and TPS.
- Ethereum's path: account abstraction as the main migration route (EIP-8141, Hegotá fork); on the consensus layer, leanXMSS (hash-based signatures) + zkVM aggregation; core PQ infrastructure targeted around 2029.
- **The real limits of "just swap a module"**:
  1. Signatures can be made pluggable with "algorithm ID + account abstraction", but **existing accounts must migrate actively**; dormant accounts and accounts with lost keys remain a legacy risk.
  2. **Encryption (private data) faces "harvest now, decrypt later" (HNDL)**: private on-chain data encrypted today with ECDH can be decrypted retroactively by future quantum computers. A privacy chain must therefore use **ML-KEM + X25519 hybrid** encryption from day one; it cannot be added later.
  3. ZK proof systems: pairing / KZG-based SNARKs are not post-quantum; hash-based STARK / FRI systems should be chosen from the start.
  4. Consensus signature aggregation: BLS is not post-quantum; a path for hash-based signatures + proof aggregation must be reserved.

## 8. Preliminary judgments (for discussion)

1. Pre-training frontier base models is the hardest and furthest from reality; it belongs at the end of the roadmap: **inference → RL post-training → fine-tuning → pre-training**.
2. In the short term, "democratized frontier intelligence" means "the best open-weight models + models trained by the network"; closed models cannot be included.
3. Decouple consensus from useful work: fast BFT/PoS consensus provides security and finality; the useful-work layer drives emission rewards.
4. Tiered service: confidential (data-center GPU + TEE) / standard (redundant verification) / batch (consumer GPUs).
5. Post-quantum: hybrid PQ encryption from day one, pluggable signatures, post-quantum proof systems.

## Sources

- Epoch AI: How far can decentralized training over the internet scale? https://epoch.ai/gradient-updates/how-far-can-decentralized-training-over-the-internet-scale
- Spheron: Pluralis / Prime Intellect / Nous Psyche 2026 https://www.spheron.network/blog/decentralized-llm-training-pluralis-prime-intellect-nous-psyche/
- Covenant-72B https://bex.co/blog/2026/03/13/templar-covenant-72b-bittensor-largest-decentralized-llm-pretraining
- Pink Brains: State of Decentralized AI 2026 https://pinkbrains.io/blogs/the-state-of-decentralized-ai
- Gensyn Verde https://www.gensyn.ai/articles/verde · https://arxiv.org/pdf/2502.19405 · https://ownyourmind.ai/projects/gensyn/
- Bittensor dTAO https://www.coingecko.com/learn/top-bittensor-subnets-dtao
- DePIN revenue https://blockeden.xyz/blog/2026/03/12/depin-compute-revenue-pivot-akash-ionet-aethir/ · https://ownyourmind.ai/tokenomics/render-vs-akash-vs-ionet/
- TOPLOC https://arxiv.org/pdf/2501.16007 · OpenLLM https://eprint.iacr.org/2026/1578 · Equilibrium https://equilibrium.co/writing/state-of-verifiable-inference
- Monad / MegaETH https://blockeden.xyz/blog/2026/04/18/monad-vs-megaeth-high-performance-evm-mainnet-battle-2026/
- GPU confidential computing https://www.spheron.network/blog/confidential-gpu-computing-nvidia-tee-encrypted-vram/ · https://arxiv.org/pdf/2608.26575 · https://phala.com/posts/GPU-TEEs-is-Alive-on-OpenRouter
- Aztec https://www.kucoin.com/news/flash/aztec-network-launches-alpha-mainnet-first-ethereum-l2-with-full-privacy-smart-contracts · https://bex.co/blog/2026/03/13/aztec-network-tge-noir-language-privacy-l2-mainnet
- Zcash Tachyon https://tachyon.z.cash/ · https://www.coindesk.com/research/building-the-zcash-machine-tachyon-and-quantum-readiness
- Ethereum PQ https://ethereum.org/roadmap/security/quantum-resistance/ · https://pq.ethereum.org/
- PoUW https://arxiv.org/html/2606.24942 · https://arxiv.org/pdf/2606.06700
