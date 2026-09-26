> 🌐 **English** | [简体中文](README.zh-CN.md)

# pallet-randomness-cr

Commit–reveal epoch randomness from validators (plan §4.2), for audit sampling (M6) and other
low-value uses.

- **Commit**: in every epoch `e`, each validator commits, in a block it authors, to
  `derive_key("agentcoin 2026-09 randomness-commit v1", secret(e))`. The secret is derived from
  the validator's key, the genesis hash and `e` (`ac_crypto::randomness_secret`), so nobody else
  can predict it and a restarted validator can still reveal it.
- **Reveal**: in epoch `e + 1` the validator reveals `secret(e)` in one of its blocks; the chain
  checks it against the commitment.
- **Publish**: at the boundary of epoch `e + 2` the chain publishes
  `R(e) = derive_key("agentcoin 2026-09 randomness v1", u64_le(e) ‖ reveals sorted by account ID)`.
  Anyone can recompute it from the published reveals with
  `ac_primitives::randomness::epoch_randomness`. An epoch without reveals has no randomness.
- Both steps travel in the inherent `note_randomness`, filled by the author's node. The author
  comes from Aura-PQ's record of the block's slot, so nobody can commit or reveal for someone
  else. A second commitment in one epoch, or a reveal that does not match, is ignored.
- Validators that committed but did not reveal are counted in `MissedReveals`; M3 turns this
  into lost emission.
- **Queries**: the `RandomnessApi` runtime API (latest value, per-subject values, per-epoch
  values and reveals) and FRAME's `Randomness` trait for other pallets. Per-subject values are
  `derive_key("agentcoin 2026-09 randomness-subject v1", R ‖ subject)`, independent across
  subjects.

## Known bias

The last validator to reveal can withhold its reveal and so choose between two outcomes;
several colluding validators can choose among more. **Use this randomness only for low-value
purposes such as audit sampling**, never where a biased outcome is worth more than the
withholding validator's rewards. The full version removes the bias with a hash-based VDF (full
plan §2.4) behind the same interfaces.

## Features

| Feature | Default | Purpose |
|---|---|---|
| `std` | yes | Native builds and tests. Disable it for the WASM runtime. |
| `runtime-benchmarks` | no | Benchmarks of recording and epoch conclusion. |
| `try-runtime` | no | SDK try-runtime support. |

## Example

Recomputing an epoch's randomness from its reveals (order does not matter):

```rust
use ac_primitives::randomness::{epoch_randomness, subject_value};

let reveals = [([2u8; 32], [8u8; 32]), ([1u8; 32], [7u8; 32])];
let r = epoch_randomness(3, &reveals)?.expect("at least one reveal");
let swapped = [reveals[1], reveals[0]];
assert_eq!(epoch_randomness(3, &swapped)?, Some(r));
assert_ne!(subject_value(&r, b"audit")?, subject_value(&r, b"sample")?);
# Ok::<(), ac_primitives::randomness::RandomnessError>(())
```
