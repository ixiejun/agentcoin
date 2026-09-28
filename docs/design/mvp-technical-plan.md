> 🌐 **English** | [简体中文](mvp-technical-plan.zh-CN.md)

# AgentCoin MVP Technical Plan v0.1

> Status: first draft, under review. Date: 2026-09.
> Basis: `docs/decisions.md` (D1–D37). For the full version see `full-technical-plan.md`.
> Convention: "**[reserved]**" means not implemented in the MVP, but the interface or data structure must already be defined in the MVP for the full version to use.

---

## 0. Goals and scope

### 0.1 The end-to-end story the MVP must deliver

> A user holds ATC in a PQ wallet, deposits it into the shielded pool and mints anonymous inference credits. Through an OpenAI-compatible API they anonymously call open models such as DeepSeek / Qwen and receive streamed output. Agents pay per call with the same credits. Data-center and consumer GPU providers register permissionlessly, stake, take jobs, and earn fees plus emission. Auditors spot-check service quality as "mystery shoppers", and cheaters are slashed. Idle compute runs DAO public jobs. Developers deploy DApps in Solidity. Every signature and every encryption on the chain is post-quantum.

### 0.2 Phases

| Version | Network | Content |
|---|---|---|
| **α** | Testnet | Chain core, PQ accounts, token economics, EVM, inference market (transparent credits), gateway, audits, public jobs (lite), PoA |
| **β** | Testnet → audit | Shielded pool + anonymous inference vouchers, token-vote governance, PoA → PoS switch logic, guardrails |
| **Mainnet Beta** | Mainnet | Launches after β passes an external audit; **anonymous vouchers are the only payment path on mainnet** (D20) |

### 0.3 Out of MVP scope (see the full version)

TEE confidential tier, training and RL, storage layer, L1 private features (voting, bidding, swaps), work-weighted election, bicameral governance, mixnet, cross-chain bridges, stablecoin payments, 500ms blocks, STARK signature aggregation.

### 0.4 Success criteria (Mainnet Beta)

| Metric | Target |
|---|---|
| Block time / finality | 1s / ≤3s (P95) |
| Extra time-to-first-token overhead (vs. calling the provider directly) | ≤150ms (P50) |
| Inference throughput | On par with calling the provider directly (the chain is not on the data path) |
| Audits | Every provider is mystery-shopped at least once per hour; detection rate ≥99% for model swapping / precision downgrades |
| Security | Zero sudo; all node invariants enabled; anonymous-voucher circuits pass an external audit |

---

## 1. Overall architecture

```
┌──────────────────────────────────────────────────────────────────────────┐
│ Client layer                                                               │
│  ac-wallet (CLI) · ac-sdk (Rust core + bindings) · OpenAI clients / agents │
└───────────────┬─────────────────────────────────────┬────────────────────┘
                │ on-chain tx (PQ signed)              │ HTTPS inference request (+ voucher)
                ▼                                     ▼
┌───────────────────────────────┐     ┌──────────────────────────────────────┐
│ AgentCoin node (ac-node)       │     │ Inference gateway (ac-gateway),       │
│  ├ Consensus: Aura-PQ + AC-BFT │◄────┤  permissionless, staked               │
│  ├ Node invariant checker      │settle│  OpenAI API · streaming · routing    │
│  │   (constitution layer 1)    │     │  voucher checks · no logs · batching │
│  ├ Runtime (WASM)              │     └───────────────┬──────────────────────┘
│  │  ├ system / PQ accounts / fee burn                │ forward (provider cannot see payer)
│  │  ├ emission / treasury / PoA→PoS                  ▼
│  │  ├ model registry / providers │     ┌──────────────────────────────────────┐
│  │  ├ Credit (transparent|shielded)│   │ Provider agent (ac-provider)          │
│  │  ├ work settlement / audit / slash│◄┤  wraps vLLM / SGLang · TOPLOC proofs │
│  │  ├ public jobs / reference rate │reg│  signed receipts · heartbeat         │
│  │  ├ governance / guardrails  │     └──────────────────────────────────────┘
│  │  └ pallet-revive (EVM) + PQ precompiles           ▲
│  └ eth-RPC adapter             │     ┌────────────────┴─────────────────────┐
└───────────────────────────────┘     │ Auditor agent (ac-auditor)             │
                ▲                     │  mystery shopping · TOPLOC re-check ·  │
                └─────────────────────┤  verdicts on chain                     │
                                      └──────────────────────────────────────┘
```

**Core principle**: the chain is **not** on the inference data path. Inference flows over HTTP streams; the chain only handles registration, credits, settlement, audits and slashing. This mirrors JAM's refine (off-chain) / accumulate (on-chain) pattern.

---

## 2. Technology stack

