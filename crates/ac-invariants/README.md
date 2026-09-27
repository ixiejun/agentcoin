> 🌐 **English** | [简体中文](README.zh-CN.md)

# ac-invariants

Constitution layer 1 of AgentCoin as pure functions (decisions D24, D30; MVP plan §6). The node
client runs them on every block it imports or authors and rejects the block when one fails.
They read only published well-known storage keys and parameters fixed in the chain spec, never
the runtime, so no runtime upgrade can bypass them.

**Changing any rule or key here is a hard fork**: it needs a new node release, its own OpenSpec
change and the user's explicit confirmation (AGENT.md §8).

## Rules

- **Supply cap**: total issuance ≤ 21,000,000 ATC.
- **Minting bounded by the emission curve**, with `minted = Δissuance + Δburned`:
  - a block that settles no emission epoch mints nothing;
  - the block settling epoch `e` mints at most `2 × S(e)` (the scheduled amount plus at most
    as much again from the reserve);
  - everything minted since genesis stays within the cumulative scheduled amount of the epochs
    settled so far.
- **Fail-closed**: a missing or undecodable well-known value rejects the block.
- **Genesis** (at start-up): the emission epoch length is present and divides the four-year
  period; the issuance is within the cap; a live chain allocates no ATC (no premine, D9) and
  names PoA admin members.

## Well-known keys (`keys`)

| Constant | Storage item | Encoding |
|---|---|---|
| `TOTAL_ISSUANCE` | `Balances::TotalIssuance` | `u128` |
| `TOTAL_BURNED` | `Emission::TotalBurned` | `u128`, never decreasing |
| `EMISSION_EPOCH_LENGTH` | `Emission::EpochLength` | `u64`, genesis only |
| `SYSTEM_ACCOUNT_PREFIX` | `System::Account` | account info with `u128` balances |
| `POA_COUNCIL_MEMBERS` | `PoaCouncil::Members` | `Vec<AccountId32>` |

## Features

| Feature | Default | Purpose |
|---|---|---|
| `std` | yes | Standard library and `std::error::Error` for the error types. |

## Example

```rust
use ac_invariants::{GenesisParams, Ledger, Violation, check_block};
use ac_primitives::emission::EmissionSchedule;

let params = GenesisParams {
    schedule: EmissionSchedule::new(10)?,
    genesis_issuance: 0,
};
let before = Ledger { issuance: 0, burned: 0 };
// Block 5 settles no emission epoch: minting even one unit is rejected.
let after = Ledger { issuance: 1, burned: 0 };
assert!(matches!(
    check_block(&params, 5, before, after),
    Err(Violation::MintOutsideSettlement { minted: 1 })
));
// Block 11 settles epoch 0 and may mint up to its scheduled amount.
let s0 = params.schedule.scheduled(0);
assert!(check_block(&params, 11, before, Ledger { issuance: s0, burned: 0 }).is_ok());
# Ok::<(), ac_primitives::emission::EmissionError>(())
```
