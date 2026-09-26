> 🌐 **English** | [简体中文](03-tokenomics-consensus-scope.zh-CN.md)

# Round 3: Token Economics, Miners in Consensus, MVP vs. Full Scope

> Status: discussion draft (Round 3). Date: 2026-09.

## Newly confirmed decisions

| # | Decision |
|---|---|
| D11 | Hard cap + demand-driven emission + unemitted amounts roll over |
| D12 | The DAO treasury is funded by a fixed share of emission (no premine) |
| D13 | **100% post-quantum signatures**; give up the MetaMask / secp256k1 ecosystem |
| D14 | Total supply leaning towards Bitcoin's 21 million |

---

## 1. Token parameters

### 1.1 References: Bitcoin and Polkadot (Ref 1710)

| | Bitcoin | Polkadot (Ref 1710, effective 2026-03-14) |
|---|---|---|
| Cap | 21 million | 2.1 billion DOT |
| Curve | Halving every 210,000 blocks (~4 years), stepwise | **Every 2 years, issue 13.14% of the "remaining issuable supply"**, geometric decay approaching the cap |
| First adjustment | — | Annual issuance cut from 120M to ~56.88M (−53.6%); inflation from 7.5% to ~3.3% |
| Half-life | 4 years | ~9.8 years |
| Tied to demand | No | No |

### 1.2 Key finding: "emission proportional to the remainder" gives rollover for free

The essence of Polkadot's model is `max emission this period = r × (cap − minted)`. **If a period emits less because of low demand, the remainder is larger and every future period's allowance grows automatically.** No separate "rollover pool" and no extra state are needed, and the total can never exceed the cap. This is exactly what D11 needs.

### 1.3 Proposed parameters

| Parameter | Proposed value | Notes |
|---|---|---|
| Cap | **21,000,000** | D14 |
| Precision | **18 decimals** | Per-token inference billing needs micropayments; Bitcoin's 8 decimals are not enough; consistent with the EVM ecosystem; `2.1e25` fits in `u128` |
| Maximum emission curve | `E_max(epoch) = r × (Cap − Minted)` | Polkadot-style geometric decay |
| r | **50% per 4 years** (daily r ≈ 0.000474) | Keeps Bitcoin's "halving every 4 years" rhythm; with full demand, 10.5M after 4 years, 15.75M after 8, ~19.69M after 16 |
| Smooth or stepwise | Smooth daily by default; could instead halve stepwise every 4 years to keep the "halving event" narrative | **Your call** |
| Block time | Target **500ms**, 1s for the MVP first | See §2 |
| Finality | Target ≤1s, ≤3s for the MVP | |
| Emission settlement period | 1 hour (epoch) | Work verification lags, so settle in per-epoch batches |

### 1.4 How each period's emission is split

`E_max` is split four ways:

| Share | Ratio (draft) | Demand-gated? | Purpose |
|---|---|---|---|
| Security budget | 10% | **No**, fixed each period | BFT validators; the chain must stay secure even without demand |
| DAO treasury | 10% | No | Foundation, development, audits, cold-start procurement (D12) |
| Market work | Up to 60% | **Yes**: `min(share, k × verified paid work)` | Paid inference, training |
| Public work | Up to 20% | Yes: only for DAO-published jobs | Community-model training, evaluation, data processing |

Unemitted shares stay in the "remaining issuable supply" and roll over automatically (§1.2).

### 1.5 A rigorous look at self-dealing

Let the self-dealer pay `F`, the burn ratio be `b`, the emission multiplier `k`, and the real compute cost of doing the work `C`:

```
Net profit of self-dealing = (1−b)F [recovered as the provider] + kF [emission] − F [paid] − C [real compute]
                           = (k − b)F − C
```

- If the work is **verified and really done**, the self-dealer must actually spend compute `C`. "Self-dealing" is then essentially **Bitcoin mining: trading real compute for tokens**, except the output is useful. **That is not an attack; it is the behavior we want.**
- Only two real threats remain:
  1. **Fabricated work** (`C ≈ 0`) → handled by the verification layer: random audits, TOPLOC, slashing.
  2. **Useless work** (computed, but nobody needs the result) → the "public job queue" is the backstop: idle miners default to DAO-published training and evaluation jobs. **This is our "mining": idle compute advances training of the community model.**
