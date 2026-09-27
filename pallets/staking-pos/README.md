> 🌐 **English** | [简体中文](README.zh-CN.md)

# pallet-staking-pos

Nominated proof of stake for AgentCoin (plan §4.3; decisions D19, D24; design D1–D4 of
`m3-pos`): the staking ledger behind the PoA → PoS switch, the validator election and slashing.
Bonding, unbonding and withdrawing never mint or burn: stake is held on the account with the
`StakingPos` hold reason.

## Roles

An account is either a **candidate** or a **nominator**, never both; changing role requires an
empty ledger (all earlier stake withdrawn).

- **Candidates** register an ML-DSA-65 validator key with a proof of possession: the key's
  signature, with context `agentcoin/validator-pop/v1`, over SCALE(`"agentcoin/validator-pop-statement"`,
  genesis hash, account, key). The key signs blocks, AC-BFT votes and randomness. A key is
  bound to one account forever, even after the candidate leaves.
- **Nominators** bond an amount and name 1 to 16 candidates. They can change their targets
  without unbonding; the change counts from the next election.

## Minimums and caps

| | Minimum | Cap (live chains) |
|---|---|---|
| Candidate self-stake | 0.1% of the total issuance | 500 candidates |
| Nomination | 0.001% of the total issuance | 2,000 nominators |

Minimums follow the issuance and are rounded up. A candidate whose self-stake falls below the
minimum as the issuance grows keeps its stake but is not qualified (does not count for the
switch and is not elected) until it tops up. When a list is full, a newcomer must bond more
than its smallest entry; that entry is removed and starts unbonding.

## Unbonding

Unbonding stake stops counting at once but stays held until it unlocks:

- a candidate's self-stake after a fixed 28 days (2,419,200 blocks);
- a nomination through a network-wide queue that drains the whole active stake in 28 days:
  each request waits between 2 and 28 days, longer when much stake leaves at the same time
  (after Polkadot RFC-0097).

The same rules apply in the PoA and PoS phases. Candidates leave with `retire`; nominators with
`unnominate` (or by unbonding everything).

## Commission

5% to 100%. An increase takes effect 7 days later (604,800 blocks); a decrease at the next
validator epoch. Until then the scheduled value is visible in the candidate record.

## Well-known storage keys

Read by the node's PoA → PoS check; never renamed or re-encoded:

| Key | Value |
|---|---|
| `StakingPos::Ledger` (`Identity` account) | `{ active: u128, unlocking: Vec<{ value: u128, unlock_at: u64 }> }` |
| `StakingPos::Candidates` (`Identity` account) | `{ key, commission_bps: u32, pending_commission: Option<(u32, u64)>, chilled: bool }` |

## Calls

`register_candidate`, `bond_extra`, `set_validator_key`, `retire`, `nominate`,
`set_nominations`, `unnominate`, `unbond`, `withdraw_unbonded`, `set_commission`, `chill`,
`validate`. All weights are benchmarked; registration and nomination are charged for scanning a
full list and refunded when they do not.

## Queries

Runtime API `StakingApi`: `stake` (active, unbonding and withdrawable amounts of an account),
`candidate` (record, commission in force and received nominations), `total_active` and
`minimums`.

## Features

| Feature | Default | Purpose |
|---|---|---|
| `std` | yes | Native builds and tests. Disable it for the WASM runtime. |
| `runtime-benchmarks` | no | Benchmarks of every call. |
| `try-runtime` | no | SDK try-runtime support. |

## Example

Live-chain parameters and the minimums for an issuance of 10,000,000 ATC:

```rust
use ac_primitives::emission::UNITS;
use ac_primitives::staking::{min_nomination, min_self_bond};
use pallet_staking_pos::StakingParams;

let live = StakingParams::LIVE;
assert_eq!(live.self_unbond_blocks, 28 * 86_400);
assert_eq!((live.nomination_unbond_min, live.nomination_unbond_max), (2 * 86_400, 28 * 86_400));
assert_eq!((live.max_candidates, live.max_nominators), (500, 2_000));

let issuance = 10_000_000 * UNITS;
assert_eq!(min_self_bond(issuance), 10_000 * UNITS);
assert_eq!(min_nomination(issuance), 100 * UNITS);
```
