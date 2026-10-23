> 🌐 **English** | [简体中文](README.zh-CN.md)

# pallet-providers

On-chain registration of inference providers (plan §5.3). Gateways read the serviceable
providers of a model from here and route requests to them.

## The provider record

| Field | Meaning |
|---|---|
| `tier` | `T1` (data center) or `T2` (consumer). `T0` (TEE) is reserved and refused. |
| `endpoint` | Where gateways connect (UTF-8, up to 256 bytes; reachability is not checked). |
| `kem_pk` | AlgId-tagged X-Wing (ML-KEM-768 + X25519) key gateways encrypt requests to. Other KEM algorithms are refused. |
| `models` | 1–16 registered models, no duplicates, each with a positive price per million input and output tokens in micro-dollars. |
| `stake`, `unlocking` | Bonded stake and up to 8 unbonding chunks, all held with the `Stake` hold reason. |
| `status` | `Active`, `Exiting` or `Jailed`. |
| `last_heartbeat` | Block of the latest heartbeat. |
| `metrics`, `attestation` | Reserved: audit metrics (M6) and TEE attestation (T0). |

## Stake in dollars

The stake threshold is set in US dollars — **T1 $1,000, T2 $100** on live chains (draft values)
— and converted at the reference rate (`pallet-ref-rate`) rounding up.

A provider's **required stake** is the larger of its tier's threshold and its **deterrent**,
`κ` times its recent revenue per epoch (`Config::Revenue`, work settlement's average over the
last 24 epochs). `κ = (g × n × H − C) / f` epochs (m6-audit-hardening design D3,
m6-multi-model-deterrence) is computed from these values:

- `g`, the assumed gain of a cheat as a share of its revenue (50%);
- `H`, the exposure horizon in epochs of a provider of one calibrated model (11, from the
  statistical judgment's simulation);
- `n`, how many of the provider's models have an audit profile (`Config::Calibration`; at
  least 1). Auditors pick one of them at random for each audit, so each is found about `n`
  times later; the simulation shows `n × H` covers it for `n` = 1, 2, 4 and 16. This assumes
  auditors can re-check every calibrated model a provider lists;
- `C`, the challenge period (`Config::ChallengeEpochs`, 2);
- `f`, the share of the stake a confirmed dispute slashes (`Config::ConfirmedSlash`, 10%).

On live chains `κ` is 35 epochs for one calibrated model, so a provider settling $100 per epoch
needs $3,500; with two it is 90 epochs and $9,000. With this stake a confirmed cheat loses at
least what it gained before being found. `g` and `H` are set at genesis (`deterrence`,
guardrails 10%–100% and 1–720 epochs). `kappa_bps_for(n)`, `calibrated_count(models)`,
`required_usd(who, tier, models)` and `required_stake(who, tier, models)` expose the computation.

Registering and unbonding check the required stake; without a rate both fail. A new model list
(`update`) that raises the required stake above the stake is refused (`BelowThreshold`); one that
does not raise it passes even below it. When the rate, the revenue or a model's calibration
moves, a provider whose stake falls below its required stake stays registered but is not
serviceable until it bonds more.

## Serviceable

A provider is **serviceable** when it is `Active`, its stake meets its required stake and at
most two heartbeat intervals (600 blocks each on live chains) have passed since its last
heartbeat. `serviceable_providers(model, start_after, limit)` pages through the serviceable
providers of a model in account order (the runtime exposes it through `MarketApi`).

## Calls

| Call | Effect |
|---|---|
| `register(registration)` | Registers the caller with a `Registration { tier, endpoint, kem_pk, models, stake, attestation }` and bonds `stake`; `attestation` must be `None`. |
| `update(endpoint?, kem_pk?, models?)` | Changes any of them (same rules as registration); not while exiting. The tier cannot change. A model list that raises the required stake above the stake fails. |
| `heartbeat()` | Records a heartbeat. **Free** when at least half an interval passed since the last one; paid otherwise. |
| `bond_extra(amount)` | Adds stake (not while exiting). |
| `unbond(amount)` | Moves `amount` to unbonding; what stays bonded must meet the required stake. |
| `exit()` | Stops serving: the whole stake starts unbonding and the provider leaves every model's list. |
| `withdraw_unbonded()` | Releases unbonding chunks that are due (7 days on live chains), except while an audit dispute on the provider is open (`Config::DisputeLock`, error `DisputeOpen`). An exited provider with nothing left is removed and may register again. |

When the unbonding list is full (8 chunks), a new chunk merges into the last one.

## Penalties (audits)

`ProviderPenalty::slash(who, ratio)` takes `ratio` of the whole stake — bonded first, then
unbonding chunks in unlock order — and burns it through `Config::Slash` (`Emission` in the
runtime, counted in `TotalBurned`). `ProviderPenalty::jail(who)` makes a provider permanently
unserviceable. Only `pallet-audit` calls them, when a dispute is confirmed (m6-audit-chain); no
transaction reaches either directly, not even the PoA administration (D6: no blacklists). A
jailed provider can still exit and withdraw what is left of its stake after unbonding.
`ProviderAudit` tells audits whether an account is a provider and its current price for a model.

## Features

| Feature | Default | Purpose |
|---|---|---|
| `std` | yes | Native builds and tests. Disable it for the WASM runtime. |
| `runtime-benchmarks` | no | Benchmarks of every call (`Config::BenchmarkHelper` registers models and sets a rate). |
| `try-runtime` | no | SDK try-runtime support. |

## Example

```rust
use ac_primitives::market::usd::to_atc_threshold;
use ac_primitives::market::{AtcPerUsd, MicroUsd};
use pallet_providers::ProviderParams;

let live = ProviderParams::LIVE;
// At 1 ATC = 2 USD a T2 provider bonds at least 50 ATC.
let rate = AtcPerUsd(500_000_000_000_000_000);
assert_eq!(live.t2_min_usd, MicroUsd::from_usd(100).unwrap());
assert_eq!(to_atc_threshold(live.t2_min_usd, rate), Ok(50_000_000_000_000_000_000));
// Two silent intervals (20 minutes) make a provider unserviceable.
assert_eq!(live.heartbeat_interval * 2, 1_200);
```
