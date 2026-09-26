> 🌐 **English** | [简体中文](02-polkadot-consensus-privacy-tee.zh-CN.md)

# Research Round 2: Polkadot/JAM, Consensus, the Privacy Boundary, TEEs

> Status: discussion draft (Round 2). Date: 2026-09.

## Decisions confirmed (from round 1)

| # | Decision |
|---|---|
| D1 | Build for the future: decentralized pre-training is ~1000× behind today, but the protocol must architecturally support training and keep evolving |
| D2 | Reward only work that the network actually used, that someone paid for, and that passed verification — never "being online" or "being connected" |
| D3 | Permissionless onboarding + tiers: consumer GPUs, TEE GPUs, CPUs and storage can all "mine", taking task tiers that match their capabilities |
| D4 | Post-quantum from day one; the implementation may be imperfect but must be smoothly replaceable |
| D5 | Roadmap: serve the best open-weight models first, then progressively train our own, aiming to match closed models |
| D6 | Protocol neutrality; content policy is decided by each service provider |
| D7 | Own L1, leaning towards Substrate (Polkadot SDK) or JAM |
| D8 | Year-one use case: anyone calls top open-model APIs anonymously from a wallet; on-chain-payable inference for agents |
| D9 | No premine; possibly fundraising and a foundation; Bitcoin-style fixed supply cap |
| D10 | One person + AI, built from scratch |

---

## 1. Polkadot / JAM / Polkadot Cloud

### 1.1 JAM (Join-Accumulate Machine)

- **Positioning**: generalizes Polkadot from "relay chain + parachains" into a "decentralized computer". Parachains become just one service (CoreChains), alongside CoreVM (a general VM) and CorePlay (an actor model).
- **Computation model**:
  - **Refine** (off-chain, parallel on "cores", heavy compute, stateless) → produces a work report
  - **Accumulate** (on-chain, lightweight, stateful) → folds results into global state
  - The execution environment is **PVM** (RISC-V based): deterministic and gas-metered
- **Security model ELVES**: a small group of validators (the backing group) executes first and vouches; then validators across the network audit **randomly and covertly**; any reported discrepancy escalates to a full re-check and the misbehaving party is slashed. Data availability comes from erasure-coded shards (the DA layer).
- **Block production**: SAFROLE (simplified from SASSAFRAS), anonymous leader selection based on Ring-VRF and zkSNARKs, nearly fork-free, **6-second slots**. Finality comes from GRANDPA in about 3 blocks, i.e. ~18 seconds.
- **Scale**: designed for up to 1023 validators and 341 cores; a 1023-node network has been run, and PolkaJAM was stress-tested using 64 cores; the Gray Paper estimates ~150 billion gas/s.
- **Progress**: Gray Paper v0.8 (end of 2025), final pre-audit draft in early 2026; 43 implementation teams, 15 submissions; public testnet in July 2026; **migrating parachains to JAM targets production in 2027** and requires passing an OpenGov referendum.

### 1.2 Polkadot Hub / Polkadot Cloud

- **Polkadot Hub**: a system parachain with dual VMs — **REVM** (an EVM in Rust; Solidity tooling works unchanged) and **PVM** (RISC-V, for compute-heavy contracts) — in the `pallet-revive` module.
- **Block time**: in 2026-01 the Hub went from 6 to 2 seconds; elastic scaling (one parachain using several cores at once) targets 500ms (needing about 12 cores). Yapchain has demonstrated 500ms blocks end to end.
- **Polkadot Cloud**: the official rebranding of Polkadot as a whole (coretime, Hub, JAM services) as a "decentralized cloud"; in essence it sells **coretime**. Note that it sells **deterministic CPU compute and data availability**, **not GPU compute**.
- **March 2026**: DOT's first "halving" cut annual issuance by about 54% and set a hard cap of 2.1 billion — consistent with your Bitcoin-style hard-cap idea.

### 1.3 A misconception to clear up: speed

| Metric | Polkadot Hub | JAM | Monad | Solana Alpenglow |
|---|---|---|---|---|
| Block / soft confirmation | 2s, 500ms target | 6s slots | 400ms | ~400ms slots |
| **Finality** | Depends on the relay chain's GRANDPA: **over ten seconds** | ~18s | ~800ms | **100–150ms** (testnet) |
| Strength | Throughput (multi-core parallelism) | Throughput (341 cores) | Low latency + EVM | Lowest finality latency |

**Conclusion**: Polkadot's and JAM's strength is **throughput and parallelism**, not **finality latency**. The 500ms / 600ms figures are parachain block intervals, not final confirmation. JAM's slot is 6 seconds, no shorter than today's Polkadot. For sub-second finality, look to Monad (MonadBFT) and Alpenglow (Votor), and replace GRANDPA with a HotStuff-2 / Jolteon-family BFT.

