> 🌐 **English** | [简体中文](README.zh-CN.md)

# pallet-work

Settlement of paid inference (plan §5.4, refine / accumulate; decision D2: rewards only for work
that was used, paid for and verified). Providers and gateways sign one receipt per request
(`ac_primitives::market::receipt`); receipts stay off chain with the gateway, and prompts and
outputs never reach them. The chain sees work reports and moves the money.

## Work reports

`submit_report(root, receipt_count, entries, vouchers)` by a registered gateway (active or
exiting):

| Part | Rule |
|---|---|
| `root`, `receipt_count` | Merkle root of the receipts (`ac_primitives::market::receipt_tree`); at least one receipt per entry. |
| `entries` (1–128) | `(job kind, provider, model, dollars, input tokens, output tokens)`; inference only; no (provider, model) twice; the provider is registered, not jailed and lists the model. |
| `vouchers` (1–16) | Redeemed through the `Credit` interface (D20) into the **gateway's account**; one per channel. |

The entries' dollars must **equal** the dollars the vouchers redeem: a gateway allocates the
users' money to providers, it cannot keep any beyond its fee. One invalid voucher rejects the
whole report. A reference rate must be set.

## Allocation

`G` = redeemed + shortfall (what the escrow could not cover; the gateway pays it from its own
free balance, so a gateway must keep free balance above the existential deposit). All integer,
rounded down:

```text
burn     = G × b                  (b = 20%, burned through Emission)
fee      = G × gateway fee        (at most 5%)
pool     = G − burn − fee
share_i  = pool × usd_i / Σ usd   (rounding remainder burned too)
work_i   = (G × usd_i / Σ usd) × k   (k = 0.5; k − b ≤ 0.5, research 03 §1.5)
```

Shares and fee stay **held on the gateway's account** (hold reason `Pending`) until the challenge
period ends; they are recorded per (recipient, maturity epoch, gateway).

## Challenge period and claims

A report submitted in emission epoch `s` matures in `s + C` (`C` = 2 epochs). Its work is that
epoch's verified market work: `Emission` mints `min(50% × avail, work)` to this pallet's pot and
reports it (`MarketPayout::settled`). Then anyone can `claim(who, [(epoch, gateway), …])` (up to
64 items): shares move from the gateway to `who`, a gateway's own fee is released, and `who`'s
share of the epoch's emission, `market × work / Σ work`, is paid from the pot. A claim fails as a
whole if `who`'s free balance plus the payment stays below the existential deposit. The pot keeps
one existential deposit forever (taken from the first emission it receives), so claims never
empty it.

No call disputes or voids a report in M5. The audit (M6) jails cheating providers through
`pallet-providers`; a provider jailed before its work is settled loses that work at once (the
epoch's verified work shrinks) and its held shares are burned when claimed. Fees of a gateway are
never voided with a provider.

## Parameters

| Parameter | Live | dev / local |
|---|---|---|
| `burn_bps` (`b`) | 2,000 | 2,000 |
| `emission_bps` (`k`) | 5,000 | 5,000 |
| `challenge_epochs` (`C`) | 2 | 2 |
| `retention_epochs` | 720 (30 days) | 20 |

Genesis rejects `b` outside 1,000–9,000, `k − b` above 5,000 and a zero challenge period or
retention. Reports are pruned (at most four per submission) `retention_epochs` after they mature;
pending payments outlive them.

## Features

| Feature | Default | Purpose |
|---|---|---|
| `std` | yes | Native builds and tests. Disable it for the WASM runtime. |
| `runtime-benchmarks` | no | Benchmarks of both calls at their bounds (16 ML-DSA-87 vouchers, 128 entries; 64 claim items). |
| `try-runtime` | no | SDK try-runtime support. |

## Example

```rust
use ac_primitives::market::MicroUsd;
use ac_primitives::market::work::allocate;

// One dollar settled (10^6 units at 10^6 units per dollar), b = 20%, a 5% gateway fee, k = 0.5,
// two providers who served $0.75 and $0.25.
let a = allocate(1_000_000, 2_000, 500, 5_000, &[MicroUsd(750_000), MicroUsd(250_000)]).unwrap();
assert_eq!(a.burn, 200_000);
assert_eq!(a.gateway_fee, 50_000);
assert_eq!(a.shares, [562_500, 187_500]);
assert_eq!(a.work, [375_000, 125_000]);
```
