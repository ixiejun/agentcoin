> 🌐 **English** | [简体中文](README.zh-CN.md)

# pallet-validator-set

Epochs and the authority set shared by Aura-PQ block authoring and AC-BFT finality (plan §4.1,
§4.3). This is the M2 part of the validator set named in the full plan (§11); M3 adds the
PoA → PoS state machine, stake weights and session-key rotation to the same pallet.

- **Epochs**: blocks are grouped into epochs of `epoch_length` blocks, fixed at genesis and at
  least twice the number of genesis authorities (so every validator gets several slots per
  epoch). Block `b ≥ 1` is in epoch `⌊(b − 1) / L⌋`; the first block of an epoch is its boundary.
- **Authority set**: ML-DSA-65 keys in order, each with a weight (1 during PoA; reserved for
  stake weighting), and a set id that starts at 0 and increases by one on every change.
- **Changes only at boundaries**: validators recorded for double signing (`pallet-ac-offences`
  calls `ValidatorSetInterface::disable`) leave the set at the next boundary. The boundary block
  installs the new list for block authoring from the next block, deposits an `acbf` digest with
  the new set id and members for AC-BFT, and emits `AuthorityDisabled` and `NewSet`. Boundaries
  without removals change nothing and carry no digest. The set is never emptied: if every member
  offended, the first one in set order stays.
- **History**: sets active within the last `HistoryEpochs` epochs stay queryable for evidence
  verification (`ValidatorSetInterface::historical`, `is_recent_member`).
- **Runtime API**: `ac_primitives::validator_set::ValidatorSetApi` (current set, epoch length,
  historical sets).

## Features

| Feature | Default | Purpose |
|---|---|---|
| `std` | yes | Native builds and tests. Disable it for the WASM runtime. |
| `runtime-benchmarks` | no | Benchmarks of epoch-boundary processing. |
| `try-runtime` | no | SDK try-runtime support. |

## Example

Epoch arithmetic shared by the runtime and the node:

```rust
use ac_primitives::epoch::{check_epoch_length, epoch_of, is_boundary};

// Four authorities need epochs of at least eight blocks.
assert!(check_epoch_length(7, 4).is_err());
assert!(check_epoch_length(20, 4).is_ok());

// With 20-block epochs, block 21 opens epoch 1.
assert_eq!(epoch_of(20, 20), Some(0));
assert_eq!(epoch_of(21, 20), Some(1));
assert!(is_boundary(21, 20));
```