- Parameter constraint: `k − b < C/F` (compute cost as a share of price, typically 0.5–0.8), so even fabricated work that escapes audits earns little. Add payer-concentration decay and the expected loss from audit slashing on top.

---

## 2. Can miners take part in block production and voting?

### 2.1 Three levels

| Level | Description | Reference |
|---|---|---|
| **L0 Separated** | Miners only do work and earn emission; staked validators produce blocks and vote | Bittensor (miners do not produce blocks), Ethereum |
| **L1 Work-weighted election** | Each epoch elects a BFT committee (~100–300 seats) with weight = `stake^α × workscore^β`; miners **must also stake** to be selected | New design |
| **L2 Work as consensus power** | Block-production eligibility weighted directly by verified work | Filecoin EC (election by storage power; safety threshold ~20% adversarial storage) |

### 2.2 Why work alone must not confer consensus power

1. **GPUs are general-purpose hardware without "sunk-cost security"**. Bitcoin's security comes from ASICs that can only mine Bitcoin, so an attack destroys the attacker's own investment. GPUs can be rented from the cloud at any time and withdrawn at any time, so attacks cost almost nothing afterwards (like NiceHash hash-rental attacks).
2. **Compute is gone once used and cannot be proven continuously**. Storage can keep proving "I still hold it" (Filecoin); compute cannot. A work score can only be "flow over a past window" — inherently lagging, and open to concentrated bursts.
3. **Work scores can be partly bought** (§1.5 self-dealing): consensus power would cost roughly fees + compute, and unlike stake it cannot be slashed.
4. **BFT safety needs slashable collateral**. There must be something to take away on misbehavior, and a work score cannot be fined.

**Hence: stake is the security floor; work score is an amplifier of participation.**

### 2.3 Recommended path

- **Phase 1 (MVP): L0**, while letting miners **vote in the work-verification layer**: CPU and GPU nodes can act as ELVES-style auditors voting valid/invalid on work reports; auditing is itself useful work and is rewarded. This is the earliest "voting" miners can take part in.
- **Phase 2: L1**. Committee election adds the work score (e.g. geometric weighting with `α=0.5, β=0.5`), a minimum stake threshold, and a 30-day sliding window for work scores. The interface is reserved in phase 1: the `ValidatorElection` trait takes `(stake, workscore)` from the start.
- **No L2**, for the reasons in §2.2.

---

## 3. Knock-on effects of 100% post-quantum signatures

| Effect | Handling |
|---|---|
| The Ethereum transaction format (RLP + secp256k1) is unusable | Only chain-native transactions; `pallet-revive` calls contracts via Substrate extrinsics |
| Hardhat / Foundry tooling | Build an **eth-RPC adapter** that calls a local PQ signer (Foundry supports external signers); keep the deploy/call experience as close as possible |
| Wallets | Built in-house: CLI wallet (MVP) → browser extension → mobile |
| Addresses | Native 32-byte AccountId = `H(alg_id ‖ pk)`; a 20-byte mapping in the EVM view. **Note**: 20-byte addresses lose security under quantum collision attacks (BHT algorithm ~2^53), so logic relying on address uniqueness such as CREATE2 should use the 32-byte form; the Ethereum community is discussing the same issue |
| `ecrecover` precompile | Kept for application logic (verifying legacy signatures) but **never for account authorization** |
| New precompiles | `ml_dsa_verify`, `slh_dsa_verify`, `falcon_verify` (reserved) for PQ signature checks inside contracts |
| Consensus signatures | ML-DSA-44 (2.4 KB signatures); 100 validators × 2 votes per block × 2 blocks per second ≈ 1 MB/s of gossip, acceptable; STARK aggregation later |
| Leader election | **No VRF** (no standardized PQ VRF exists yet); stake-weighted deterministic rotation (HotStuff / Monad style) |
| On-chain randomness (for audit sampling) | Hash commit–reveal (RANDAO style); later a hash-chain VDF (proven by STARKs, post-quantum); **no BLS threshold randomness** |
| Hashing | All 256-bit outputs (against Grover); BLAKE3 / SHA3 on chain, Poseidon2 inside ZK circuits |
| Encryption | ML-KEM-768 + X25519 hybrid (transport and note encryption) |
| ZK | STARK / FRI family only, **no Groth16 / KZG / BLS** |