| Layer | Choice | Notes |
|---|---|---|
| Language | **Rust is the primary language** (D31): chain, runtime, consensus, cryptography, circuits, gateway, provider agent, auditor agent, wallet and SDK core are all Rust | Sole exception: a **thin Python plugin** inside vLLM / SGLang that extracts TOPLOC activations (about 200 lines, see §2.1) |
| Chain framework | **Polkadot SDK** (latest stable release), solochain template | Forkless runtime upgrades |
| EVM | `pallet-revive` (REVM backend) | Compatible with Solidity tooling |
| PQ signatures | ML-DSA-44 / ML-DSA-65 (FIPS 204); SLH-DSA (FIPS 205) and FN-DSA (Falcon) reserved | RustCrypto `ml-dsa` (chosen in M0: pure Rust, no_std); validated against NIST ACVP vectors |
| PQ encryption | ML-KEM-768 + X25519 hybrid (X-Wing combiner) | P2P transport, end-to-end encryption beyond gateway TLS, shielded-pool note encryption |
| Hashing | BLAKE3 (general on-chain), SHA3-256 (interop), Poseidon2 (inside circuits) | All outputs 256 bits |
| ZK (β) | FRI-based STARKs (candidates: Plonky3 / Stwo / Winterfell; benchmark-based selection in month 7) | Groth16 / KZG / BN254 / BLS12 **forbidden** |
| P2P | libp2p (bundled with the Polkadot SDK) + PQ hybrid handshake | See §3.6 |
| Inference engines | vLLM / SGLang (provider's choice) | |
| Verifiable inference | TOPLOC | |
| Gateway | Rust (axum + tokio), SSE streaming | |

### 2.1 Rust-first principle and its boundaries (D31)

| Component | Language | Notes |
|---|---|---|
| Node, runtime, pallets, consensus, invariant checker | Rust | Native to the Polkadot SDK |
| `ac-crypto`, STARK circuits (Plonky3 / Stwo / Winterfell are all Rust) | Rust | The same code compiles to native, the WASM runtime and client-side WASM |
| Gateway, provider agent, auditor agent, eth-RPC adapter | Rust (tokio + axum) | |
| Wallet, SDK core | Rust | Python / TS are only bindings |
| TOPLOC | Rust port (`ac-toploc`): proof encoding, validation, comparison | The auditors' re-check logic is pure Rust |
| Inference engines | Not built in-house; call the OpenAI-compatible APIs of vLLM / SGLang | The engines are third-party Python programs |
| **TOPLOC activation extraction** | **Thin Python plugin** inside the inference engine that only hands hidden-layer activations to the Rust agent (over a local socket) | TOPLOC must read the model's internal activations, which is only possible inside the engine process; the plugin stays tiny and contains no business logic |
| **[optional] Rust-native inference backend** | mistral.rs / candle | T2 consumer nodes may choose a pure-Rust path with no Python plugin; where it underperforms vLLM, vLLM remains the recommendation |

Engineering conventions: stable Rust toolchain (the version the Polkadot SDK requires); `#![forbid(unsafe_code)]` by default (crypto libraries and FFI are exceptions, each with a stated reason); `cargo fmt` + `clippy -D warnings` + `cargo deny` (licenses and advisories) + `cargo audit` in CI; runtime and pallets stay `no_std` compatible.

---

## 3. Cryptographic abstraction layer (`ac-crypto`) — the pluggable core

### 3.1 Design principles

1. **Every** signature, public key, ciphertext and proof carries an `AlgId` prefix; there is no "implicit algorithm".
2. Verification logic lives in the runtime (forklessly upgradable); performance-sensitive primitives are exposed via host functions (a new algorithm needs a node upgrade, but not a hard fork).
3. Accounts are decoupled from algorithms: the account ID does not change when keys rotate.
4. Adding an algorithm = one new `AlgId` variant + one implementation + one runtime upgrade, **without changing any existing data**.

### 3.2 Data structures

```rust
// Each category has its own 1-byte AlgId space; the AlgId is also the SCALE enum index (D34).
// 0x00 is never allocated; 0xFF is reserved in every category as an extension marker.
#[repr(u8)]
pub enum SigAlg {
    MlDsa44   = 0x01,  // MVP default (user accounts)
    MlDsa65   = 0x02,  // MVP (validators / high-value accounts)
    MlDsa87   = 0x03,
    SlhDsaSha2_128s = 0x10,  // [reserved] hash-based fallback if lattices are broken
    FnDsa512  = 0x20,        // [reserved] Falcon, smaller signatures
    XmssLean  = 0x30,        // [reserved] consensus signatures + STARK aggregation (full version)
}

#[repr(u8)]
pub enum KemAlg { XWing /* ML-KEM-768 + X25519, draft-06 */ = 0x01, MlKem1024 = 0x02 }

// Tagged types are enums: variant index = AlgId, payload = fixed-length bytes.
// Canonical encoding = SCALE encoding = on-chain encoding = what TypeInfo describes.
pub enum PqPublicKey { #[codec(index = 0x01)] MlDsa44(Box<[u8; 1312]>), /* 0x02, 0x03 … */ }
pub enum PqSignature { #[codec(index = 0x01)] MlDsa44(Box<[u8; 2420]>), /* 0x02, 0x03 … */ }

/// Account ID: 32 bytes, algorithm-independent, unchanged by key rotation
/// At creation: AccountId = BLAKE3-derive_key("agentcoin 2026-09 account-id v1", alg_id ‖ pk)
pub struct AccountId([u8; 32]);
```

### 3.3 Transaction signing

- **v5 General transactions + the `PqAuthorize` transaction extension** (D36). The extension heads the extension pipeline and verifies an ML-DSA signature (context `agentcoin/tx/v1`) over `derive_key("agentcoin 2026-09 tx-payload v1", SCALE(inherited implication))` — the extension version, the call and every later extension's explicit and implicit data (spec and transaction version, genesis hash, mortality, nonce). The SDK's `Verify` trait only receives an account ID, never chain state, so it cannot look keys up in a registry; the legacy `Signed` extrinsic is therefore closed with an uninhabited signature type (`NoClassicSignature`), and no classic scheme can authorize an account.
- **Public-key registry** (`pallet-pq-accounts`): an ML-DSA public key is 1.3 KB — too big to carry in every transaction. An account's first transaction carries its public key, which must derive the claimed account ID; it is registered when the transaction is applied. Later transactions carry only the `AccountId` and signature, and the chain looks the key up. This saves about 1.3 KB per transaction.
- **Key-rotation transaction**: `rotate_key(new_pk, proof)`, authorized by the current key through `PqAuthorize`; `proof` is the new key's signature (context `agentcoin/key-rotation/v1`) over (genesis hash, account, rotation count, new key). The account ID stays the same, and a key can only belong to one account. Migrating to a new algorithm later uses this transaction.
- **Replay protection**: nonce + genesis hash + runtime version are all part of the signed payload (standard Substrate practice).

### 3.4 EVM integration (D13: 100% PQ)

- The native Ethereum transaction format (RLP + secp256k1) is **disabled**: `eth_transact` and every Ethereum-signed path are refused by the runtime's call filter, and RLP bytes never decode as an AgentCoin transaction.
- **Transaction format**: a contract transaction is an ordinary AgentCoin v5 transaction authorized by `PqAuthorize` (ML-DSA) whose call is `Revive::call` or `Revive::instantiate_with_code` with EVM bytecode (constructor arguments appended to the init code); the `SetEvmPayer` extension makes the signer pay for contract accounts, so deployments never mint. EVM chain ID 4403.
- EVM address = `keccak256(AccountId)[12..]` (`pallet-revive`'s account mapping, automatic); contract logic that needs uniqueness (e.g. CREATE2 collision checks) relies on the 32-byte form.
- **Precompiles** (addresses fixed forever):
  - `0x000000000000000000000000000000000A010000` `pq_verify(uint8 alg, bytes publicKey, bytes message, bytes signature) -> bool`: ML-DSA-44/65/87 under the fixed context `agentcoin/evm-verify/v1`; anything that does not verify returns false;
  - `0x000000000000000000000000000000000A020000` `blake3(bytes) -> bytes32`;
  - `0x000000000000000000000000000000000A030000` `poseidon2`: pending the research gate of m4-evm (task 4.1);
  - **[reserved]** `0x000000000000000000000000000000000A100000` `stark_verify(vk_id, proof, public_inputs)`: reserved for L2 general private contracts (D27); every call reverts for now.
- `pallet-revive`'s built-in Ethereum precompiles stay available to applications but are **not post-quantum**: `ecrecover` (0x01), `bn128` add/mul/pairing (0x06–0x08), `point_eval` (0x0a, KZG) and `p256_verify` (0x100). They are for application compatibility only, never for account authorization, and not recommended.
- **eth-RPC adapter** (`ac-eth-rpc`, [README](../../services/eth-rpc/README.md)): Ethereum JSON-RPC for Foundry and other tools: `web3_clientVersion`, `net_version`, `eth_chainId`, `eth_syncing`, `eth_blockNumber`, `eth_accounts`, `eth_gasPrice`, `eth_maxPriorityFeePerGas`, `eth_feeHistory`, `eth_getBalance`, `eth_getTransactionCount`, `eth_getCode`, `eth_getStorageAt`, `eth_call`, `eth_estimateGas`, `eth_getBlockByNumber`, `eth_getBlockByHash`, `eth_getTransactionByHash`, `eth_getTransactionReceipt`, `eth_getLogs`, `eth_sendRawTransaction`. `safe`/`finalized` are AC-BFT finality. `eth_sendRawTransaction` relays only ML-DSA-signed AgentCoin contract transactions; `ac-wallet evm` is the external signer (deploy, send, raw, broadcast of `forge script` simulations, sign-message for `pq_verify`).

### 3.5 Consensus keys

| Purpose | Algorithm | Notes |
|---|---|---|
| Block authoring (Aura-PQ) | ML-DSA-65 | One signature per block |
| Finality votes (AC-BFT) | ML-DSA-65 | Broadcast individually; 100 validators × 2 rounds × 1 block/s ≈ 660 KB/s |
| Session-key rotation | Per era | Reuses the `pallet-session` flow with a custom PQ key type |

### 3.6 Network layer

- The libp2p Noise handshake adds **ML-KEM-768 + X25519 hybrid** key exchange to defeat "harvest now, decrypt later".
- Node identity keys use ML-DSA-65. This requires changing libp2p's identity layer; **if that is too much work for α, Ed25519 node identities + hybrid KEM encryption are allowed temporarily**. Node identity only affects routing, not assets, so the risk is contained. It must be replaced before β.

### 3.7 On-chain hashing (D35)

- **Every integrity commitment is BLAKE3-256**: block header hashes (the block ID), parent references, the extrinsics root, the state root and state-proof nodes. The chain's `Hashing` type is `ac_primitives::Blake3Hasher`, a thin adapter over `ac-crypto`; trie roots computed inside the runtime use it directly, because the SDK only offers BLAKE2 and Keccak trie roots as host functions.
- **Storage-key hashers are a bounded exception**: SDK pallets (`frame-system`, `pallet-balances`, …) lay out storage keys with 128-bit `Blake2_128Concat` / `Twox` hashers. These only spread keys across the trie and make no integrity claim — the BLAKE3 state root does — so they are kept. AgentCoin's own pallets key account maps with `Identity`: an account ID is already a BLAKE3 output.
- **Fixed at genesis**: the hashing scheme is published through the `ChainProfileApi` runtime API; changing it means a new chain or a hard fork, never an ordinary runtime upgrade.

---

## 4. Consensus

### 4.1 α: Aura-PQ + AC-BFT

- **Block authoring**: a fork of `sc-consensus-aura` with `AuthorityId` replaced by an ML-DSA-65 application key; 1s slots, rotating through the validator list, **no VRF** (there is no standardized PQ VRF yet).
- **Finality: AC-BFT**, an in-house HotStuff-2-style finality gadget running as a node-client component:
  - Validators vote on each block in 2 rounds (prepare and commit); at ≥2/3 weight the gadget calls `client.finalize_block()`;
  - Pipelined: a block's commit round is merged with the next block's prepare round;
  - View change on timeout, with exponential backoff;
  - Vote message = `(round, block_hash, height, validator_id, PqSignature)`; finality proof = a set of ≥2/3 votes, verifiable by light clients;
  - **Why not GRANDPA**: GRANDPA's signature type is hard-wired to Ed25519 in `sp-consensus-grandpa`, so retrofitting it is close to a rewrite, and its finality latency is on the high side.
- **Scale**: ≤100 validators in the MVP.
- **Slashing evidence**: double-signing at one height and double-voting in one round can both be submitted as on-chain evidence and slash 100% of stake.
- **As implemented in M2** (protocol form D38, offence handling D39):
  - Rounds, not heights: the leader of round `r` (member `r mod n`) proposes its best block; members prepare-vote under a locking rule, commit-vote on a prepare certificate (`q = ⌊2W/3⌋ + 1`) and enter the next round at once; a commit certificate finalizes the target and its ancestors. Timeouts start at two slots and back off ×1.5 up to 30 s; nodes that fall behind catch up from the rounds that members worth more than `W − q` have reached.
  - Messages are versioned and signed with ML-DSA-65 under `agentcoin/bft-vote/v1`, bound to the genesis hash and set id, and flooded over `/<genesis>/acbft/1`. Votes are persisted before they are sent.
  - Authority sets change only at epoch boundaries (`ScheduledChange` digest, engine `acbf`); the change block is finalized by the old set. A finality proof (versioned set of commit votes) is stored for every change block and at least every 64 blocks, and verified on import and sync.
  - Offences: seal double signing (two headers in one slot) and vote double signing (two messages of one kind in one round and set) are reported automatically through an unsigned, authorized extrinsic. M2 records each kind at most once per offender and set, removes the offender from the next epoch's set on its first record (never emptying it) and punishes only once — slashing will follow the most severe recorded kind, never adding up; no stake is slashed yet — slashing plugs into a reserved `SlashHandler` with PoS. From M3 PoS, `pallet-staking-pos` slashes the offender's self-stake (vote 100%, seal 10%) and burns it.
  - Measured on one 4-core machine (release build, localhost): see the `m2-finality` design, "测量结果"; P95 finality latency is well under 3 s for 4, 7 and 10 validators.

### 4.2 On-chain randomness (for audit sampling)

- `pallet-randomness-cr`: each epoch validators commit `H(secret)` and reveal `secret` in the next epoch. Randomness = `BLAKE3(all revealed values)`. Validators that do not reveal lose emission.
- Known weakness: the last revealer can withhold and thereby bias the result by 1 bit. Acceptable for audit sampling. The full version adds a hash-based VDF.
- **As implemented in M2**: block authors put `note_randomness` (commit of epoch `e`, reveal of epoch `e − 1`) into their blocks as a mandatory inherent; secrets are derived from the validator seed, genesis and epoch, so they survive restarts. `R(e) = derive_key("agentcoin 2026-09 randomness v1", u64_le(e) ‖ reveals sorted by account ID)` is published at the first block of epoch `e + 2` and is recomputable from the published reveals. Missed reveals are counted per validator (the emission penalty comes with `pallet-emission`). The runtime exposes `RandomnessApi` and FRAME's `Randomness`. From M3 PoS, an epoch with a missed reveal earns its validator no reward points, and three in a row pause it from elections.

### 4.3 Automatic PoA → PoS switch (D19)

```
pallet-validator-set state machine:
  Bootstrap(PoA)  ──[all conditions met, at the next era boundary]──►  PoS
Conditions:
  (a) Σ staked ≥ 10% × circulating supply
  (b) number of candidates meeting the stake threshold ≥ N_min (genesis parameter, suggested 21)
  (c) time since genesis ≥ T_min (prevents being met instantly, suggested 30 days)
During PoA: the validator security budget is not paid out and goes to the rollover reserve;
the PoA validator list comes from genesis config + multisig add/remove
```

- The switch logic is written both in the runtime and in the **node invariant checker** (§6), so no runtime upgrade can bypass it.
- MVP election in the PoS phase: top K by stake (a simple stand-in for NPoS); **[reserved]** the `ValidatorElection` trait takes `(stake, workscore)` as input and is replaced by work-weighted election in the full version.
- **As implemented in M3 (`m3-pos`)**:
  - Conditions (revising D19): all active stake (self-stake plus nominations, excluding unbonding) ≥ 10% of the **total issuance**; at least 21 candidates whose active self-stake is ≥ 0.1% of the issuance and that are not paused; height ≥ 63,115,200 (two years after launch). Every validator-epoch boundary is a checkpoint; `ValidatorSet::QualifiedSince` records the first passing checkpoint of an uninterrupted run, and the boundary at which the run has lasted 604,800 blocks (seven days) switches to PoS for good. The multisig can add and remove PoA authorities until then and loses that power at the switch; all PoA authorities leave. During the seven days every epoch publishes a preview election.
  - Staking (`pallet-staking-pos`): candidates register an ML-DSA-65 validator key with a proof of possession (`agentcoin/validator-pop/v1`) and a commission of 5–100% (increases after 7 days, decreases at the next epoch); nominators name 1–16 candidates with at least 0.001% of the issuance; at most 500 candidates and 750 nominators, full lists evict their smallest entry. Self-stake unbonds in 28 days; nominations through a network-wide queue in 2–28 days (after Polkadot RFC-0097).
  - Election: sequential Phragmén with balancing (`sp-npos-elections`) in the last block of each epoch, `K` = 100 on live chains (a parameter; storage bound 1,000, issue I-004); AC-BFT weight `max(1, backing / 10^12)`. The worst case fits one block (issue I-005).
  - Rewards (R1): the security budget is shared by blocks authored, not by stake; each validator's share pays its commission, the rest goes pro rata to its backing, paid automatically from a reward pot. An epoch with a missed randomness reveal earns no points; three in a row pause the validator.
  - Slashing (U3): only the validator's self-stake, bonded and unbonding — vote double signing 100%, seal double signing 10%, the most severe kind per set, burned through `Emission`. Nominators are never slashed; going offline costs no principal.
  - The node recomputes every checkpoint from well-known keys (§6), so no runtime upgrade can switch early, delay the switch or go back.

---

## 5. Runtime modules (pallets)

| Pallet | Version | Responsibility |
|---|---|---|
| `pallet-pq-accounts` | α | Public-key registration, key rotation, `PqAuthorize` transaction authorization |
| `pallet-emission` | α | Scheduled emission, rollover reserve, four-way split, per-epoch settlement |
| `pallet-treasury-dual` | α | Community grants + holder treasury; linear vesting of the 5% floor |
| ~~`pallet-fee-burn`~~ | α | Merged into `pallet-emission`, which records every burn (m3-economics) |
| `pallet-poa-admin` + `pallet-collective` | α | PoA multisig: ML-DSA members with a threshold act as Root (D41) |
| `pallet-validator-set` | α | Epochs, PoA roster, the one-way PoA → PoS state machine, installing elected sets |
| `pallet-staking-pos` | α | Candidates, nominations, unbonding queue, commission, Phragmén election, rewards by work, slashing (m3-pos) |
| `pallet-randomness-cr` | α | Commit–reveal randomness |
| `pallet-model-registry` | α | Model registration, lineage declarations |
| `pallet-providers` | α | Provider registration, tiers, stake, prices, heartbeats |
| `pallet-gateways` | α | Gateway registration, stake, escrowed credits |
| `pallet-credits` | α / β | `Credit` trait; transparent implementation (α) + shielded implementation (β) |
| `pallet-work` | α | Work reports (batched receipts) → settlement → work scores |
| `pallet-audit` | α | Auditor registration, assignment, verdicts, disputes, slashing |
| `pallet-public-jobs` | α (lite) | DAO public job queue |
| `pallet-ref-rate` | α | ATC / USD reference rate (set by governance, with an adjustment cap) |
| `pallet-guardrails` | β | Parameter bound checks (constitution layer 2) |
| `pallet-constitution` | β | Constitution text hash, amendment process |
| `pallet-referenda` + `pallet-conviction-voting` | β | Token-house governance (built-in Polkadot SDK modules with configured tracks) |
| `pallet-shielded` | β | Shielded pool: note-commitment tree, nullifiers, STARK verification |
| `pallet-revive` | α | EVM |

Details of the key modules follow.

### 5.1 `pallet-emission`

```rust
// Constants (published; also inputs to the node invariants)
const UNITS: u128             = 10u128.pow(18);
const CAP: u128               = 21_000_000 * UNITS;
const FIRST_PERIOD_TOTAL: u128 = 10_500_000 * UNITS;
const BLOCKS_PER_PERIOD: u64  = 126_230_400;        // 4 × 365.25 days of 1 s blocks
// Emission epoch length L: genesis parameter, must divide BLOCKS_PER_PERIOD
EpochLength: u64                                    // 3,600 on live chains; ≤ 20 on development chains

fn scheduled(e: u64) -> u128 {                      // S(e)
    let n = e * L / BLOCKS_PER_PERIOD;              // halvings so far; they fall on epoch boundaries
    (FIRST_PERIOD_TOTAL >> n) * L as u128 / BLOCKS_PER_PERIOD as u128
}

// Storage (TotalBurned is a published well-known key)
Reserve, TotalMinted, TotalBurned, LastSettled

// Epoch e = blocks e·L+1 ..= (e+1)·L, settled in on_initialize of block (e+1)·L+1:
S        = scheduled(e)
avail    = S + min(Reserve, S)
security = S * 10%                                  // PoA: not paid, stays in the reserve (D19)
market   = min(avail * 50%, verified_market_work)
public   = min(avail * 20%, verified_public_work)
prop     = (market + public) * 20 / 70
treasury = max(prop, S * 5%)                        // the larger, never the sum (red line 5)
total    = security + market + public + treasury   // scaled down pro rata (rounding down) if above avail
drawn    = max(0, total - S)
Reserve  = Reserve - drawn + max(0, S - total)
```

- All arithmetic is `u128` with rounding down; remainders stay in the reserve, so `Reserve' + total = Reserve + S` and nothing is created. The same function (`ac_primitives::emission::settle`) runs in the runtime, in the node invariants and in the economic simulation (`tests/sim`, two four-year periods).
- **Treasury**: `treasury` is proportional to actual work emission — exactly the 20% of 10/50/20/20 at full demand, scaled down when demand is low (research 04 §2) — but never below the `5% × S` floor. The proportional share goes 40% to community grants and 60% to the holder treasury (genesis `community_share`, rounding remainder to the holder treasury); the holder treasury has no way out until on-chain holder voting (M8, D42). The floor top-up goes to a floor account in 30-day batches, each vesting linearly over 2 years from the end of its batch, spendable only for audits and cold start.
- Before M5 no work is verified and during PoA the security budget is not paid, so each epoch mints only the 5% floor.
- **Burns** (formerly the separate `pallet-fee-burn`, now merged into this pallet): every burn — 80% of transaction fees and tips, dust of reaped accounts, later slashing and inference fees — drops its credit through `Emission` and adds to `TotalBurned`.
- `k` (emission multiplier) and the burn ratio `b` are guardrailed: `0 ≤ k − b ≤ 0.5` (research 03 §1.5).

### 5.2 `pallet-credits`: unified credit interface (D20)

```rust
pub trait Credit {
    type Voucher: Encode + Decode;      // one-time payment voucher
    type Batch:   Encode + Decode;      // batched redemption data

    /// Gateway checks a voucher off-chain (not on chain, milliseconds)
    fn verify_offchain(v: &Self::Voucher, ctx: &RequestCtx) -> Result<Amount, Error>;
    /// Gateway redeems a batch on chain: double-spend protection + transfer to the gateway's settlement account
    fn redeem_batch(gateway: AccountId, batch: Self::Batch) -> DispatchResult;
}

// α: transparent implementation
TransparentVoucher { account, gateway, nonce, max_amount_usd, expiry, sig: PqSignature }
  // the user escrows prepaid credit with a gateway in pallet-gateways; vouchers are signed by the user
  // on batch redemption the gateway debits the user's escrow

// β: shielded implementation
ShieldedVoucher { nullifier, value_commitment, gateway_id, expiry, stark_proof }
  // spent from a shielded-pool note; the nullifier prevents double spends; neither gateway nor provider learns the source
```

- Market, gateway and settlement modules **depend only on the `Credit` trait**. When β ships the shielded implementation, the market code needs zero changes.
- At mainnet genesis, **the transparent implementation is not enabled for inference payments** (D20, item 4).

### 5.3 `pallet-providers` and `pallet-model-registry`

```rust
Model {
    id: H256,                          // = BLAKE3(canonicalized weight manifest)
    name, arch, quant: QuantType,      // e.g. FP8 / INT4-AWQ
    shard_hashes: BoundedVec<H256>,    // hash of each weight shard
    license_tag: LicenseTag,           // informational only, not checked by the protocol (D6)
    lineage: Option<(H256, LineageKind)>,  // parent model + derivation kind (D28)
    royalty: Option<RoyaltySpec>,      // [reserved] community-model royalties (D22), enabled in the full version
}

Provider {
    owner: AccountId, tier: Tier /* T1 | T2 | [reserved] T0 */,
    endpoint: BoundedVec<u8>,          // address gateways connect to (may be a relay address)
    models: BoundedVec<(ModelId, PriceUsd /* per million tokens, input and output priced separately */)>,
    stake: Balance, status: Active | Jailed | Exiting,
    kem_pk: (KemAlg, Bytes),           // end-to-end encryption from gateway to provider
    metrics: SlaMetrics,               // written from auditor measurements
    [reserved] attestation: Option<AttestationRef>,   // T0 TEE
}
```

- **Minimum stake** (in USD equivalent, converted at the reference rate): $1,000 for T1, $100 for T2 (draft).
- **Zero-stake entry**: new nodes without stake can only take jobs from `pallet-public-jobs` (research 05 B3).

### 5.4 `pallet-work`: settlement in the refine / accumulate pattern

1. **Refine (off-chain)**: for every completed request the provider produces a receipt:
   ```
   Receipt {
     gateway, provider, model_id, voucher_ref,
     in_tokens, out_tokens, price_usd,
     toploc_commit: H256,
     t_first_token, t_done,
     sig_provider, sig_gateway,
   }
   ```
   Signed by both parties and aggregated by the gateway.
2. **Accumulate (on-chain)**: once per epoch the gateway submits `WorkReport { merkle_root(receipts), totals, credit_batch }`:
   - calls `Credit::redeem_batch` to redeem credits;
   - pays providers per receipt (minus the gateway fee and the burned share);
   - updates providers' work scores (`EpochWork`);
   - enters a **challenge period** (e.g. 2 epochs) during which auditors may submit fraud proofs; emission is paid only after the challenge period.
3. Raw receipts stay with the gateway until the challenge period ends and are then deleted; they never go on chain; **prompts and outputs never go on chain**.

### 5.5 `pallet-audit`: mystery-shopper audits (privacy-preserving)

**Problem**: re-checking TOPLOC needs the prompt and output, but auditing real user requests would expose user content to auditors.

**Solution: audit only requests the auditors originate themselves.**
1. Each epoch, on-chain randomness assigns m auditors to each provider.
2. Auditors send requests through ordinary gateways (or relays) **as ordinary users**, paying with their own credits and vouchers and using prompts from a public question bank or generated at random. Providers **cannot tell** an audit from a real user.
3. With the output and TOPLOC proof in hand, the auditor (a T1/T2 auditor) recomputes the prefill locally with the declared model, compares it to the TOPLOC proof, and measures latency metrics.
4. It submits a verdict `Verdict { provider, model_id, pass | fail(evidence), metrics }`:
   - `fail` must carry evidence (output, TOPLOC proof, signed receipt) that anyone can re-check;
   - ≥2 independent `fail` verdicts against the same provider → dispute period → escalation to 5 auditors → on confirmation, **x% of stake is slashed** and the provider is jailed. Providers may appeal during the dispute period.
5. Auditors are rewarded from market-work emission; an auditor whose verdict is overturned is slashed.

**Effect**: user content never enters the audit process; since providers cannot distinguish audits from real requests, they must serve every request honestly.

### 5.6 `pallet-public-jobs` (lite)

- Job types (MVP): model evaluation (run public benchmarks and submit scores), data cleaning and deduplication, embedding computation.
- Verification: redundant execution (each job goes to 3 nodes; majority agreement passes) + sampled recomputation.
- Budget: the public-work emission share (20%); jobs are published by the holder treasury or governance. **[reserved]** training jobs in the full version.

### 5.7 `pallet-ref-rate` (D29)

- Stores `ATC_per_USD`; changeable only through a governance track; **each adjustment ≤ ±20%**, at least 1 day apart (enforced by guardrails).
- **[reserved]** `PriceSource` trait: a multi-source oracle (median + TWAP) in the full version.

### 5.8 `pallet-shielded` (β)

- **Note** = `(value, owner_pk_hash, rho, rcm)`; commitment = `Poseidon2(...)`; stored in an incremental Merkle tree of depth 32.
- **Spend**: a STARK proves "I know a note in the tree whose nullifier = PRF(sk, rho), and value is conserved".
- **Note encryption**: ML-KEM-768 + X25519 hybrid; recipients scan with a view key.
- **Voucher minting**: spend a note and create N one-time inference vouchers at once. Each voucher is a small note bound to a specific gateway, amount cap and expiry.
- **Proof size and performance**: expected 50–200 KB per proof, 1–10 s to generate on the client. **Optimizations**: mint vouchers in batches (amortizing proof cost); gateways verify off-chain and put them on chain in per-epoch batches.
- **[reserved]** `ShieldedAction` enum: `Transfer | MintVoucher | Vote | Bid | Swap`; the full version adds the L1 features.

### 5.9 `pallet-guardrails` (β)

```rust
// Every governable parameter registers a bound rule
Guardrail { param: ParamId, min, max, max_step_per_change, min_interval }
// e.g. TreasuryShare ∈ [0, 20%]; RefRate step ≤ ±20%; BurnRatio ∈ [10%, 90%]
```

- Every parameter change goes through `guardrails::check()`; a runtime upgrade must carry a `constitution_version` that matches the on-chain constitution hash before it can execute.

---

## 6. Node invariant checker (constitution layer 1, D30)

Written in the node client (native code, crate `ac-invariants`), **not in the runtime**, so no runtime upgrade can change it.

```rust
// ac-invariants (pure functions), called by ac-node's InvariantBlockImport for every block
fn check_block(params: &GenesisParams, n: u64, pre: Ledger, post: Ledger) -> Result<(), Violation> {
    ensure!(post.issuance <= CAP);                              // constitution 1: 21 million
    let minted = (post.issuance + post.burned) - (pre.issuance + pre.burned);
    match settled_epoch(n) {
        None => ensure!(minted == 0),                           // only settlement blocks mint
        Some(e) => ensure!(minted <= 2 * scheduled(e)),
    }
    let since_genesis = post.issuance + post.burned - params.genesis_issuance;
    ensure!(since_genesis <= cumulative_scheduled(settled_epochs(n)));
    Ok(())
}
```

- `Ledger` is read from published well-known keys: `Balances::TotalIssuance` and `Emission::TotalBurned` (SCALE `u128`). Minting is measured as the change of issuance + burned, so burning never hides minting, and a runtime that inflates the burned counter only makes the checks stricter. A missing or undecodable key rejects the block (fail-closed).
- `InvariantBlockImport` sits between the AC-BFT block import and the client: blocks authored locally, received or synced all pass through it. For other nodes' blocks it executes the block on the parent state and hands the storage changes on, so each block is executed once. Blocks that would be imported without execution are rejected, so warp and fast sync are not supported.
- At start-up the node checks the chain spec's genesis: a valid `Emission::EpochLength`, issuance within the cap, and for live chains zero issuance, no balances and at least one PoA admin member (`PoaCouncil::Members`).
- The PoA → PoS switch (constitution 4, `m3-pos`): on every block the node reads `ValidatorSet::{Phase, QualifiedSince, PoaAuthorities}` before and after. At each PoA epoch boundary it recomputes the checkpoint from the parent state (active stake summed over `StakingPos::Ledger`, qualified candidates from `StakingPos::Candidates`, the issuance and the height) with the runtime's own `transition_step`, and rejects a block whose phase or `QualifiedSince` differ: no early switch, no delayed switch, no way back, no PoA roster in PoS, and no change of the switch state outside boundaries. At start-up a live chain's `ValidatorSet::TransitionParams` must equal the constitution values (10%, 21, 63,115,200, 604,800).
- Changing these rules = releasing a new node client = a **hard fork**.
- "Protocol neutrality" (constitution 2) belongs to layer 2; the client adds an auxiliary check that the runtime contains no storage prefixes named `Blacklist`, `Blocklist`, `GeoFence`, etc. (only a helper; the main safeguard is layer-2 review).

---

## 7. Off-chain components

| Component | Responsibility | Key points |
|---|---|---|
| **ac-gateway** | OpenAI-compatible API (`/v1/chat/completions`, `/v1/models`); SSE streaming; voucher checks; routing by price, latency and reputation; automatic failover; receipt aggregation and batch settlement | **Permissionless, anyone can run one**, staked; **no request logs** (open-source code + public commitment; users may run their own gateway); ML-KEM hybrid encryption between gateway and provider |
| **ac-provider** | Wraps vLLM / SGLang; produces TOPLOC proofs; signs receipts; heartbeats; auto-registration | One command to start: `ac-provider --model Qwen/... --tier T2` |
| **ac-auditor** | Runs mystery-shopper requests as assigned on chain; recomputes TOPLOC locally; submits verdicts | Runs on T1 / T2 / T3 nodes (T3 only measures latency and consistency) |
| **ac-wallet** | CLI wallet: create and manage PQ keys, transfer, stake, rotate keys; β adds shielded pool and vouchers; external signer for Foundry | Mnemonic → seed → deterministic ML-DSA key derivation (via FIPS 204's seed-based key generation) |
| **ac-eth-rpc** | Ethereum JSON-RPC adapter | §3.4 |
| **ac-sdk** | **Rust core** (voucher management, PQ signing, STARK proof generation, chain interaction); Python bindings via PyO3, browser / TS bindings via wasm-bindgen, both thin wrappers | Only one Rust implementation of the cryptography, avoiding cross-language inconsistencies |
| **Block explorer** | Minimal version | Can be adapted from an open-source Substrate-ecosystem explorer |

### 7.1 Full flow of one inference request (β)

```
1. The client SDK takes a voucher V from its local pool (bound to gateway G, cap ≥ estimated cost)
2. POST https://G/v1/chat/completions  Header: X-AC-Voucher: V
3. G verifies V off-chain (STARK verification in a few ms, and checks the nullifier is unused)
4. G picks provider P by its routing policy and forwards the request with ML-KEM hybrid encryption (P only knows it came from G)
5. P streams tokens back; G relays them as SSE to the client
6. On completion: P signs receipt R (token counts, TOPLOC commitment); G co-signs; overpayment is returned to the client as a new change voucher
7. Every epoch: G submits a WorkReport (receipt Merkle root + voucher batch) → on-chain redemption, settlement, challenge period
8. After the challenge period: P receives its fee (minus gateway fee and burned share) + market-work emission
```

---

## 8. Token and fee parameters (MVP defaults)

| Parameter | Value | Guardrailed? |
|---|---|---|
| Total supply | 21,000,000 ATC (18 decimals) | Constitution layer 1 |
| Scheduled emission in the first 4-year period | 10,500,000, halving every 4 years thereafter | Constitution layer 1 |
| Emission epoch | 3,600 blocks on live chains (genesis parameter dividing 126,230,400 blocks) | Constitution layer 1 |
| Reserve drawdown cap | 1× the scheduled amount per epoch | Guardrail [0.5, 2] |
| Split | 10 / 50 / 20 / 20 (security / market / public / treasury) | Guardrail; treasury ≤ 20% |
| Treasury floor | 5% of the scheduled amount; 30-day batches, each vesting linearly over 2 years from its end; audits and cold start only | Guardrail |
| Treasury proportional share | 40% community grants / 60% holder treasury (locked until M8) | Guardrail (M8) |
| Inference-fee burn ratio `b` | 20% | Guardrail [10%, 90%] |
| Emission multiplier `k` | 0.5 | Guardrail: `k − b ≤ 0.5` |
| Gateway fee cap | 5% | Guardrail |
| Trainer royalty | 5% (enabled in the full version) | Guardrail [0, 10%] |
| Transaction fees | Weight + length based; of every fee and tip 20% (rounded down) to the block author, the rest burned; a share below the existential deposit of a new author account is burned | Guardrail |
| Existential deposit | 0.001 ATC | Runtime constant |
| PoA administration | Multisig of ML-DSA accounts with a threshold; 7-day motions | Genesis, then the multisig itself |
| PoA → PoS switch | 10% of the issuance staked, 21 qualified candidates, height ≥ 63,115,200, held for 604,800 blocks | Constitution layer 1 |
| Active set size `K` | 100 (storage bound 1,000) | PoA administration; guardrail (M8) |
| Minimum self-stake / nomination | 0.1% / 0.001% of the total issuance | Runtime |
| Candidates / nominators | at most 500 / 750 | Genesis |
| Unbonding | self-stake 28 days; nominations 2–28 days (queue) | Genesis |
| Commission | 5%–100%; increases after 7 days | Genesis |
| Slashing | vote double signing 100%, seal double signing 10% of self-stake; burned | Runtime |
| Block time | 1s | Runtime constant |
| Challenge period | 2 epochs | Guardrail |

---

## 9. Repository layout

```
agentcoin/
├── docs/
│   └── decisions.md · research/ · design/ · rust-guidelines/
├── openspec/                   # SDD: archived specs/ + local changes/
├── crates/
│   ├── ac-crypto/              # AlgId, PQ signature / KEM wrappers, test vectors
│   ├── ac-primitives/          # shared types: AccountId, Receipt, Voucher, …
│   ├── ac-invariants/          # node invariants (pure functions, amenable to formal verification)
│   └── ac-toploc/              # Rust binding / port of TOPLOC
├── node/                       # ac-node: Aura-PQ, AC-BFT, invariant checker, PQ libp2p
│   └── consensus/{aura-pq, ac-bft}/
├── runtime/                    # WASM runtime assembly
├── pallets/
│   ├── pq-accounts/ emission/ treasury-dual/ fee-burn/ validator-set/
│   ├── randomness-cr/ model-registry/ providers/ gateways/ credits/
│   ├── work/ audit/ public-jobs/ ref-rate/ guardrails/ constitution/
│   └── shielded/               # β
├── circuits/                   # β: STARK circuits (note spend, voucher mint)
├── services/
│   ├── gateway/ provider/ auditor/ eth-rpc/
├── clients/
│   ├── wallet-cli/ sdk/ (Rust core) sdk-bindings/{py (PyO3), wasm (wasm-bindgen)}/
├── contracts/                  # example Solidity contracts + PQ precompile interfaces
├── tests/
│   ├── e2e/                    # zombienet-style multi-node tests
│   └── sim/                    # economic simulation (emission, self-dealing, cold start)
└── .github/workflows/          # CI: fmt, clippy, unit tests, runtime build, e2e
```

---

## 10. Milestones and acceptance criteria

> Estimated for "one person + AI". Every milestone is accepted on a **runnable demo + automated tests**.

| Milestone | Time (months) | Deliverables | Acceptance criteria |
|---|---|---|---|
| **M0 Foundation** | 0–1 | Repo skeleton, CI, solochain template running, `ac-crypto` (ML-DSA, ML-KEM hybrid, with NIST test vectors) | CI green; 100% of test vectors pass |
| **M1 PQ chain** | 1–3 | `pallet-pq-accounts`, `PqAuthorize` transaction authorization, Aura-PQ, key-rotation transaction, CLI wallet | 3-node local network produces blocks; ML-DSA-signed transfers succeed; account ID unchanged after key rotation |
| **M2 Finality** | 3–5 | AC-BFT, slashing evidence, commit–reveal randomness | 4–10 nodes: finality P95 ≤3s; recovers after killing 1/3 of nodes; double-signing is slashed |
| **M3 Economics** | 4–6 | Emission, treasury, fee burning, validator-set state machine, **node invariants** | Simulated 8-year emission matches the formula exactly; a runtime upgrade that over-mints is rejected by nodes; PoA → PoS switches automatically when conditions are met |
| **M4 EVM** | 5–7 | `pallet-revive`, PQ precompiles, eth-RPC adapter, Foundry external signer | Deploy and call ERC-20 and Uniswap-V2-style contracts with Foundry |
| **M5 Inference market** | 6–9 | Model and provider registration, gateway (transparent credits), provider agent, work settlement | End to end: OpenAI SDK calls Qwen with streaming; extra TTFT overhead ≤150ms; batched receipt settlement is correct |
| **M6 Audits** | 8–10 | Auditor agent, mystery shopping, TOPLOC re-check, disputes and slashing, public jobs (lite) | A provider that swaps models / downgrades precision is detected and slashed within 1 hour; false-positive rate for honest providers <0.1% |
| **🚩 α testnet** | 10 | Public testnet + docs + faucet | An external provider can join within 30 minutes |
| **M7 Shielded pool** | 9–14 | STARK selection benchmark → circuits → `pallet-shielded` → voucher mint and redeem → wallet / SDK integration | Voucher flow works end to end; client proving ≤10s; on-chain verification ≤50ms; **internal circuit review + fuzzing** |
| **M8 Governance and constitution** | 12–15 | Token voting (referenda + conviction), guardrails, constitution pallet, sudo-removal process | Out-of-bounds parameter changes are rejected; upgrades take effect only after the mandatory delay |
| **🚩 β testnet** | 15 | Complete MVP | 30 days of stable operation; incentivized test (external miners invited) |
| **M9 Audit** | 15–18 | External security audit (circuits, consensus, economics) and fixes | No unresolved critical / high findings |
| **🚀 Mainnet Beta** | ≈18 | Genesis (PoA, no premine); gateways accept only anonymous vouchers | All §0.4 targets met |

**Status** (2026-09): M0–M3 complete. M4 in progress: `pallet-revive` with the PQ precompiles, the eth-RPC adapter and the Foundry external signer are done and tested end to end with an ERC-20; the Uniswap V2 acceptance run is pending.

**Critical path**: M1 → M2 → M5 → M7 → M9. M7 (STARK circuits) is the riskiest, so it starts in parallel with M5 / M6 and completes its technology selection before month 9.

---

## 11. Main risks and mitigations

| Risk | Impact | Mitigation |
|---|---|---|
| PQ signature libraries immature or side-channel prone | Private-key leakage | Use implementations validated by test vectors; constant-time implementations for validators; SLH-DSA reserved as a wallet fallback |
| Replacing Substrate consensus components takes longer than expected | Delay | AC-BFT is an independent crate depending only on `client.finalize_block`; if needed, α can use PoA single-signature finality as a stopgap |
| STARK circuit bug = inflation bug | Fatal | Minimal circuits (only two operations); formal spec + differential testing + external audit; on-chain invariant check that the shielded pool balance ≤ total transparent deposits |
| TOPLOC false positives across GPUs / engines | Honest providers wrongly punished | Thresholds calibrated per hardware/engine combination; two-level disputes; auditors slashed for wrong verdicts |
| Insufficient demand at cold start | Miners leave | Public job queue; treasury floor used to buy inference; zero-stake entry |
| Gateways become centralized bottlenecks or censorship points | Conflicts with the mission | Permissionless gateways; SDK fails over across gateways; users can run their own |
| Solo developer is a single point of failure | Schedule | Specs before code; tests for every module; open-source early to attract contributors; grants for external developers |
| Regulation (restrictions on privacy coins) | Listing difficulties | Protocol neutrality + selective disclosure (view keys); no dependence on centralized exchanges |
