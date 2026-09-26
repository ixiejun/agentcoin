> 🌐 **English** | [简体中文](full-technical-plan.zh-CN.md)

# AgentCoin Full Technical Plan v0.1

> Status: first draft, under review. Date: 2026-09.
> Basis: `docs/decisions.md` (D1–D37). This document describes the target architecture after the MVP (P2–P4) and **which interfaces reserved in the MVP it depends on**.
> MVP details are in `mvp-technical-plan.md` and are not repeated here.

---

## 0. Full-version goals

AgentCoin's end state: a **post-quantum, privacy-native, permissionless** L1 covering the full large-model pipeline of **pre-training → post-training → inference**, powered by the world's heterogeneous compute (data-center GPUs, consumer GPUs, CPUs, storage), with a service experience on par with centralized clouds; the EVM ecosystem, the agent economy and a bicameral DAO run on top of it.

### 0.1 Phase overview

| Phase | Timing (after mainnet, rough) | Theme | Signature capabilities |
|---|---|---|---|
| **P2 Service expansion** | +0–18 months | Faster, more private, able to train | 500ms blocks / ≤1s finality; TEE confidential inference; RL post-training; fine-tuning; dedicated storage layer; private voting; sealed bids; bicameral governance |
| **P3 Training and consensus** | +12–36 months | Train our own models; miners join consensus | Decentralized pre-training; community-model royalties; work-weighted validator election; private swaps in the shielded pool; relays / mixnet; STARK bridge to Ethereum; optional stablecoin payments; oracle |
| **P4 Frontier** | +36 months and beyond | Catch up with the frontier | Large-scale decentralized MoE training; STARK signature aggregation supporting 1,000 validators; evaluate L2 general private contracts; evaluate migrating to JAM-style execution |

---

### 0.2 Development language (D31)

The full version keeps the MVP's Rust-first principle: consensus, runtime, cryptography, circuits, orchestrators (RL / fine-tuning / pre-training coordinators), storage nodes, gateways and all agents are Rust.

The **computation itself** for training and inference runs in third-party frameworks (PyTorch, vLLM, SGLang — all Python ecosystem). For these we write only **thin plugins**: extracting TOPLOC activations, exporting and importing pseudo-gradients and checkpoints, and reporting intermediate results of reproducible operators. Plugins talk to the Rust agents over local sockets or shared memory and contain no protocol or settlement logic.

For RepOps-style reproducible operators, Rust implementations are evaluated first (e.g. custom kernels on candle / Burn) to reduce dependence on Python frameworks.

## 1. Target architecture

```
┌───────────────────────────────────────────────────────────────────────────────┐
│ Application: EVM DApps · agents (session keys, HTTP 402, MCP) ·               │
│              private features (voting / bidding / swaps)                       │
├───────────────────────────────────────────────────────────────────────────────┤
│ Services                                                                       │
│  inference market (T0 confidential / T1 / T2) · RL orchestrator · fine-tuning  │
│  jobs · pre-training coordinator · storage market                              │
│  model registry (lineage + royalties) · SLA reputation · sealed-bid            │
│  procurement · oracle                                                          │
├───────────────────────────────────────────────────────────────────────────────┤
│ Verification (ELVES-style: optimistic execution → random covert audits →       │
│   escalated re-checks → slashing)                                              │
│  TOPLOC · RepOps reproducible operators + refereed disputes · redundant        │
│  execution · TEE attestation · storage challenges                              │
├───────────────────────────────────────────────────────────────────────────────┤
│ Consensus: fast/slow dual-path BFT (500ms / ≤1s) · work-weighted election ·    │
│   hash-based signatures + STARK aggregation                                    │
│ Constitution: node invariants (layer 1) · constitution + guardrails (layer 2)  │
│   · bicameral track governance (layer 3)                                       │
├───────────────────────────────────────────────────────────────────────────────┤
│ Cryptography: pluggable AlgId (ML-DSA / SLH-DSA / FN-DSA / XMSS) ·             │
│   ML-KEM hybrid · STARK                                                        │
├───────────────────────────────────────────────────────────────────────────────┤
│ Resources: T0 TEE GPU · T1 data-center GPU · T2 consumer GPU · T3 CPU ·        │
│   T4 storage                                                                   │
└───────────────────────────────────────────────────────────────────────────────┘
```