**What it means for us**: inference experience does not depend on chain finality. Inference runs on off-chain prepaid credits or payment channels, and the chain only settles. Chain latency mainly affects atomic settlement in DeFi and between agents; a target of "500ms blocks, ≤1s finality" is already well ahead.

### 1.4 Substrate, JAM, or a Polkadot parachain?

| Option | Pros | Cons | Verdict |
|---|---|---|---|
| **A. Polkadot SDK standalone chain (solochain)** | Mature production framework; **forkless runtime upgrades** (the WASM runtime is replaced on chain — natural support for "pluggable, painless upgrades"); pluggable consensus; `pallet-revive` (EVM); crypto abstractions (`MultiSignature` / `sp_application_crypto`) extensible with new algorithms | GRANDPA/BABE finality is slow and must be replaced with a fast BFT ourselves; large codebase | **Recommended starting point** |
| B. Polkadot parachain | Shared security, elastic scaling | Depends on DOT and coretime; crypto primitives constrained by the relay chain, so **the PQ timeline is decided by Polkadot**; contradicts "own L1, sovereignty" | Not recommended |
| C. Fork JAM | Most advanced architecture; refine/accumulate fits AI work very well | Production only in 2027; the spec is still converging; complex — a one-person team cannot maintain a JAM node; SAFROLE depends on Bandersnatch Ring-VRF (elliptic curves, **not post-quantum**) | **Not now**, but **borrow its design patterns** |

**Recommendation**: build a standalone L1 on the Polkadot SDK and design the AI work layer on JAM's **refine (off-chain heavy compute) / accumulate (on-chain settlement) + ELVES (random audits + escalated re-checks + slashing)** pattern. Once JAM matures, evaluate migrating or becoming a JAM service.

Also, **Quantus Network** (a post-quantum chain based on Substrate, using Dilithium / ML-DSA signatures and STARK-style proofs) can serve as a reference for "PQ signatures on Substrate"; details to be verified.

---

## 2. Consensus in depth

### 2.1 Core constraints

1. AI work **cannot** serve as proof for block production: verification is expensive, floating-point results are non-deterministic, heterogeneous hardware disagrees, and there is latency.
2. D2 pays only for "paid + verified" work, which creates a **self-dealing problem**: miners pay themselves to farm emission. Filecoin's Fil+ "10× power boost for verified deals" was gamed at scale by fake clients.
3. D9 requires a Bitcoin-style fixed cap, but Bitcoin **emits on schedule regardless of demand**, which conflicts with D2 and must be reconciled.

### 2.2 Candidate designs

| Option | Description | Pros | Cons |
|---|---|---|---|
| **A. Pure PoS + fast BFT** | Stakers produce blocks and vote; AI work only affects emission | Fast finality, most mature | "Miners" are separate from block producers, unlike Bitcoin's ethos; capital decides security |
| **B. Resource-weighted consensus (Filecoin EC style)** | Block-production eligibility weighted by verified useful work | Contributing resources = participating in consensus | Compute is a "flow" not a "stock" (storage can be proven continuously; compute vanishes once done), hard to prove continuously; self-dealing can buy consensus power directly |
| **C. Hybrid: PoS-BFT for security + useful work drives emission** | BFT validators must stake; most emission goes to AI workers, a small part to validators | Security and incentives decoupled, each can be optimized and upgraded independently | Two economic models to maintain |
| **C+. Hybrid + work-weighted validator selection** | Validator weight = f(stake, recent verified work score) | Lets "miners" into the consensus layer too | Complex; phase two |
| D. Pure PoW (Quantus style) | Hash puzzles | Closest to Bitcoin | Wastes compute, contradicts "useful work" |

