> 🌐 **English** | [简体中文](README.zh-CN.md)

# pallet-validator-set

Epochs, the authority set shared by Aura-PQ block authoring and AC-BFT finality, and the
one-way PoA → PoS switch (plan §4.1, §4.3; decisions D19, D24; design D6, D7 of `m3-pos`).

- **Epochs**: blocks are grouped into epochs of `epoch_length` blocks, fixed at genesis and at
  least twice the larger of the PoS active-set size `K` and the PoA set size (so every validator
  gets several slots per epoch). Block `b ≥ 1` is in epoch `⌊(b − 1) / L⌋`; the first block of an epoch is its boundary.
- **Authority set**: ML-DSA-65 keys in order, each with a weight (1 during PoA; in PoS
  `max(1, backing / 10^12)`), and a set id that starts at 0 and increases by one on every
  change. Slots rotate through the members regardless of weight; AC-BFT counts weight.
- **Changes only at boundaries**, for three reasons: the administration changed the PoA roster;
  a validator recorded for double signing (`pallet-ac-offences` calls
  `ValidatorSetInterface::disable`) leaves; in PoS, the election result changed. The boundary
  block installs the new list for block authoring from the next block, deposits an `acbf` digest
  with the new set id and members for AC-BFT, and emits `AuthorityDisabled` (offenders) and
  `NewSet`. Boundaries without a change carry no digest. The set is never emptied: if every
  member offended, the first one in set order stays.
- **PoA roster**: `add_poa_authority` and `remove_poa_authority` (administration only) change
  the roster, which becomes the set at the next boundary. The roster is never emptied and never
  exceeds half the epoch length. Offenders leave it for good.
- **Switch to PoS**: every PoA boundary is a checkpoint. It passes when all active stake is at
  least 10% of the issuance, at least 21 candidates are qualified and the height is at least
  63,115,200 (live values; development presets use smaller ones). `QualifiedSince` records the
  first passing checkpoint of an uninterrupted run and is cleared by any failing one. The
  boundary at which the run has lasted 604,800 blocks switches to PoS: the roster is emptied,
  the elected validators form the set, and the administration can no longer change it. During
  the run, the last block of every epoch publishes a preview election. The switch is one-way.
- **PoS**: the last block of every epoch runs the election for `K` seats (`set_validator_count`,
  administration only, between the switch's candidate count and half the epoch length); the
  boundary installs it, leaving out disabled validators.
- **Well-known storage keys**, read by the node, which recomputes every checkpoint and rejects
  any block that switches early, late or back: `ValidatorSet::Phase` (`0` PoA, `1` PoS),
  `ValidatorSet::QualifiedSince` (`Option<u64>`), `ValidatorSet::PoaAuthorities` (keys) and
  `ValidatorSet::TransitionParams` (`stake_bps: u32, min_candidates: u32, min_height: u64,
  sustain_blocks: u64`). They are never renamed or re-encoded.
- **History**: sets active within the last `HistoryEpochs` epochs stay queryable for evidence
  verification (`ValidatorSetInterface::historical`, `is_recent_member`).
- **Runtime API**: `ac_primitives::validator_set::ValidatorSetApi` (current set, epoch length,
  historical sets); the switch progress is part of `ac_primitives::staking::StakingApi`.

## Features

| Feature | Default | Purpose |
|---|---|---|
| `std` | yes | Native builds and tests. Disable it for the WASM runtime. |
| `runtime-benchmarks` | no | Benchmarks of epoch-boundary processing and of the roster calls. |
| `try-runtime` | no | SDK try-runtime support. |

## Example

Epoch arithmetic and the switch checkpoint, shared by the runtime and the node:

```rust
use ac_primitives::epoch::{check_epoch_length, epoch_of, is_boundary};
use ac_primitives::staking::{TransitionInputs, TransitionParams, transition_step};

// Four authorities need epochs of at least eight blocks.
assert!(check_epoch_length(7, 4).is_err());
assert!(check_epoch_length(20, 4).is_ok());

// With 20-block epochs, block 21 opens epoch 1.
assert_eq!(epoch_of(20, 20), Some(0));
assert_eq!(epoch_of(21, 20), Some(1));
assert!(is_boundary(21, 20));

// A checkpoint two years in with 12% staked and 25 candidates starts the qualified run.
let step = transition_step(
    None,
    &TransitionInputs::new(12, 100, 25, 63_115_200),
    &TransitionParams::CONSTITUTION,
);
assert_eq!(step.qualified_since, Some(63_115_200));
assert!(!step.switch);
```
