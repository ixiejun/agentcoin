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
- **PoA → PoS switch** (`check_transition`; decisions D19, D24):
  - the phase never goes back from PoS to PoA, and in PoS the PoA roster is empty;
  - outside PoA epoch boundaries the phase and the start of the qualified run do not change;
  - at a PoA boundary the node recomputes the checkpoint from the parent state (all active
    stake summed over the ledger, qualified candidates, issuance, height) with
    `ac_primitives::staking::transition_step`; the block's phase and start of the qualified run
    must be exactly that result, so the switch happens neither early nor late.
- **Fail-closed**: a missing or undecodable well-known value rejects the block.
- **Genesis** (at start-up): the emission epoch length is present and divides the four-year
  period; the issuance is within the cap; the switch parameters and the validator epoch length
  are present; a live chain allocates no ATC (no premine, D9), names PoA admin members and uses
  the constitution's switch values (10%, 21 candidates, 63,115,200 blocks, 604,800 blocks).

## Well-known keys (`keys`)

| Constant | Storage item | Encoding |
|---|---|---|
| `TOTAL_ISSUANCE` | `Balances::TotalIssuance` | `u128` |
| `TOTAL_BURNED` | `Emission::TotalBurned` | `u128`, never decreasing |
| `EMISSION_EPOCH_LENGTH` | `Emission::EpochLength` | `u64`, genesis only |
| `SYSTEM_ACCOUNT_PREFIX` | `System::Account` | account info with `u128` balances |
| `POA_COUNCIL_MEMBERS` | `PoaCouncil::Members` | `Vec<AccountId32>` |
| `PHASE` | `ValidatorSet::Phase` | `u8` enum: `0` PoA, `1` PoS |
| `QUALIFIED_SINCE` | `ValidatorSet::QualifiedSince` | `Option<u64>`, always present |
| `POA_AUTHORITIES` | `ValidatorSet::PoaAuthorities` | vector of tagged public keys |
| `TRANSITION_PARAMS` | `ValidatorSet::TransitionParams` | `(u32, u32, u64, u64)`, genesis only |
| `VALIDATOR_EPOCH_LENGTH` | `ValidatorSet::EpochLength` | `u64`, genesis only |
| `STAKING_LEDGER_PREFIX` | `StakingPos::Ledger` (`Identity` account) | ledger, first field `u128` active |
| `STAKING_CANDIDATES_PREFIX` | `StakingPos::Candidates` (`Identity` account) | `CandidateRecord` |

## Features

| Feature | Default | Purpose |
|---|---|---|
| `std` | yes | Standard library and `std::error::Error` for the error types. |

## Example

```rust
use ac_invariants::{
    GenesisParams, Ledger, StakeSnapshot, SwitchState, TransitionGenesis, Violation, check_block,
    check_transition,
};
use ac_primitives::emission::EmissionSchedule;
use ac_primitives::staking::{ChainPhase, TransitionParams};

let params = GenesisParams {
    schedule: EmissionSchedule::new(10)?,
    genesis_issuance: 0,
    transition: TransitionGenesis {
        params: TransitionParams::CONSTITUTION,
        epoch_length: 600,
    },
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

// A runtime that switches to PoS in its first year is rejected, whatever the stake.
let poa = SwitchState { phase: ChainPhase::Poa, qualified_since: None, poa_authorities: 4 };
let pos = SwitchState { phase: ChainPhase::Pos, qualified_since: None, poa_authorities: 0 };
let stake = || Ok(StakeSnapshot { total_active: 50, issuance: 100, qualified_candidates: 30 });
assert_eq!(
    check_transition(&params.transition, 601, &poa, &pos, stake),
    Err(Violation::SwitchEarly)
);
# Ok::<(), ac_primitives::emission::EmissionError>(())
```
