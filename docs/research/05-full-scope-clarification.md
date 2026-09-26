> 🌐 **English** | [简体中文](05-full-scope-clarification.zh-CN.md)

# Round 5: Clarifying the Full Version

> Status: discussion draft (Round 5). Date: 2026-09.
> Goal: settle the key design choices for every subsystem of the full version, then produce two technical plans (MVP + full).
> Each question lists options and a **recommendation**; if you agree with the recommendation, just reply "agree".

## Newly confirmed decisions

| # | Decision |
|---|---|
| D18 | Treasury = 20% of actual emission + a 5% floor (the floor vests linearly over 2 years and may only fund audits and cold start); split into "community grants" and "holder treasury" accounts; the founder is funded through public grant applications |
| D19 | During genesis, PoA validators **receive no security budget** (it rolls over); once stake ≥ 10% of circulating supply and validators ≥ N, **on-chain code switches to PoS automatically**, and nobody can delay it |
| D20 | The MVP takes the compromise: a unified `Credit` interface; α implements non-ZK privacy measures; the testnet uses only transparent credits; **mainnet launch waits until anonymous vouchers pass an external audit** |
| D21 | Produce two technical plans: MVP and full version |

---

## A. Consensus layer (full)

**A1. Fast BFT protocol**
- Options: (a) HotStuff-2 / Jolteon (two voting rounds, the Aptos and Monad lineage); (b) Alpenglow style (Votor: a one-round fast path ~100–150ms with a two-round slow-path fallback); (c) keep GRANDPA
- **Recommendation**: a simplified (a) for the MVP; evolve to a (b)-style "fast/slow dual path" in the full version, targeting 500ms blocks and ~1s finality.

**A2. Validator count**
- Options: 100 / 300 / 1000
- **Recommendation**: 50–100 in the MVP; 300 in the full version. PQ signatures are large; 1,000 validators is realistic only once STARK aggregation matures.

**A3. Post-quantum path for consensus signatures**
- Options: (a) ML-DSA broadcast individually (O(n) bandwidth); (b) hash-based signatures (XMSS family) + STARK aggregation, in line with Ethereum's leanSig / leanMultisig; (c) ML-DSA + STARK folding
- **Recommendation**: (a) for the MVP; (b) for the full version. (b) relies only on hashes, the most conservative security assumption; in 2026 binary-field proof systems (e.g. Flock) reach ~660k BLAKE3 compressions/s, making aggregation practical. The signature scheme stays pluggable via `alg_id`.

**A4. Parameters for miners in consensus (L1 work-weighted election)**
- Weight = `stake^α × workscore^β`, work score over a 30-day sliding window, with a minimum stake threshold.
- **Recommendation**: initially α = β = 0.5; work score capped at 5% of the network total to stop any single large farm from dominating. Parameters adjustable by governance within step limits.

**A5. Slashing severity**
- **Recommendation**: 100% for double-signing or equivocation; downtime only forfeits emission, not principal; work-layer cheating is slashed as **a percentage of the tier's stake** (see B3).

---

## B. AI service layer (full)

**B1. Pricing mechanism**
- Options: (a) providers post prices + gateway routing; (b) order book; (c) per-request real-time auction
- **Recommendation**: (a). Lowest latency, same model as OpenRouter. Prices are **denominated in USD and settled in ATC** (see G2).

**B2. Service quality (SLA) and reputation**
- Recorded on chain per provider: TTFT P50/P95, throughput, error rate, audit pass rate. Gateways route on these metrics.
- **Recommendation**: reputation data is measured by auditors through sampling and put on chain as signed measurement reports; providers' self-reported numbers are not trusted.

**B3. Stake thresholds per tier**

| Tier | Minimum stake (draft, USD equivalent) | Rationale |
|---|---|---|
| T0 TEE | High (e.g. $10k equivalent) | Handles private data; TEEs can be broken physically |
| T1 data center | Medium | Large-model inference |
| T2 consumer | Low | Lower barrier to entry |
| T3 CPU / auditor | Low | |
| T4 storage | By capacity | |

- Issue: at cold start miners have no ATC, so how can they stake? **Recommendation**: new miners can join the **"public job queue" with zero stake**, mining without taking paid jobs; once they have accumulated ATC they upgrade to a paid tier. This is also part of the fair launch.

**B4. Model registry**
- Each model records: weight hashes (per shard), architecture, quantization, license tag, provenance.
- **Recommendation**: the protocol only checks that "the model hash you declared = the model you actually run" (via TOPLOC or TEE attestation) and **does not police licenses** (D6, protocol neutrality); the license tag is informational only.

