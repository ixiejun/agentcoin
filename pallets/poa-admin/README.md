> 🌐 **English** | [简体中文](README.zh-CN.md)

# pallet-poa-admin

The chain's administration during proof of authority (decision D41): a set of ML-DSA member
accounts fixed at genesis that act together once a threshold `t` of them approves
(`1 ≤ t ≤ members`). It covers runtime upgrades, treasury spends and its own membership, and is
handed over to on-chain governance later.

## How a decision is made

Proposals, votes and closing come from the SDK's `pallet-collective` (instance `Instance1`,
named `PoaCouncil` in the runtime):

1. a member calls `PoaCouncil::propose(threshold, call, length_bound)`; the motion is identified
   by the BLAKE3 hash of the call;
2. members call `PoaCouncil::vote` (the proposer too: proposing is not a vote);
3. anyone calls `PoaCouncil::close` once enough members approved, or after the motion duration
   (7 days on live chains, 20 blocks on development chains), when members who did not vote
   count as no.

An approved motion runs its call with the collective origin `Members(yes, total)`.
`EnsureCouncilThreshold` accepts that origin only when `yes ≥ Threshold`, so a member cannot
get around the threshold by proposing with a lower motion threshold. Non-members can neither
propose nor vote.

## Calls

| Call | Origin | Effect |
|---|---|---|
| `dispatch_as_root(call)` | administration | Runs `call` as Root (for example `System::set_code`) after `Config::RootCallFilter` allowed it; emits `DispatchedAsRoot` with the result. |
| `set_threshold(t)` | administration | `1 ≤ t ≤ members`. |
| `set_members(members, t)` | administration | Replaces the members and the threshold together; drops removed members' votes from open motions. The collective's own `set_members` is closed (`SetMembersOrigin = EnsureNever`). |

Because members and threshold only change together and are checked, the administration can
never lock itself out.

## Genesis

`PoaCouncil.members` and `PoaAdmin.threshold`. A threshold outside `1..=members` makes genesis
fail; no members and threshold 0 leave the administration unset, which the node refuses on a
live chain (`ac-invariants::check_genesis`). Presets: `dev` alice / 1, `local` alice, bob,
charlie / 2.

## Why not sudo or pallet-multisig

Sudo gives one key control of the whole chain. The SDK's `pallet-multisig` derives multisig
accounts with BLAKE2-256, an integrity-bearing identifier outside the exception of decision
D35; `pallet-collective` uses only the chain hasher.

## Features

| Feature | Default | Purpose |
|---|---|---|
| `std` | yes | Native builds and tests. Disable it for the WASM runtime. |
| `runtime-benchmarks` | no | Benchmarks of the three calls. |
| `try-runtime` | no | SDK try-runtime support. |

## Example

The council instance and the default motion duration of live chains:

```rust
use pallet_poa_admin::{CouncilInstance, DEFAULT_MOTION_DURATION};

assert_eq!(DEFAULT_MOTION_DURATION, 604_800); // 7 days of 1-second blocks
let _instance: Option<CouncilInstance> = None;
```