**Recommendation: start with C and reserve C+.** The consensus layer uses a HotStuff-2 / Jolteon-class BFT (in the spirit of MonadBFT and Alpenglow), ~100 validators, targeting 500ms blocks and sub-second finality. Signatures are ML-DSA; since PQ signatures cannot be BLS-aggregated, accept O(n) signature overhead at first and later aggregate with STARKs (following Ethereum's leanSig / leanMultisig).

### 2.3 Reconciling a Bitcoin-style cap with "useful work": a draft

- **Total supply 2.1 billion** (number to be decided), with a **maximum emission curve** `E_max(t)` that halves Bitcoin-style.
- Actual emission per epoch `E(t) = min(E_max(t), k × fees of verified paid work)`.
- **Unemitted amounts are neither burned nor given to a foundation; they roll into a "future emission pool"** and are released later. The cap stays unchanged and emission strictly follows real demand.
- **Anti-self-dealing**:
  - **Burn** part of user payments (like EIP-1559) so that "paying yourself" always loses money: require `k × fee < fee`, or design it so the self-dealer's expected return is negative.
  - Work rewards are computed as `min(contribution share, payer-diversity factor)`: the more concentrated the payers, the smaller the reward.
  - Random audits + stake slashing (ELVES style) punish fabricated work.
- **Validators (security budget)**: a fixed share of emission (e.g. 10–20%) to BFT validators, plus transaction fees.
- **Cold start**: little demand early means little emission and weak miner motivation. Two possible mitigations: the foundation or DAO buys inference (real demand), or "open model-training jobs" where the DAO is the payer and the emission pool funds training a community model. The latter is itself "building for the future".

### 2.4 Resource tiers and task matrix

| Tier | Hardware | Tasks | Verification |
|---|---|---|---|
| T0 confidential | H100 / H200 / B200 + CPU TEE | Private inference, hosted inference of closed weights | TEE remote attestation + TOPLOC spot checks |
| T1 data center | A100 / H100 (CC off) | Interactive large-model inference, training nodes | TOPLOC + redundant spot checks + stake |
| T2 consumer | RTX 3090 / 4090 / 5090, Mac | Small-model inference, **RL rollouts**, embeddings, data cleaning, evaluation, data-parallel nodes in decentralized training | Redundant execution + sampling + RepOps-style determinism |
| T3 CPU | Servers and PCs | Verification audits, ZK proof generation, data preprocessing, RPC | Deterministic re-execution |
| T4 storage | Disks | Distribution of model weights, checkpoints, datasets | Storage proofs (PoRep / PoSt style) |

---

## 3. The privacy boundary, item by item

| Object | Benefit of default privacy | Cost / risk | Recommendation |
|---|---|---|---|
| **Payment relationship** (who paid for which inference) | The core of "anonymous API calls": identity cannot be linked to prompts | Requires a shielded pool + ZK; clients must generate proofs | **Private by default** (must ship in year one) |
| **Transfer amounts and parties** | Zcash-level financial privacy | EVM contracts cannot read private balances directly; exchange-listing and compliance pressure; PQ shielded-pool proofs are larger (STARKs ~50–200 KB) | **Dual state**: public accounts (EVM) + native shielded pool, user's choice; view keys for selective disclosure |
| **Contract state** | Private DeFi, private agent strategies | A fully private EVM needs TEEs (the Oasis approach) or FHE (orders of magnitude slower); Aztec needs a new language (Noir) and its 2026 Alpha still has serious vulnerabilities | **Public in year one**; private contracts as phase-two research (Noir or private PVM execution) |
| **Prompts / outputs** | The privacy users care about most | Pure-crypto approaches are unusable; TEEs carry trust assumptions; non-TEE nodes necessarily see plaintext | **Per tier**: T0 confidential (TEE) or standard (node sees plaintext but cannot link it to an identity); prompts **never stored on chain** |
| **Model weights** | Lets closed or commercial weights onto the network, expanding supply | TEE only; a leaked weight cannot be recalled | Open weights public; private weights only on T0 |
| **Network metadata** (IP, timing) | Prevents de-anonymization by traffic correlation | Mixnets add latency | Optional Nym or Tor-style relays on the client; gateways reachable via relays |

**Key design: Anonymous Inference Credits**

1. The user deposits tokens from a public account or the shielded pool and mints a batch of **unlinkable inference credits** (ZK + nullifiers, akin to Privacy Pass / blind-signature tokens).
2. Each inference call carries a one-time voucher that the provider cannot link to the depositor.
3. Providers redeem vouchers in batches and settle on chain; nullifiers prevent double spends.
4. Agents: an agent holds a voucher wallet and pays per call without going on chain each time.

This directly serves D8 (the year-one use case). Every primitive can be built from hashes (Poseidon2) + STARKs, **post-quantum by construction**, with note encryption using ML-KEM + X25519 hybrid.

---

## 4. TEEs: pros and cons

### Pros
1. **Nearly no performance loss**: 2–5% overhead for H100 CC, 1–3% on tuned Blackwell; remote attestation is a one-off ~1–3 seconds.
2. **Existing inference stacks work as is**: vLLM / SGLang run inside a confidential VM with minimal engineering.
3. **Two birds with one stone**: remote attestation proves "which model and which code is running", giving **privacy and verifiability** together.
4. **Protects model weights**, allowing closed or commercial models onto the network and expanding frontier supply.
5. Precedent: Phala offers GPU TEE inference on OpenRouter; Meta, Microsoft and Google are all building confidential inference pipelines.

### Cons
1. **The root of trust is the vendor**: NVIDIA, Intel and AMD hold the attestation keys and the power to revoke, and all are subject to US export controls. **This is exactly the centralized power you oppose**: vendors can revoke attestation for devices in a region, and H100 / B200 are themselves export-controlled, so **the T0 tier is inherently geographically concentrated**.
2. **Mismatched threat model (the most important point)**: TEEs are designed against "other cloud tenants and compromised system software"; they **do not defend against a machine owner with physical access**. In a permissionless network, **the node operator is precisely the potential attacker with physical access**.
   - WireTap / Battering RAM (2025): DDR4 memory-bus interposers costing $50–1,000 break SGX.
   - **TEE.fail (2025-10)**: breaks Intel TDX and AMD SEV-SNP on DDR5; **extracted attestation keys can forge TEE quotes and thereby fool NVIDIA GPU confidential computing**, letting attackers impersonate a TEE in a completely unprotected environment.
   - DDRop (2026-09): yet another attack breaking TDX and SEV-SNP.
   - Root cause: for performance, server-grade TEEs use deterministic AES-XTS encryption and dropped integrity and replay protection.
3. Side channels and firmware bugs will keep appearing, each requiring revocations and patches.
4. Consumer GPUs are not supported; T0 can only be high-end data-center cards.

### Conclusion
In our network a TEE can only be **"a layer that raises the cost of attack", not "a privacy guarantee"**. Mitigations (defense in depth):
- T0 nodes post **high stakes**, slashed on provable compromise (e.g. the same attestation key appearing in several places);
- An **on-chain revocation list** of attestation keys and firmware versions, with a fast DAO response;
- T0 nodes may publish data-center operator details (a reputation layer, not mandatory);
- On the user side, **separate identity privacy from content privacy**: even if a prompt leaks, it cannot be linked to a wallet or identity (thanks to anonymous vouchers and relays);
- Track GPU-side integrity protection, open-source TEEs (Keystone, the OpenTitan family) and FHE acceleration over the long run as pluggable privacy backends.

---

## 5. Year-one MVP for one person + AI (draft)

1. **Chain**: Polkadot SDK standalone chain; get it running with Aura + GRANDPA first, then replace with a fast BFT; account signatures support `ML-DSA` + `Ed25519` hybrid from the start (with algorithm IDs); integrate `pallet-revive` for EVM.
2. **Native shielded pool + anonymous inference vouchers**: STARK proofs + Poseidon2 + ML-KEM hybrid encryption.
3. **Inference market pallet**: provider registration (tier, models, prices, stake) → voucher redemption and settlement → random audits (TOPLOC) → slashing.
4. **Inference gateway**: OpenAI-compatible API, streaming output, routing by latency and price; the client SDK manages vouchers automatically.
5. **Emission pallet**: hard cap + demand-driven emission + burning + concentration decay.

## Sources

- JAM: https://wiki.polkadot.com/learn/learn-jam-chain/ · https://blockeden.xyz/blog/2026/01/16/polkadot-jam-architecture-blockchain-virtual-machine-paradigm-shift/ · https://blockeden.xyz/blog/2025/10/28/jam-chain-polkadot-s-paradigm-shift-toward-the-decentralized-global-computer/ · https://www.hokanews.com/2026/09/polkadot-prepares-major-jam-transition.html · https://coinbureau.com/review/polkadot-dot · https://github.com/openguild-labs/learn-jam
- Polkadot Hub: https://docs.polkadot.com/reference/polkadot-hub/smart-contracts/ · https://openguild.wtf/blog/polkadot/polkadot-introducing-about-dual-vm-architecture-polkadot-hub · https://blockchain.news/flashnews/polkadot-hub-to-add-evm-pvm-smart-contracts-and-2-second-blocks-on-jan-20-2026
- Elastic scaling / 500ms: https://forum.polkadot.network/t/elastic-scaling-wen-500ms-blocks/11971 · https://wiki.polkadot.com/learn/learn-elastic-scaling/ · https://medium.com/polkadot-network/polkadot-roundup-2025-3c3c71c7e9c4
- Consensus: https://wiki.polkadot.com/learn/learn-consensus/ · https://spec.filecoin.io/algorithms/expected_consensus/ · https://www.helius.dev/blog/alpenglow · https://solana.com/alpenglow
- Polkadot PQ roadmap: https://medium.com/@gwrx2005/post-quantum-roadmaps-for-blockchain-ecosystems-af9e77a6fe8b
- TEE attacks: https://www.bleepingcomputer.com/news/security/teefail-attack-breaks-confidential-computing-on-intel-amd-nvidia-cpus/ · https://thehackernews.com/2026/09/new-ddrop-attack-breaks-intel-tdx-and.html · https://hacken.io/insights/wiretap-and-battering-ram-risks/ · https://arxiv.org/pdf/2507.02770
