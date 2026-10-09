> 🌐 **English** | [简体中文](README.zh-CN.md)

# ac-primitives

Shared on-chain types for AgentCoin, used by the runtime, the node and clients.

- `Blake3Hasher`: BLAKE3-256 adapted to the Polkadot SDK hasher traits. It is the chain's
  `Hashing` type, so block hashes, extrinsics roots and state roots are all BLAKE3 (decision D35).
- Addresses: `encode_address` / `decode_address` turn a 32-byte account ID into an `atc1…`
  bech32m (BIP-350) string and back; one wrong character is always detected.
- `NoClassicSignature`: an uninhabited type that fills the signature slot of the SDK's legacy
  `Signed` extrinsic, so no classic signature scheme can authorize an account (decision D36).
- `ChainProfile` and the `ChainProfileApi` runtime API: lets clients check the chain's hashing
  scheme.
- `aura_pq`: Aura-PQ slot, pre-runtime digest and seal digest helpers.
- `ac_bft`: AC-BFT finality types — versioned signed messages (proposals, prepare / commit votes,
  timeouts) signed with context `agentcoin/bft-vote/v1`, finality proofs and
  `verify_finality_proof`, and the authority-set change digest (engine `acbf`). Unknown format
  versions fail to decode instead of being read as another format.
- `offences`: double-signing evidence (two blocks sealed in one slot, or two conflicting AC-BFT
  messages in one round) and `verify_evidence`, used by the runtime and the node alike.
- `epoch`: epoch numbering (`epoch_of`, `is_boundary`) and the minimum epoch length.
- `emission`: the ATC emission curve and epoch settlement (plan §5.1). The runtime, the node's
  invariant checker and the economic simulation all use it, so they compute the same numbers.
- `staking`: the PoA → PoS switch rule and its constitution values, minimum stakes, the
  nomination unbonding queue, reward splitting and election inputs. The runtime and the node's
  invariant checker share it, so both reach the same switch decision.
- `market`: inference-market types (M5): dollar amounts and the reference rate with explicit
  rounding (payments round down, thresholds up), model manifests and model IDs (context
  `agentcoin 2026-09 model-id v1`), cumulative transparent vouchers signed with
  `agentcoin/voucher/v1` and `check_voucher` (the one implementation of the redemption rules,
  used on and off chain), provider / gateway / channel records, the interfaces between the
  market pallets (`PriceSource`, `Credit`, `ProviderPenalty`, …) and the `MarketApi` runtime API.

Byte-level regression vectors for the AC-BFT formats live in `tests/vectors/` (see `SOURCES.md`).

## Features

| Feature | Default | Purpose |
|---|---|---|
| `std` | yes | Standard-library support for the node, wallet and tests. Disable it for the WASM runtime. |

## Example

```rust
use ac_primitives::{Blake3Hasher, decode_address, encode_address};
use sp_runtime::traits::Hash;

let digest = Blake3Hasher::hash(b"agentcoin");
assert_eq!(digest.as_ref().len(), 32);

let address = encode_address(&[7u8; 32]);
assert!(address.starts_with("atc1"));
assert_eq!(decode_address(&address), Ok([7u8; 32]));
```

## Verifying a finality proof

A finality proof is checked with nothing but the genesis hash and the authority set it names, so
light clients can verify finality without trusting a node.

