> 🌐 **English** | [简体中文](README.zh-CN.md)

# pallet-ac-offences

Double-signing evidence on chain (plan §4.1). Two kinds of evidence prove an offence by
themselves; the chain checks every ML-DSA signature and trusts no reporter:

- **Seal double signing**: two different block headers sealed by one ML-DSA-65 key in the same
  slot (context `agentcoin/aura-seal/v1`), the key being a member of a recent authority set and
  the slot no older than `MaxEvidenceAge`.
- **Vote double signing**: two different AC-BFT messages signed by one member of one set in one
  round, of the same kind — proposal, prepare or commit. Timeouts never count.

The evidence format and its verification (`ac_primitives::offences::verify_evidence`) are shared
with the node, which detects double signing on block import and in AC-BFT messages and submits
reports automatically through the `OffencesApi` runtime API.

## Submission without signature or fee

`report_equivocation(evidence)` authorizes itself: through the runtime's `AuthorizeCall`
transaction extension, a transaction with no account signature may call it when the evidence is
valid and new. The transaction pool checks this before admitting the report, so invalid evidence
never enters the pool or a block, and no account pays a fee or uses a nonce.

## Recording and consequences

- Each offence is recorded once, and each offender at most once per authority set; later
  evidence against a recorded offender in the same set is stale. A set's records never exceed
  its size and are dropped once the set leaves the validator set's history window.
- A recorded offender leaves block authoring and AC-BFT voting at the next epoch boundary
  (`pallet-validator-set`), which also keeps the set from ever becoming empty.
- **No slashing in M2**: during PoA validators hold no stake (decisions D9, D19), so the
  `SlashHandler` of M2 is `()`, slashes nothing and leaves every balance and the total issuance
  unchanged. M3 connects stake and slashes (and burns) 100%.
- Event: `OffenceReported { offender, key, set_id, slashed }`.

## Features

| Feature | Default | Purpose |
|---|---|---|
| `std` | yes | Native builds and tests. Disable it for the WASM runtime. |
| `runtime-benchmarks` | no | Benchmarks of evidence verification and recording. |
| `try-runtime` | no | SDK try-runtime support. |

## Example

Evidence identifies its offence, so a node can tell duplicates apart before submitting:

```rust
use ac_primitives::offences::{OffenceKey, OffenceKind};
use ac_primitives::ac_bft::MessageKind;

let key = OffenceKey::Bft { set_id: 0, signer: 2, round: 7, kind: MessageKind::Commit };
assert_eq!(key.kind(), OffenceKind::BftEquivocation);
```
