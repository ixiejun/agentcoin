> 🌐 **English** | [简体中文](decisions.zh-CN.md)

# AgentCoin Decision Log (D1–D37)

> This file consolidates the final conclusions of the requirements discussion. The discussion itself is in `docs/research/01–07`; the technical plans are in `docs/design/`.

| # | Area | Decision | Source |
|---|---|---|---|
| D1 | Vision | Build for the future: the protocol architecturally supports the full pipeline from pre-training to inference and keeps evolving | R2 |
| D2 | Incentives | Rewards attach only to work that the network actually used, that someone paid for, and that passed verification | R2 |
| D3 | Resources | Permissionless onboarding + tiers (T0 TEE / T1 data center / T2 consumer / T3 CPU / T4 storage) | R2 |
| D4 | Cryptography | Post-quantum from day one; the first implementation may be imperfect but must be pluggable and smoothly upgradable | R2 |
| D5 | Roadmap | Serve the best open-weight models first, then progressively train our own, aiming to match closed models | R2 |
| D6 | Neutrality | Protocol neutrality; content policy is decided by each service provider | R2 |
| D7 | Architecture | Own L1 built on the Polkadot SDK (Substrate); the AI work layer borrows JAM's refine/accumulate + ELVES pattern | R2/R3 |
| D8 | Use case | Year one: anyone calls top open-source model APIs anonymously from a wallet; on-chain-payable inference for agents | R2 |
| D9 | Token | No premine; Bitcoin-style fixed cap | R2 |
| D10 | Team | One person + AI | R2 |
| D11 | Emission | Hard cap + demand-driven emission + unemitted amounts roll over | R3 |
| D12 | Treasury | The DAO treasury is funded from emission | R3 |
| D13 | Signatures | 100% post-quantum signatures; give up the MetaMask / secp256k1 ecosystem | R3 |
| D14 | Token | Total supply 21,000,000 ATC, 18 decimals | R3 |
| D15 | Emission | Stepwise halving every 4 years (scheduled emission + rollover reserve; reserve drawdown capped at 1× the scheduled amount) | R4 |
| D16 | Allocation | Validators 10% (of the scheduled amount, unconditional) / market work 50% / public work 20% / treasury 20% | R4 |
| D17 | Brand | Project AgentCoin, token ATC | R4 |
| D18 | Treasury | Treasury = max(5% of scheduled amount as a floor, the 20% share proportional to actual work emission) (take the larger, do not add) (the floor vests linearly over 2 years and may only fund audits and cold start); two accounts: community grants + holder treasury | R5 |
| D19 | Genesis | PoA validators receive no security budget (it rolls over); switch to PoS automatically once stake ≥ 10% of circulating supply and validators ≥ N | R5 |
| D20 | MVP | Unified `Credit` interface; α ships non-ZK privacy measures; testnet uses only transparent credits; mainnet waits until anonymous vouchers pass an audit | R5 |
| D21 | Deliverables | Produce two technical plans: MVP and full version | R5 |
| D22 | Models | Community-model weights are fully open; about 5% of inference fees on this network go to training contributors | R6 |
| D23 | Pricing | Priced in USD, settled in ATC | R6 |
| D24 | Constitution | 21 million cap; no content censorship or geo-blocking at the protocol layer; no premine; PoA → PoS switch conditions | R6 |
| D25 | Governance | Bicameral: token house + contributor house (votes weighted by work score) | R6 |
| D26 | Storage | Dedicated storage layer (weights, checkpoints, datasets) | R6 |
| D27 | Privacy | MVP: L0 (private transfers, anonymous vouchers); full: L1 (private voting, sealed bids, private swaps); L2 interfaces reserved only; no L3 | R7 |
| D28 | Royalties | Incentives instead of enforcement (lineage declaration + routing priority) | R7 |
| D29 | Exchange rate | During the transition, governance sets a reference rate (with a cap on each adjustment) | R7 |
| D30 | Constitution | Three layers of protection: node-enforced invariants / constitution + guardrails / track-based day-to-day governance | R8 |
| D31 | Engineering | Primary development language is **Rust**; Python only for thin plugins inside inference/training engines (the engines are third-party) | R9 |
| D32 | Engineering | Spec-driven development (SDD) with OpenSpec (`openspec/`); specs first, then code | R10 |
| D33 | Documentation | Project docs are bilingual with English first (English at the canonical path, Chinese in `*.zh-CN.md`, linked at the top); OpenSpec artifacts are Chinese only; code comments and commit messages are English | R11 |
| D34 | Cryptography | AlgId is 1 byte per category and is also the SCALE enum index of the tagged types, so canonical encoding = on-chain SCALE encoding = what TypeInfo describes (one byte form, one numbering); `0x00` never allocated, `0xFF` reserved as extension marker | M0 apply |
| D35 | Cryptography | On-chain hashing is BLAKE3-256: block hashes, the extrinsics root and the state trie all use it (`Hashing = Blake3Hasher`). Documented exception: storage-key hashers of SDK pallets (`Blake2_128Concat` / `Twox64Concat` / `Twox128`) only spread keys and commit to nothing; AgentCoin's own pallets key by account ID with `Identity`. Fixed at genesis | M1 apply |
| D36 | Signatures | Transactions are v5 General transactions authorized by the `PqAuthorize` extension (first in the pipeline): ML-DSA over a BLAKE3 `derive_key` payload with context `agentcoin/tx/v1`; the first transaction registers the public key, later ones look it up; the legacy `Signed` form is closed (`NoClassicSignature`). Replaces "`AcMultiSignature` implements `Verify`" in the MVP plan §3.3 | M1 apply |
| D37 | Licensing | Source stays MIT. GPL-3.0 with Classpath exception is allowed only in the dependency closure of `node/` (and `tests/e2e`, which runs the node); `crates/`, `pallets/`, `runtime/` and `clients/` stay GPL-free, enforced in CI. `ac-node` binary releases ship a GPL source offer | M1 apply |