**Design principles for crypto agility**: every signature, public key and ciphertext carries an `alg_id`; verification logic lives in the runtime (WASM) and can be replaced with forkless upgrades; performance-sensitive primitives live in node host functions — a new algorithm needs a node upgrade but **no hard fork of chain history**; accounts support a "key-rotation transaction" (the old algorithm's signature authorizes binding a new algorithm's public key).

---

## 4. MVP and full scope

### 4.1 MVP (testnet → Mainnet Beta, one person + AI, 12–18 months)

**The end-to-end story the MVP can deliver**:

> A user buys tokens with a PQ wallet, deposits them into the shielded pool and mints anonymous inference credits. Through an OpenAI-compatible API they anonymously call top open models such as DeepSeek / Qwen and receive streamed output. Agents pay per call with the same credits. GPU providers (data-center and consumer cards) register, stake, take jobs, and earn fees and emission; random auditors spot-check results and cheaters are slashed. Idle GPUs run DAO-published evaluation and data jobs. Developers deploy DApps in Solidity.

| Module | Content | Phase |
|---|---|---|
| M1 Chain core | Polkadot SDK standalone chain; PQ accounts (ML-DSA with alg_id); Aura block production (PQ signed), finality first via PQ-retrofitted GRANDPA or a simplified HotStuff-2; 1s blocks | α |
| M2 Token economics | 21M cap, geometric emission, four-way split, burning, per-epoch settlement | α |
| M3 EVM | `pallet-revive` (REVM) + PQ verification precompiles + eth-RPC adapter + CLI wallet | α |
| M4 Inference market | Provider registration (tier T1/T2, model hash, price, stake), prepaid credits, settlement, slashing | α |
| M5 Inference gateway | OpenAI-compatible API, streaming, routing by latency and price, provider-side vLLM / SGLang adapters | α |
| M6 Verification layer | Random auditor sampling + TOPLOC proofs + dispute votes + slashing | α |
| M7 Shielded pool + anonymous inference vouchers | STARKs (Plonky3 / Stwo style) + Poseidon2 + ML-KEM hybrid; nullifiers against double spends | **β** |
| M8 Public job queue (lite) | DAO-published evaluation and data-processing jobs as the sink for idle compute | β |
| M9 Governance (lite) | sudo on testnet → multisig → token voting before mainnet; **sudo must be removed before mainnet** | β |

**Not in the MVP**: TEE confidential tier, training and RL, storage proofs, private contracts, work-weighted consensus, mixnet, cross-chain bridges, 500ms blocks.

### 4.2 Full scope (by phase)

| Phase | New capabilities |
|---|---|
| **P2 Service expansion** | T0 TEE confidential inference (GPU CC remote attestation + on-chain revocation list); **RL post-training** (rollouts on T2 consumer GPUs); LoRA / SFT fine-tuning jobs; T4 storage layer (distribution and storage proofs for weights, checkpoints, datasets); 500ms blocks + sub-second finality (fast BFT + STARK signature aggregation); full OpenGov-style governance; browser-extension and mobile wallets |
| **P3 Training and consensus** | Decentralized pre-training (DiLoCo / DisTrO-style low-communication training + Verde-style reproducible verification); community-model registration and revenue sharing; **L1 work-weighted validator election**; mixnet (IP privacy); private-contract research (Noir or private PVM execution); PQ-safe cross-chain bridges |
| **P4 Frontier** | Large-scale decentralized MoE training; migrate to a JAM-style refine/accumulate execution model or become a JAM service; catch up with closed models |

## Sources

- Polkadot Ref 1710: https://polkadot.polkassembly.io/referenda/1710 · https://bex.co/blog/2026/03/14/polkadot-pi-day-hard-cap-2-1b-dot-supply-cap-tokenomics-revolution · https://bitcoinethereumnews.com/tech/polkadot-adopts-2-1b-cap-issuance-cuts-begin-mar-2026/ · https://forum.polkadot.network/t/wfc-completing-the-1710-monetary-reform/18448
- Bittensor roles: https://docs.learnbittensor.org/learn/anatomy-of-incentive-mechanism · https://arxiv.org/pdf/2507.02951
- Filecoin EC security: https://arxiv.org/abs/2308.06955 · https://spec.filecoin.io/systems/filecoin_blockchain/storage_power_consensus/