---

## 2. Consensus layer (P2–P4)

### 2.1 Fast/slow dual-path BFT (P2)

- Following the Alpenglow (Votor) approach:
  - **Fast path**: one voting round reaching ≥80% of weight finalizes directly (target 100–300ms);
  - **Slow path**: two rounds reaching ≥60% as the fallback (target ≤1s).
- Block production: 500ms slots, deterministic rotation + stake weighting; blocks are broadcast as erasure-coded shreds (similar to Rotor / Turbine) to relieve the leader's bandwidth bottleneck.
- Evolves from the MVP's AC-BFT: the vote-message and finality-proof formats remain backward compatible (versioned).

### 2.2 Consensus signatures: hash-based signatures + STARK aggregation (R&D in P2, live in P3)

- Validator signatures migrate from ML-DSA-65 to `SigAlg::XmssLean` (stateful hash-based signatures, modelled on Ethereum's leanSig) via the `rotate_session_key` transaction — **using the MVP's AlgId mechanism, with zero hard forks**.
- An aggregator folds a round's n signatures into one STARK proof (following leanMultisig / EIP-8288). The finality proof shrinks from O(n × 2.4 KB) to O(one proof), the prerequisite for scaling validators from 100 to 300 and then 1,000.
- Stateful-signature risk (reusing a one-time key): guarded by local state persistence on the node plus a self-check at startup; **if the signing-state file is corrupted, the node refuses to sign** (better offline than a double use).

### 2.3 Work-weighted validator election (P3, prerequisite for D25)

```
weight(v) = stake(v)^α × (1 + workscore(v))^β        // initially α = β = 0.5
workscore(v) = Σ_{last 30 days} USD value of verified (audited) work, capped at 5% of the network total per node
constraint: stake(v) ≥ MinValidatorStake (so slashing stays meaningful)
```

- Replaces the implementation of the MVP's `ValidatorElection` trait, whose `(stake, workscore)` input is already reserved in the MVP.
- Work scores must be **lagged** (only data past its challenge period) and **count audited work only**, to prevent burst gaming.

### 2.4 On-chain randomness upgrade

- Commit–reveal + a **hash-based VDF** (a sequential hash chain whose correct computation is proven by a STARK, post-quantum), removing the "last revealer" 1-bit bias.

---

## 3. AI service layer

### 3.1 T0 confidential inference (P2)

| Item | Design |
|---|---|
| Hardware | Confidential-computing mode on H100 / H200 / B200 / GB200 + CPU TEE (TDX / SEV-SNP) |
| Attestation verification | Auditors verify NVIDIA and CPU-vendor attestation reports off-chain → submit a signed verdict on chain (the MVP reserves `Provider.attestation`); in P3 this evolves into a STARK proving "I verified this report correctly" |
| On-chain state | `TrustedMeasurements` (allow-list of firmware, driver and image hashes), `RevokedKeys` (revocation list); the **security council fast track** may only revoke entries in these two tables |
| Stake | High bar (e.g. $10k equivalent); provable misconduct (e.g. the same attestation key appearing on two different nodes) is slashed 100% |
| User side | The client SDK verifies the attestation chain locally → encrypts with ML-KEM hybrid directly to a public key generated inside the TEE → **even the gateway cannot see the prompt in plaintext** |
| Risk positioning | TEE is only "a layer that raises the cost of attack" (research 02 §4); identity privacy is still guaranteed by anonymous vouchers |

### 3.2 SLA and reputation (P2)

- Auditors' mystery-shopper measurements accumulate into `SlaMetrics` (TTFT P50/P95, throughput, error rate, audit pass rate, uptime) with exponential-decay weighting.
- Gateway routing score = `f(price, SLA, reputation, lineage badge)`; the formula is open source, gateways may tune it, and the SDK uses the standard formula by default.

### 3.3 Sealed-bid compute procurement (P2, L1 private feature)

- Use cases: large training jobs, long-term inference capacity, bulk procurement for public jobs.
- Flow:
  1. The buyer publishes a request (spec, duration, budget cap);
  2. Providers submit **encrypted bids** in the shielded pool (commitment + STARK proof that "the bid is valid and stake is sufficient");
  3. Bids are revealed during the reveal window per the rules (second-price or first-price, chosen by the buyer);
  4. Unrevealed bids forfeit their deposit.
- Effect: providers cannot see each other's quotes and therefore cannot collude.

### 3.4 Agent economy (P2)

- **Session keys** (PQ account abstraction):
  ```
  SessionGrant { parent: AccountId, session_pk: PqPublicKey,
                 spend_limit_usd, expiry, allowed: {models, contracts, gateways}, nonce }
  ```
  Once authorized by the master key, the agent pays with the sub-key, which can be revoked at any time.
- **HTTP 402-style payments**: the gateway answers requests without a voucher with `402 Payment Required`, a quote and the accepted voucher types; the agent SDK attaches a voucher and retries automatically. Interoperable with x402-style machine-payment standards used in the industry.
- **MCP server**: exposes "paid inference", "check balance" and "mint vouchers" as MCP tools that agent frameworks can call directly.
- Optional on-chain agent identity (public key + metadata); the protocol performs no KYC (D6).

---

## 4. Training

### 4.1 RL post-training (P2, the first training capability to ship)

Following the asynchronous architecture of PRIME-RL / INTELLECT-2:

```
┌──────────────┐  policy weights (storage-layer broadcast)  ┌───────────────────────┐
│ Trainer T1    │ ─────────────────────────────────────────► │ Rollout workers T2 × N │
│ (policy update)│ ◄───────────────────────────────────────── │ (samples + TOPLOC)     │
└──────┬───────┘      rollouts + rewards + proofs            └───────────────────────┘
       │ checkpoints (storage layer)
       ▼
  RL orchestrator (on-chain jobs + off-chain coordination): dispatch, verify rollouts, settle
```

- **On-chain object**:
  ```
  RlJob { id, base_model, reward_spec_hash, env_hash, budget, policy_version,
          rollout_price, trainer_set, checkpoint_refs }
  ```
- **Verification**:
  - TOPLOC verifies that rollouts were "generated by the specified policy version";
  - reward computation (reproducible environments, graders) uses redundant execution and spot checks;
  - trainer update steps use RepOps-style reproducible operators + refereed disputes (see §4.3).
- **Payers**: users (e.g. enterprise custom models) or the DAO (public-work emission, D16).

### 4.2 Fine-tuning jobs (P2)

- LoRA and SFT jobs: `FinetuneJob { base_model, dataset_ref, method, hyperparams_hash, budget }`.
- Verification: reproducibility spot checks of the training process + evaluation on a held-out set (redundant evaluation).
- The resulting model is registered automatically with its lineage (D28).

### 4.3 Decentralized pre-training (P3–P4)

| Item | Design |
|---|---|
| Algorithm | DiLoCo / SparseLoCo-style low-communication training: many local steps + outer synchronization, pseudo-gradients quantized to 4–8 bits and sparsified (100–2000× less communication than naive data parallelism) |
| Topology | Mainly data parallel; P4 adds pipeline / expert parallelism for MoE models |
| Participants | T1 (main force); T2 when model size allows, or for data preprocessing and evaluation |
| Verification | ① **RepOps-style reproducible operators**: bitwise-identical results on heterogeneous hardware so training steps can be recomputed; ② **refereed delegation**: verifiers open a bisection dispute on a suspicious update, narrowing it to a single operator that the chain or auditors adjudicate; ③ pseudo-gradient quality spot checks (loss decrease measured on a validation set) |
| Coordination | On-chain `PretrainRun { spec_hash, data_recipe_hash, schedule, participants, checkpoints }`; training proposals are initiated by the DAO (reviewed by the contributor house + token house) and funded by public-work emission + community crowdfunding |
| Data | Public data recipes (D26 / research 05 C4); datasets stored in the dedicated storage layer with registered hashes |

### 4.4 Community-model royalties (D22, D28)

- A model is registered with `RoyaltySpec { rate: 5%, beneficiaries: ContributionLedger }`. The `ContributionLedger` is generated automatically from the audited work performed during training — a Merkle tree distributing by contribution share.
- When the model is used for inference on this network, settlement automatically diverts 5% of provider revenue to the contributors.
- **Lineage incentives**: a derived model that declares its `lineage` gets a "verified lineage" badge and routing weight, and automatically pays royalties upstream (decreasing shares, e.g. 5% to the parent and 2.5% to the grandparent); undeclared derivatives get none of these benefits. Cryptographically unenforceable — it relies entirely on incentives.

---

## 5. Dedicated storage layer (P2, D26)

| Item | Design |
|---|---|
| Scope | Only model weights, checkpoints, datasets and proof data; **no general-purpose storage** |
| Encoding | Erasure coding (e.g. Reed-Solomon, k/n = 1/3), shards spread across different nodes |
| Proofs | Randomly challenged proofs of retrievability (PoR): each epoch, on-chain randomness challenges random positions in several shards and nodes return data + Merkle paths; plus sampled download tests (measuring bandwidth). All hash-based and post-quantum |
| Distribution | Weight broadcast via BitTorrent-like P2P distribution (SHARDCAST-style), serving RL policy version updates |
| Incentives | Storage fees (paid by the payer) + market-work emission (computed from "proven storage × duration × reads"); lost data is slashed |
| On-chain object | `StorageDeal { content_root, size, redundancy, duration, price, providers[] }` |

---

## 6. Privacy (full version)

### 6.1 L1 private features (D27), all on the same shielded pool

The MVP reserves the `ShieldedAction` enum; the full version enables the variants one by one:

| Feature | Phase | Circuit essentials |
|---|---|---|
| `Transfer` / `MintVoucher` | MVP | — |
| **`Vote` (private voting)** | P2 | Proves "I hold x votes of voting power (token balance or lock-duration weighted, or work score) and vote only once" **without revealing the voter or the choice**; totals are published after tallying. **A prerequisite for the bicameral system** (prevents vote buying) |
| **`Bid` (sealed bids)** | P2 | See §3.3 |
| **`Swap` (private swaps in the shielded pool)** | P3 | Matched against a batch-auction AMM inside the shielded pool (batch clearing at a uniform price), **so privacy no longer breaks at the DEX**, and front-running disappears |

- **Audit requirement**: every new circuit is externally audited separately before it is enabled; the on-chain invariant "shielded pool total ≤ cumulative deposits − cumulative withdrawals" is permanent.

### 6.2 L2 general private contracts (reserved only; evaluate around 2028)

- The MVP reserves the `stark_verify` precompile and the calling convention "private execution → proof → verification by a public contract".
- Preconditions for evaluation: a mature **PQ-secure** proof backend plus language and tooling (the current Aztec / Noir backend is BN254-based and does not qualify).

### 6.3 Network-layer privacy (P3)

| Traffic | Approach |
|---|---|
| Inference requests | Optional 1–2 hop relays with an Oblivious-HTTP-style split: the relay knows who the user is but cannot see content; the gateway sees content but not who the user is; ≤50ms added latency |
| On-chain transactions, voucher minting | Optional mixnet (PQ Sphinx-style packet format, e.g. a KEM-hybrid Sphinx variant), latency tolerant |
| Node P2P | ML-KEM hybrid encryption (done in the MVP) |

### 6.4 Selective disclosure

- Tiered view keys: `full_view` (all income and spending), `incoming_view` (income only), `payment_proof` (proves one specific payment). Users provide them voluntarily to auditors or exchanges; the protocol never compels it.

---

## 7. Governance and constitution (full version, D25, D30)

### 7.1 Bicameral system (from P2, enabled once private voting is live)

| | Token house | Contributor house |
|---|---|---|
| Voting power | ATC balance × lock-duration weighting (conviction) | Audited work score over the last 180 days (square-rooted to dampen large mining farms) |
| Voting method | Private voting (`ShieldedAction::Vote`) | Private voting |
| Sybil resistance | Inherent (tokens cost money) | Work scores require real paid, audited work; optional stake threshold |

**Tracks**

| Track | Power | Passing condition | Enactment delay |
|---|---|---|---|
| Signal | Executes no code, only expresses intent | Simple majority in the token house | — |
| Parameters | Adjust parameters within guardrails | Token house passes + contributor house does not veto | 7 days |
| Treasury (holders) | Large spending | Token house passes; above a threshold both houses must pass | 7 days |
| Community grants | Small grants | Approved by an independent grants committee (elected by both houses) | — |
| Runtime upgrade | Replace the runtime | **Both houses pass** + a public code-diff report | **28 days** |
| Constitutional amendment (layer 2) | Change the constitution text or guardrails | ≥75% in each house + minimum turnout | **90 days** |
| Security council | **Can only pause modules and revoke TEE attestations** | 5-of-7 multisig (elected by both houses) | Immediate; must be ratified by a full vote within 14 days or it lapses automatically |
| Layer-1 invariants | **No track** — hard fork only | — | — |

### 7.2 Three constitutional layers (D30)

- **Layer 1 (node-enforced)**: supply cap, emission curve / no premine, PoA → PoS switch — implemented in the MVP and unchanged in the full version.
- **Layer 2 (constitution + guardrails)**: protocol neutrality, parameter bounds; reviewed by both houses; the contributor house makes "pure capital capture" harder.
- **Layer 3 (day-to-day governance)**: see the track table above.

---

## 8. Interoperability and pricing

### 8.1 Oracle (P3, replaces the MVP's governance-set reference rate)

- Interface: the MVP reserves the `PriceSource` trait.
- Implementation: multiple independent (staked) reporters submit prices → on-chain median → TWAP → deviation circuit breaker (if the deviation from the previous TWAP exceeds X%, pause and fall back to the governance price).
- Sources: on-chain DEX (once batch-auction prices from private swaps in the shielded pool are available) + external reporters.

### 8.2 Ethereum bridge (P3)

- Ethereum → AgentCoin: run an Ethereum light client on AgentCoin (verifying sync-committee signatures, or STARK proofs of Ethereum state).
- AgentCoin → Ethereum: deploy a contract on Ethereum that verifies AgentCoin finality proofs. Verifying an aggregated STARK over PQ signatures on Ethereum is costly, so this direction **opens only after STARK aggregation (§2.2) is live**.
- Bridged assets are uniformly tagged `NonPqAsset`; wallets warn that "this asset's security depends on Ethereum and is not currently post-quantum".

### 8.3 Optional stablecoin payments (P3, D23)

- Bridged stablecoins can mint inference vouchers; at settlement the protocol converts them to ATC through private swaps in the shielded pool, so **providers always receive ATC** and the emission and burning logic stays unchanged.
- Stated risk: stablecoin issuers can freeze addresses on Ethereum; nothing can be frozen on AgentCoin itself, but the bridge may be affected.

---

## 9. JAM path (evaluated in P4)

| Convention followed since the MVP | Why it matters |
|---|---|
| All AI work is "off-chain refine → work report → on-chain accumulate" | Isomorphic to JAM's work package / work report model |
| Audits are ELVES-style (random, covert, escalating) | Consistent with JAM's security model |
| Execution logic separated from state transition (accumulate functions in pallets are pure) | Eases porting to PVM services |

**Evaluation conditions**: JAM is in production (Polkadot plans 2027) and stable for ≥1 year; JAM solves its PQ issue (SAFROLE depends on Bandersnatch Ring-VRF); migration does not violate constitution layer 1.
**Options**: (a) remain a standalone L1; (b) turn the AgentCoin chain into an independent instance of the JAM protocol (own validators); (c) become a service on Polkadot JAM (loses sovereignty and PQ autonomy; ruled out in principle).

---

## 10. Full-version token economics additions

| Item | Full-version change |
|---|---|
| Market-work emission (50%) | Split into four sub-pools — inference / training / storage / audits — with ratios set by governance within guardrails |
| Public-work emission (20%) | Mainly funds DAO training proposals (pre-training, RL), then evaluations and datasets |
| Validator emission (10%) | Distributed by weight after work-weighted election |
| Trainer royalties | 5%, from inference fees, not from emission |
| Storage fees | Paid directly by the payer; lost data is slashed |

---

## 11. Interfaces the MVP must reserve (full-version dependencies)

| Interface / structure | MVP module | Full-version use |
|---|---|---|
| `SigAlg::{SlhDsa, FnDsa512, XmssLean}` | `ac-crypto` | Fallback signatures; consensus signature aggregation |
| `rotate_key` / `rotate_session_key` | `pallet-pq-accounts` / `validator-set` | Algorithm migration with zero hard forks |
| `stark_verify` precompile (address reserved) | `pallet-revive` integration | L2 private contracts; bridges |
| `ValidatorElection(stake, workscore)` | `pallet-validator-set` | Work-weighted election |
| `Credit` trait | `pallet-credits` | Stablecoin vouchers, session-key vouchers |
| `ShieldedAction::{Vote, Bid, Swap}` | `pallet-shielded` | L1 private features |
| `Provider.attestation`, `Tier::T0` | `pallet-providers` | TEE confidential inference |
| `Model.lineage`, `Model.royalty` | `pallet-model-registry` | Royalties and lineage incentives |
| `PriceSource` trait | `pallet-ref-rate` | Oracle |
| `JobKind` enum (`Eval`, `DataClean`, `Embed`; reserved `Rl`, `Finetune`, `Pretrain`, `Storage`) | `pallet-public-jobs` / `pallet-work` | Training and storage jobs |
| Finality-proof version number | `ac-bft` | Consensus-upgrade compatibility |
| Extensible governance track table, composable `Origin` | `pallet-referenda` configuration | Bicameral system |

---

## 12. Full-version milestones (coarse)

| Phase | Milestone | Acceptance criteria |
|---|---|---|
| P2 | Fast/slow dual-path BFT | 500ms blocks; finality P95 ≤1s (100 validators across continents) |
| P2 | T0 confidential inference | SDK verifies attestation end to end; gateways cannot obtain plaintext; revocation takes effect within 1 epoch |
| P2 | RL post-training | An RL run on a 7B–32B model completed on the network with ≥100 T2 nodes, with reproducible results |
| P2 | Storage layer | Hundreds of GB of weights stored with 3× redundancy; ≥99.9% pass rate on random challenges |
| P2 | Private voting + bicameral system | A real runtime upgrade passed by private votes in both houses |
| P3 | Work-weighted election | "Miner-type" validators appear in the validator set; security simulation passes |
| P3 | Decentralized pre-training | A permissionless pre-training run of ≥10B parameters, verifiably reproducible end to end |
| P3 | Private swaps + oracle + bridge | Stablecoin → voucher → inference with no privacy break anywhere |
| P4 | STARK aggregation + 1,000 validators | Finality-proof size independent of the number of validators |
| P4 | Frontier-scale training | A community model reaches the top tier of open models on mainstream benchmarks |
