> 🌐 **English** | [简体中文](README.zh-CN.md)

# pallet-gateways

On-chain registration of inference gateways (plan §7). A gateway serves the OpenAI-compatible
API, checks vouchers, routes requests to providers and settles batched receipts; users escrow
credits with the gateway they choose (`pallet-credits`).

## The gateway record

| Field | Meaning |
|---|---|
| `endpoint` | Where users connect (UTF-8, up to 256 bytes; reachability is not checked). |
| `fee_bps` | Gateway fee in basis points, at most **500 (5%)** (plan §8; M8 registers the cap with the guardrails). |
| `stake`, `unlocking` | Bonded stake and up to 8 unbonding chunks, held with the `Stake` hold reason. |
| `status` | `Active` or `Exiting`. |

The stake threshold is **$1,000** on live chains (a draft value), converted at the reference rate
(`pallet-ref-rate`) rounding up; without a rate, registering and unbonding fail.

## Calls

| Call | Effect |
|---|---|
| `register(endpoint, fee_bps, stake)` | Registers the caller and bonds `stake` (at or above the threshold). |
| `update(endpoint?, fee_bps?)` | Changes the endpoint and/or the fee (still capped); not while exiting. |
| `bond_extra(amount)` | Adds stake (not while exiting). |
| `unbond(amount)` | Moves `amount` to unbonding; what stays bonded must meet the threshold. |
| `exit()` | Stops accepting new escrow; the whole stake starts unbonding. |
| `withdraw_unbonded()` | Releases unbonding chunks that are due (7 days on live chains); an exited gateway with nothing left is removed. |

## Other pallets

`GatewayLookup` (`is_active`, `fee_bps`) lets `pallet-credits` refuse escrow for gateways that
are not active and lets settlement (the next M5 change) pay the gateway fee.

## Features

| Feature | Default | Purpose |
|---|---|---|
| `std` | yes | Native builds and tests. Disable it for the WASM runtime. |
| `runtime-benchmarks` | no | Benchmarks of every call (`Config::BenchmarkHelper` sets a rate). |
| `try-runtime` | no | SDK try-runtime support. |

## Example

```rust
use ac_primitives::market::records::MAX_GATEWAY_FEE_BPS;
use ac_primitives::market::usd::to_atc_threshold;
use ac_primitives::market::{AtcPerUsd, MicroUsd};
use pallet_gateways::GatewayParams;

let live = GatewayParams::LIVE;
assert_eq!(live.max_fee_bps, MAX_GATEWAY_FEE_BPS); // 5%
// At 1 ATC = 1 USD a gateway bonds at least 1,000 ATC.
let rate = AtcPerUsd(1_000_000_000_000_000_000);
assert_eq!(live.min_usd, MicroUsd::from_usd(1_000).unwrap());
assert_eq!(to_atc_threshold(live.min_usd, rate), Ok(1_000 * 1_000_000_000_000_000_000));
```