```rust
# #[cfg(feature = "std")] {
use ac_crypto::{SigAlg, dev_seed, sig::SigningKey};
use ac_primitives::ac_bft::{
    Authority, BlockRef, FinalityProof, Message, VOTE_CONTEXT, VersionedFinalityProof, VoteKind,
    signing_payload, verify_finality_proof,
};
use sp_core::H256;

let keys: Vec<SigningKey> = ["alice", "bob", "charlie", "dave"]
    .iter()
    .map(|n| SigningKey::from_seed(SigAlg::MlDsa65, &dev_seed(n)?))
    .collect::<Result<_, _>>()?;
let set: Vec<Authority> = keys
    .iter()
    .map(|k| k.public_key().map(Authority::poa))
    .collect::<Result<_, _>>()?;

let genesis = H256::repeat_byte(1);
let target = BlockRef { hash: H256::repeat_byte(2), number: 10 };
let commit = Message::Vote { kind: VoteKind::Commit, round: 0, target };
let payload = signing_payload(&genesis, 0, &commit).expect("fixed context");

// Three of four members (q = ⌊2·4/3⌋ + 1 = 3) signed the commit vote.
let mut commits = Vec::new();
for (index, key) in keys.iter().enumerate().take(3) {
    commits.push((index as u16, key.sign_deterministic(&payload, VOTE_CONTEXT)?));
}
let proof = VersionedFinalityProof::V1(FinalityProof {
    set_id: 0,
    round: 0,
    target,
    commits: commits.try_into().expect("at most 100 members"),
});
assert_eq!(verify_finality_proof(&genesis, 0, &set, &proof), Ok(target));
# }
# Ok::<(), ac_crypto::Error>(())
```

## Emission schedule

- The first four-year period (126,230,400 blocks at one block per second) emits
  10,500,000 ATC; every later period emits half of the previous one, so the whole curve stays
  below the 21,000,000 ATC cap (decisions D14, D15).
- Emission is settled per *emission epoch* of `L` blocks, a genesis parameter that must divide
  126,230,400 (live chains: 3,600). Epoch `e` is settled in block `(e + 1) × L + 1`; only these
  blocks mint.
- `settle` splits an epoch: security budget 10% of the scheduled amount `S` (paid only under
  PoS), market work up to 50% and public work up to 20% of `S + min(reserve, S)`, and the
  treasury `max(5% × S, 20/70 × work emission)` — the larger, never the sum. What is not
  minted rolls over into the reserve; at most `S` is drawn from it per epoch. Integer shares in
  basis points, rounded down.

```rust
use ac_primitives::emission::{EmissionSchedule, EpochInput, Phase, settle};

let schedule = EmissionSchedule::new(3_600)?;
let s = schedule.scheduled(0);
// A PoA epoch without work mints only the 5% treasury floor; the rest rolls over.
let out = settle(&EpochInput::new(s, 0, (0, 0), Phase::Poa));
assert_eq!(out.total, s * 5 / 100);
assert_eq!(out.reserve + out.total, s);
// Block 3,601 settles epoch 0.
assert_eq!(schedule.settled_epoch(3_601), Some(0));
# Ok::<(), ac_primitives::emission::EmissionError>(())
```

## Staking and the PoA → PoS switch

- Constitution values (decisions D19, D24): total active stake ≥ 10% of the issuance, at least
  21 qualified candidates, height ≥ 63,115,200 (two years), and all three holding at every
  epoch boundary for 604,800 blocks (seven days). `TransitionParams::CONSTITUTION` holds them;
  `ChainPhase` and `TransitionParams` are values of published well-known storage keys and
  never change their encoding.
- `transition_step` performs one epoch-boundary checkpoint: it clears the start of the
  qualified run when a checkpoint fails, sets it at the first passing one, and switches once
  the run has lasted the sustain period. The switch is one-way.
- Minimum self-stake 0.1% and minimum nomination 0.001% of the issuance, rounded up.
- `unbonding_unlock`: nominations leave through a network-wide queue that drains the whole
  active stake in the maximum period; each request waits between the minimum (2 days on live
  chains) and the maximum (28 days).
- `split_by_points` shares the security budget by work points (blocks authored), independent
  of stake; `split_reward` takes the commission and splits the rest by backing. Both round
  down, so no ATC is created.

