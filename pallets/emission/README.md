> 🌐 **English** | [简体中文](README.zh-CN.md)

# pallet-emission

Scheduled emission of ATC (plan §5.1; decisions D11, D14–D16, D18, D19). It is the only place
where the runtime mints, and the node enforces its bounds independently (constitution layer 1,
`ac-invariants`).

## Schedule

- Constants: supply cap 21,000,000 ATC; the first four-year period emits 10,500,000 ATC over
  `BLOCKS_PER_PERIOD = 126,230,400` blocks (4 × 365.25 days of 1-second blocks); each later
  period emits half of the previous one.
- Blocks are grouped into **emission epochs** of `EpochLength` blocks. The length is a genesis
  parameter (3,600 on live chains; at most 20 on development chains) and must divide
  `BLOCKS_PER_PERIOD`, so halvings fall on epoch boundaries. A bad value makes genesis fail.
- Epoch `e` is in period `n = e × L / BLOCKS_PER_PERIOD` and has the scheduled amount
  `S(e) = (10,500,000 ATC >> n) × L / BLOCKS_PER_PERIOD`, rounded down.
- Blocks `1..=L` form epoch 0. Epoch `e` is settled in `on_initialize` of block `(e + 1) × L + 1`;
  no other block mints.

## Settlement

`ac_primitives::emission::settle` is the single implementation, shared with the node
invariants and the economic simulation (`tests/sim`):

```text
avail    = S + min(reserve, S)
security = 10% × S                          (paid only in PoS; PoA rolls it over)
market   = min(50% × avail, verified market work)
public   = min(20% × avail, verified public work)
treasury = max((market + public) × 20 / 70, 5% × S)   (the larger, never the sum)
total    = security + market + public + treasury, scaled down pro rata if above avail
reserve' = reserve + S − total
```

All arithmetic is on `u128` with rounding down; remainders stay in the reserve. The treasury
part goes to `Config::Treasury` (`pallet-treasury-dual`): the proportional share to community
grants and the holder treasury, the top-up to 5% × S to the vesting floor. Before M5 no work is
verified (`Config::WorkSource = ()`), and during PoA the security budget is not paid
(`Config::SecurityBudget = PoaPhase`), so each epoch mints only the 5% floor and the rest
accumulates in the reserve. Anything the currency refuses to mint (for example a share below the
existential deposit of a new account) also returns to the reserve.

## Burns

Every burn in the runtime (80% of transaction fees and tips today; slashing and inference fees
later) goes through this pallet's `OnUnbalanced` implementation: the credit is dropped, which
reduces the total issuance, and its amount is added to `TotalBurned`.

## Well-known storage keys

Published and read by the node invariants; never rename or re-encode them, and keep the pallet
named `Emission` in the runtime:

| Item | Key | Encoding |
|---|---|---|
| `Emission::TotalBurned` | `twox128("Emission") ‖ twox128("TotalBurned")` | SCALE `u128`, written at genesis |
| `Emission::EpochLength` | `twox128("Emission") ‖ twox128("EpochLength")` | SCALE `u64`, genesis parameter |

## Queries

Runtime API `EmissionApi`: `epoch_length`, `current_epoch`, `scheduled(epoch)`, `reserve`,
`total_minted`, `total_burned`. Each settlement emits `EpochSettled` with every part.

## Future guardrails

In M3 these are constants or genesis parameters; once the guardrail module exists (M8) they
become bounded governable parameters:

| Parameter | Today | Future guardrail range |
|---|---|---|
| Split security / market / public / treasury | 10 / 50 / 20 / 20 | bounded around these values |
| Reserve draw limit per epoch | 1 × S | [0.5, 2] × S |
| Community share of the proportional treasury part | 40% (genesis) | bounded range |

## Features

| Feature | Default | Purpose |
|---|---|---|
| `std` | yes | Native builds and tests. Disable it for the WASM runtime. |
| `runtime-benchmarks` | no | Benchmark of the settlement hook. |
| `try-runtime` | no | SDK try-runtime support. |
| `test-overmint` | no | **Test only**: settlement mints twice the scheduled amount more, to show that the node rejects such a runtime. Never enabled in a real build. |

## Example

Settling the first epoch of a live chain during PoA without work mints only the 5% floor:

```rust
use ac_primitives::emission::{EmissionSchedule, EpochInput, Phase, UNITS, settle};

let schedule = EmissionSchedule::new(3_600)?;
let s = schedule.scheduled(0);
assert_eq!(s, 10_500_000 * UNITS * 3_600 / 126_230_400);
let out = settle(&EpochInput::new(s, 0, (0, 0), Phase::Poa));
assert_eq!(out.total, s * 5 / 100);
assert_eq!(out.reserve, s - out.total);
# Ok::<(), ac_primitives::emission::EmissionError>(())
```
