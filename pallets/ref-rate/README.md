> 🌐 **English** | [简体中文](README.zh-CN.md)

# pallet-ref-rate

The ATC/USD reference rate of the inference market (decisions D23, D29; plan §5.7). Prices are
set in US dollars and settled in ATC at this rate.

## Units

- The rate (`AtcPerUsd`) is the number of smallest ATC units per US dollar: 1 ATC = 10^18 units,
  so "1 ATC = 2 USD" is `5 × 10^17`.
- Dollar amounts (`MicroUsd`) are integers of micro-dollars (10^-6 USD).
- Conversions state their rounding: **payments round down** (the payer is never over-charged),
  **thresholds round up** (a minimum stake is never lowered by rounding). The intermediate
  product is 256-bit; only a result above `u128::MAX` fails.

## Calls

| Call | Origin | Effect |
|---|---|---|
| `set_rate(rate)` | PoA administration | Sets the rate. The rate must be positive. Once a rate is set, the new one stays within ±20% of it (`0.8 × old ≤ new ≤ 1.2 × old`, bounds included) and comes at least `min_interval` blocks after the previous change. The first setting only needs a positive rate. Emits `RateSet`. |

Without a rate, every dollar conversion fails, and so does everything that depends on one
(registering a provider or gateway, redeeming a voucher).

## Genesis

`RefRate.initial` (optional initial rate) and `RefRate.params.minInterval` (86,400 blocks, one
day, on live chains). A zero interval or a zero initial rate makes genesis fail. The `dev` and
`local` presets start at 1 ATC = 1 USD with a 10-block interval; live chains start unset until
the administration sets the rate.

## Other pallets

Market pallets read the rate only through `ac_primitives::market::traits::PriceSource`, which
this pallet implements, so an oracle can replace it later (full plan §11) without changing
their rules. M8 moves `set_rate` to token governance and registers the ±20% bound with the
guardrails.

## Features

| Feature | Default | Purpose |
|---|---|---|
| `std` | yes | Native builds and tests. Disable it for the WASM runtime. |
| `runtime-benchmarks` | no | Benchmark of `set_rate`. |
| `try-runtime` | no | SDK try-runtime support. |

## Example

```rust
use ac_primitives::market::usd::{to_atc_payment, to_atc_threshold};
use ac_primitives::market::{AtcPerUsd, MicroUsd};
use pallet_ref_rate::within_band;

// 1 ATC = 2 USD.
let rate = AtcPerUsd(500_000_000_000_000_000);
// A $100 stake threshold is 50 ATC.
let hundred = MicroUsd::from_usd(100).unwrap();
assert_eq!(to_atc_threshold(hundred, rate), Ok(50_000_000_000_000_000_000));
// One micro-dollar at 3 units per dollar: a payment takes 0 units, a threshold needs 1.
assert_eq!(to_atc_payment(MicroUsd(1), AtcPerUsd(3)), Ok(0));
assert_eq!(to_atc_threshold(MicroUsd(1), AtcPerUsd(3)), Ok(1));
// The next change may move the rate by at most ±20%.
assert!(within_band(rate, AtcPerUsd(600_000_000_000_000_000)));
assert!(!within_band(rate, AtcPerUsd(600_000_000_000_000_001)));
```