```rust
use ac_primitives::staking::{
    TransitionInputs, TransitionParams, min_self_bond, split_reward, transition_step,
};

let params = TransitionParams::CONSTITUTION;
let issuance = 1_000_000u128;
// 12% staked, 25 candidates, two years reached: the qualified run starts here.
let first = transition_step(None, &TransitionInputs::new(120_000, issuance, 25, 63_115_200), &params);
assert_eq!(first.qualified_since, Some(63_115_200));
assert!(!first.switch);
// Seven days later, still qualified: switch to PoS.
let later = TransitionInputs::new(120_000, issuance, 25, 63_115_200 + 604_800);
assert!(transition_step(first.qualified_since, &later, &params).switch);

assert_eq!(min_self_bond(issuance), 1_000);
// 10% commission, then 1/3 to the validator's own stake and 2/3 to its nominator.
let split = split_reward(100, 1_000, &[("validator", 1), ("nominator", 2)]);
assert_eq!(split.commission, 10);
assert_eq!(split.shares, vec![("validator", 30), ("nominator", 60)]);
```

## Statistical judgment of audits

`market::audit::stats` holds the per-provider statistical judgment (OpenSpec change
`m6-audit-sprt`). It catches deviations too subtle for a single audit, such as 8-bit weights,
including 8-bit weights used only while decoding.

- `AuditStats` are the integers a verdict judged by the thresholds carries:
  - the prompt token count;
  - the prefill chunk's mean mantissa error;
  - the decode chunks' mean errors, averaged;
  - the number of decode chunks.

  Means are in hundredths and rounded down. A chunk with no matching exponent, or any value
  above 65,535, saturates at 65,535. Without decode chunks the decode mean is 0.
  `AuditStats::from_chunks` computes them from a re-check's chunks.
- `StatsParams` is one version of the parameters; the versions are listed in `AUDIT_STATS`,
  and `stats_params` looks one up. A version holds:
  - the audit length band;
  - bin edges and log-likelihood ratios for each statistic, in thousandths of a nat;
  - the clamp on one verdict's contribution;
  - the per-auditor cap, a third of the bound, so crossing needs at least three auditors;
  - the bound;
  - the most entries a provider's state keeps.

  Version 1 is provisional, derived from the GPU calibration: clamp −0.5 / +3.0 nats, bound
  24.7 nats.
- A verdict's contribution is the sum of two ratios, clamped:
  - the prefill bin's ratio, counted only when the prompt is in the band;
  - the decode bin's ratio, counted only when there are decode chunks.
- `SprtState` is a provider's CUSUM: `S ← max(0, S + counted)`.
  - A state back at 0 forgets its entries.
  - A positive contribution counts only up to what is left of its auditor's cap.
  - A full list drops its oldest entry and replays the rest.
  - `SprtState::replay` recomputes a state from its entries; anyone can check the chain with it.

```rust
use ac_primitives::market::audit::{AuditStats, CURRENT_STATS, SprtEntry, SprtState};

let p = CURRENT_STATS;
// An 8-bit-like verdict: high prefill and decode means for a prompt in the band.
let int8 = AuditStats { prompt_tokens: 200, prefill_mean_centi: 125, decode_mean_centi: 250, decode_chunks: 4 };
assert_eq!(p.contribution(&int8), 3_000); // clamped to +3.0 nats
// An honest-looking one counts against the state, down to the floor of −0.5 nats.
let honest = AuditStats { prompt_tokens: 200, prefill_mean_centi: 50, decode_mean_centi: 100, decode_chunks: 4 };
assert_eq!(p.contribution(&honest), -500);

let mut s = SprtState::default();
for (auditor, round) in [(1u8, 0), (2, 0), (3, 1), (4, 1), (5, 2), (6, 2), (7, 3), (8, 3), (9, 4)] {
    s.record(&p, SprtEntry { auditor, round, contribution: p.contribution(&int8) });
}
assert!(s.crossed(&p)); // 27.0 nats ≥ 24.7
assert_eq!(s, SprtState::replay(&p, s.entries.clone()));
```
