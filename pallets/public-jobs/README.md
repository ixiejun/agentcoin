> 🌐 **English** | [简体中文](README.zh-CN.md)

# pallet-public-jobs

The public job queue of AgentCoin (MVP plan §5.6; OpenSpec change `m6-public-jobs`; spec
`market/public-jobs`). Zero-stake workers run evaluation, data cleaning and embedding units
three at a time with `ac-worker`, off chain; this pallet draws the workers, takes their
commitments and reveals, decides which summary is the unit's result, pays public work, and
punishes a majority that a canary catches. Data and full results never go on chain: only small
summaries and result hashes do.

## How it works

1. **Workers** register with no stake and up to 8 models they run (none for data cleaning
   only), and declare themselves ready once per round (`ready`; `ac-worker run` does it). The
   first block of each round freezes the roster: the workers ready in the previous round, not
   suspended, with room for more pending work, and a seed from the chain's commit-reveal
   randomness (subject `agentcoin/public-round`).
2. **Jobs** are published by the administration (a PoA motion until M8; `ac-wallet public
   publish` prints the call): kind, model, comparison rules version 1, the data manifest's
   BLAKE3 hash and URL, the results URL, 1–100,000 units, a dollar price per unit and passing
   worker (at most the price cap), and optionally a canary Merkle root. At most 16 jobs are in
   progress; `cancel` stops opening new units.
3. **Units.** Each round opens up to `units_per_round` units, oldest job first; a job whose unit
   cannot be staffed waits without holding up the others. Three eligible workers are drawn for
   each with the round's seed (domain `agentcoin 2026-10 public-assign v1`); a worker never gets
   the same unit twice across attempts.
4. **Commit, then reveal.** Until `commit_by` each worker commits to
   `derive("agentcoin 2026-10 public-commit v1", job, unit, attempt, worker, summary,
   result_hash, salt)`; after it, until `reveal_by`, it reveals the summary, the result hash and
   the salt. The unit settles at the third reveal or, after the deadline, with `close` (free for
   its workers).
5. **Settlement (rules v1).** Two summaries agree when, for evaluation (answer indices), at most
   max(1, 2%) answers differ; for embedding (32-bit fingerprints per text), at most max(1, 2%)
   texts are farther apart than Hamming distance 3; for data cleaning (the result's BLAKE3), they
   are equal. The reference is the summary that agrees with most others (ties to the earlier
   slot); the unit passes when at least two agree with it. The majority earns the unit's price
   as public work; the others and those who did not reveal miss. A unit without a majority is
   reopened to fresh workers, at most three attempts. Three consecutive misses suspend a worker.
6. **Canaries.** For a canary unit the publisher knows the expected summary. Once the unit
   passed, anyone reveals its leaf `derive("agentcoin 2026-10 public-canary v1", 0x00 ‖ job ‖
   unit ‖ summary ‖ salt)` with a Merkle proof (`reveal_canary`; `ac-worker collect` does it).
   If the reference disagrees with it, the unit fails, every majority worker loses its locked
   rewards (burned) and all its unclaimed work (the settled share is burned from the payout
   account) and is suspended, and a minority worker that agreed with the canary earns the unit.
7. **Rewards.** Public work counts after `challenge_epochs` emission epochs; emission then mints
   the epoch's public emission into the payout account (`PalletId(*b"ac/publc")`, through
   `PublicPayout`). Workers `claim` their share of settled epochs; it moves to their account
   under the hold `Locked` until `lock_blocks` later (rounded up to a sixteenth of the lock, at
   most 32 segments), slashable until then, and `withdraw` releases it.

Worker calls for assigned work (`ready`, `commit`, `reveal`, `close`, `claim`, `withdraw`) are
refunded when they succeed: a worker needs a little balance for the fee taken up front.

## Parameters

| Parameter | Live draft | Test presets | Guardrail | Changed by |
|---|---|---|---|---|
| Round length | 600 blocks | 10 | > 0 | genesis |
| Units opened per round | 64 | 8 | 1–256 | genesis |
| Commit period | 1,800 blocks | 20 | > 0 | genesis |
| Reveal period | 300 blocks | 10 | > 0 | genesis |
| Challenge period | 2 epochs | 1 | ≥ 1 | genesis |
| Reward lock | 604,800 blocks | 30 | > 0 | genesis |
| Suspension | 86,400 blocks | 30 | > 0 | genesis |
| Unit record retention | 604,800 blocks | 200 | > 0 | genesis |
| Unit price cap | $1 | $1 | $0–$10 | administration (`set_price_cap`) |

## Calls

`register`, `set_models`, `deregister`, `ready`, `publish` and `cancel` (administration),
`commit`, `reveal`, `close`, `reveal_canary`, `claim` (up to 16 epochs), `withdraw`,
`set_price_cap` (administration). The wallet wraps them as `ac-wallet public …`; `PublicJobsApi`
answers the round, roster, workers, jobs, units, assignments, epochs, pending work, locked
rewards, the payout balance and the parameters.

## Example

The guardrails of the genesis parameters:

```rust
use ac_primitives::market::public::{ParamsError, PublicParams};

assert!(PublicParams::LIVE.check().is_ok());
assert!(PublicParams::DEV.check().is_ok());
let broken = PublicParams { units_per_round: 0, ..PublicParams::LIVE };
assert_eq!(broken.check(), Err(ParamsError::UnitsPerRound));
```

## Features

`std` (default), `runtime-benchmarks`, `try-runtime`.