**B5. Attestation verification for the T0 TEE tier**
- Options: (a) verify NVIDIA and Intel/AMD certificate chains directly on chain (ECDSA/RSA — not post-quantum, and the chains are large); (b) auditors verify off-chain and put signed results on chain; (c) a STARK proving "I verified this attestation report"
- **Recommendation**: start with (b), evolve to (c) in the full version. The chain maintains a **trusted-measurement allow-list** and a **revocation list**, updated through a governance fast track.

---

## C. Training (full)

**C1. RL post-training architecture**
- Following PRIME-RL / INTELLECT-2:
  - T2 consumer GPUs generate rollouts;
  - TOPLOC verifies that rollouts were really generated by the declared policy model;
  - T1 trainer nodes perform policy updates;
  - new weights are broadcast through the T4 storage layer (SHARDCAST style).
- 2026 work such as ECHO-2 shows that large-scale distributed rollouts can cut costs substantially.
- **Recommendation**: this is the first training capability to ship in the full version (P2).

**C2. Decentralized pre-training**
- Algorithm: DiLoCo / SparseLoCo (8- or 4-bit quantization + sparsification, **100–2000× less communication than naive data parallelism**); Templar has already trained a 72B model this way.
- Verification: Verde-style reproducible operators + dispute arbitration, plus random spot checks of gradient contributions.
- **Recommendation**: ship in P3; "training proposals" are initiated by the DAO and funded by public-work emission (20%) plus community crowdfunding.

**C3. Ownership of community-trained models (key question)**
- Options:
  - (a) fully open (Apache 2.0 / MIT), usable by anyone including closed-source companies;
  - (b) open weights, but **inference on this network automatically pays royalties** to training contributors;
  - (c) a network-proprietary license: runs only on this network
- **Recommendation**: (b). Open weights fit the mission of universal access; each paid inference of the model on this network automatically diverts a small share (e.g. 5%) to trainers in proportion to their proven work, creating a "training → model → revenue → more training" flywheel. Use outside the network is free and cannot be charged anyway.

**C4. Training data**
- The protocol is neutral and does not police data; datasets register hashes and provenance and are stored on T4.
- **Recommendation**: DAO-funded training requires **public dataset provenance** (transparent data recipes) for reproducibility and auditing.

---

## D. Storage layer (full)

**D1. Scope**
- Options: (a) **dedicated**: only model weights, checkpoints, datasets and proof data; (b) **general-purpose**: a Filecoin / Arweave-style storage market
- **Recommendation**: (a). General-purpose storage is a huge market of its own and would dilute focus; a dedicated layer can be optimized for large files and high-bandwidth distribution (weight broadcast).

**D2. Storage proofs**
- Options: (a) Filecoin-style PoRep / PoSt (expensive sealing, complex implementation); (b) erasure coding + randomly challenged proofs of retrievability (PoR) + sampled download tests
- **Recommendation**: (b) — simple, practical, hash-only and post-quantum.

---

## E. Privacy (full)

**E1. Private smart contracts**
- Options:
  - (a) Aztec-style dual state (private functions executed and proven on the client; needs a dedicated language such as Noir);
  - (b) a TEE confidential EVM (Oasis Sapphire style; good UX, but trust assumptions, and TEEs can be broken physically);
  - (c) an FHE coprocessor (Zama style; poor performance; most schemes are lattice-based and can be post-quantum);
  - (d) not for now
- **Recommendation**: in P3, start with a slim version of (a): only fixed circuits for "private transfers + private vouchers + selective disclosure", **no general private contracts**. Wait 1–2 years on general private contracts.

**E2. Network-layer privacy (mixnets)**
- Mixnets add hundreds of milliseconds to seconds of latency, conflicting with streaming inference.
- Options: (a) integrate an existing mixnet (e.g. Nym); (b) build lightweight one- or two-hop relays; (c) use the mixnet only for low-frequency operations such as "submit transaction / redeem voucher"
- **Recommendation**: (c) + (b). Low-frequency on-chain operations go through the mixnet; inference traffic takes optional 1–2 hop relays (like Oblivious HTTP: the relay knows who you are but cannot see content; the gateway sees content but not who you are).

**E3. Selective disclosure**
- **Recommendation**: support view keys so users can voluntarily disclose their transaction history to auditors or exchanges. The protocol never compels it.

---

## F. Governance (full)

**F1. Runtime upgrade authority**
- A runtime upgrade is a "god power" that can rewrite every rule — the most sensitive point in the whole governance design.
- **Recommendation**:
  - upgrades must pass a token vote;
  - after passing, a **mandatory delay** (e.g. 28 days) gives dissenters time to exit;
  - **constitutional immutable clauses** with extremely high amendment thresholds (e.g. 75% turnout plus an absolute majority) or no amendment at all:
    1. total supply of 21 million;
    2. protocol neutrality (no content censorship or geo-blocking at the protocol layer);
    3. the no-premine principle (no issuance may bypass the emission rules);
    4. the conditions for switching from PoA to PoS.

