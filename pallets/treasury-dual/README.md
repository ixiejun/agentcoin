> 🌐 **English** | [简体中文](README.zh-CN.md)

# pallet-treasury-dual

The AgentCoin treasury (plan §5.1; decisions D12, D18, D42). It holds only what emission
settlement pays into it and never mints or burns: spending is a plain transfer.

## Accounts

Three keyless accounts derived from published `PalletId`s (`"modl" ‖ id`, zero-padded, no
hashing), so anyone can compute them and nobody holds their keys:

| Account | `PalletId` | Receives | Spent by |
|---|---|---|---|
| Community grants | `ac/trcom` | 40% of the proportional treasury share (genesis `community_share`, basis points) | the administration, `spend` |
| Holder treasury | `ac/trhld` | the rest of the proportional share, including the rounding remainder | **nobody until M8** |
| Floor | `ac/trflr` | the top-up to 5% of the scheduled amount | the administration, `spend_floor`, vested part only |

The proportional share is `(market + public) × 20 / 70` of each epoch's work emission; the
treasury takes the larger of it and 5% × S, never the sum (red line 5).

## Floor vesting

Floor top-ups are grouped into batches of 30 days (2,592,000 blocks). Each batch vests linearly
over 2 years (63,115,200 blocks) starting at the **end** of its batch, so nothing becomes
spendable earlier than two years after it arrived. The spendable amount is
`Σ vested(batch) − spent`. At most 26 batches are still vesting at any time; fully vested
batches are folded into a single matured total.

`spend_floor` names its purpose, `Audit` or `ColdStart`; the purpose is recorded in the `Spent`
event.

## Holder-treasury lock

Until on-chain holder voting exists (M8) the holder treasury only receives funds. Three layers
guarantee it:

1. its account has no private key (`PalletId` derivation);
2. this pallet has no call that spends from it;
3. the runtime's `HolderTreasuryLock` call filter refuses forced transfers, forced balance
   changes and raw storage writes that touch it — both as the base call filter and inside
   `PoaAdmin::dispatch_as_root`, since Root bypasses the base filter.

A runtime upgrade can change any rule; upgrades are public motions of the administration and
stay bound by the node invariants.

## Administration

`spend` and `spend_floor` require `Config::AdminOrigin`: in the runtime, a PoA-council motion
with at least the threshold of members, directly or through `PoaAdmin::dispatch_as_root`.
Signed accounts are refused.

## Queries

Runtime API `TreasuryApi`: `community`, `holder`, `floor` (account and balance) and
`floor_spendable`.

## Features

| Feature | Default | Purpose |
|---|---|---|
| `std` | yes | Native builds and tests. Disable it for the WASM runtime. |
| `runtime-benchmarks` | no | Benchmarks of `spend` and `spend_floor`. |
| `try-runtime` | no | SDK try-runtime support. |

## Example

Half of a floor batch is spendable one year after the batch closed:

```rust
use ac_primitives::emission::{FLOOR_VESTING_BLOCKS, split_treasury, vested};
use pallet_treasury_dual::{DEFAULT_COMMUNITY_SHARE, HOLDER_PALLET_ID};

assert_eq!(&HOLDER_PALLET_ID.0, b"ac/trhld");
assert_eq!(split_treasury(1_000, u128::from(DEFAULT_COMMUNITY_SHARE)), (400, 600));
let batch_end = 2_592_000;
assert_eq!(vested(1_000, batch_end, batch_end + FLOOR_VESTING_BLOCKS / 2), 500);
assert_eq!(vested(1_000, batch_end, batch_end + FLOOR_VESTING_BLOCKS), 1_000);
```