**F2. Voting-power structure**
- Options: (a) pure token voting (one coin, one vote); (b) tokens + lock-time weighting (Polkadot conviction voting); (c) bicameral: a token house + a "contributor house" (weighted by work score, so miners also vote)
- **Recommendation**: start with (b), evolve to (c) in the full version. A bicameral system prevents pure capital capture and answers the call for "miners in governance".

**F3. Emergency channel**
- E.g. fast revocation after a TEE is broken, or pausing a module after a critical bug.
- **Recommendation**: an elected **security council** (e.g. a 5-of-7 multisig) that **can only pause and revoke, never change rules or move funds**; every pause must be ratified by a full vote within N days or it lapses automatically.

---

## G. Interoperability and stable pricing (full)

**G1. Cross-chain bridges**
- Difficulty: counterpart chains (Ethereum, Bitcoin) are not post-quantum today, so a bridge is only as safe as its weakest end.
- Options: (a) no bridge; (b) a light-client + STARK-proof bridge, clearly labelling "the counterpart chain's security"; (c) a multisig bridge (not recommended)
- **Recommendation**: build (b) in P3, connecting only to Ethereum (to bring in stablecoins and liquidity); bridged assets are **labelled "non-PQ assets"** on chain.

**G2. Unit of account for inference (key question)**
- Users and agents need **stable prices** ("$0.50 per million tokens"), but the ATC price fluctuates.
- Options: (a) price purely in ATC; (b) **price in USD, settle in ATC** (converted via an oracle); (c) accept bridged stablecoins (USDC etc.) directly; (d) a native decentralized stablecoin
- **Recommendation**: (b) for the MVP, add (c) in the full version. Caveat for (c): the USDC issuer can freeze addresses, which conflicts with censorship resistance, so it can only be an option, never the only way. **The oracle itself is a new attack surface** and needs a multi-source median plus TWAP.

---

## H. Agent economy (full)

**H1. Agent accounts**
- **Recommendation**: "session keys" built on PQ account abstraction: the master key authorizes a sub-key for the agent with a spending limit, expiry and an allow-list of callable contracts or models, revocable at any time.

**H2. Agent payment protocol**
- The industry already has HTTP 402-style machine-payment standards (e.g. x402).
- **Recommendation**: gateways support HTTP 402-style "pay per request" interactions backed by anonymous vouchers. Also provide an MCP server so agent frameworks can call "paid inference" directly as a tool.

**H3. Agent identity and reputation**
- **Recommendation**: optional on-chain agent identity (public key + metadata); reputation is built by services outside the protocol; the protocol mandates no KYC.

---

## I. Long term: JAM migration

- **Recommendation**: from the MVP onward, write the AI work layer in the refine (off-chain heavy compute producing reports) / accumulate (on-chain settlement) structure, with ELVES-style "random, covert, escalating" audits. After 2027, if JAM has matured and solved its PQ issue (SAFROLE depends on elliptic curves), evaluate migrating or running AgentCoin as a JAM service.

---

## Key questions for you to answer

1. **C3 ownership of community models**: (a) fully open, (b) open + royalties on in-network inference, or (c) network-proprietary?
2. **G2 unit of account for inference**: do you accept USD pricing with ATC settlement? Are bridged stablecoins allowed as an optional payment method?
3. **F1 constitutional immutable clauses**: are the four above right? Anything to add?
4. **F2 bicameral system**: should miners eventually have governance votes (the contributor house)?
5. **D1 storage layer**: dedicated or general-purpose?
6. **E1 private contracts**: do you accept that the full version only ships fixed-circuit private features, without general private contracts?

Everything else follows the recommendations unless you object.

## Sources

- INTELLECT-2 / PRIME-RL: https://arxiv.org/pdf/2505.07291 · https://www.primeintellect.ai/blog/intellect-2
- ECHO-2: https://arxiv.org/pdf/2602.02192
- Sparsity of RL weight updates: https://arxiv.org/pdf/2602.03839
- INTELLECT-1 / DiLoCo: https://arxiv.org/pdf/2412.01152
- Epoch AI: https://epoch.ai/gradient-updates/how-far-can-decentralized-training-over-the-internet-scale
- PQ consensus aggregation: https://hackmd.io/@goatresearch/H1G2tOCwGx · https://eips.ethereum.org/EIPS/eip-8288 · https://cic.iacr.org/p/2/1/13
- PQ mixnets: https://arxiv.org/pdf/2501.02933
