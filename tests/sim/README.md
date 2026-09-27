> 🌐 **English** | [简体中文](README.zh-CN.md)

# ac-sim

Economic simulation of AgentCoin. It replays emission settlement
(`ac_primitives::emission::settle`, the same code the runtime runs) over many epochs and
compares every epoch with an independent restatement of the formula of the MVP plan §5.1.

- `tests/eight_years.rs`: two four-year periods at the live epoch length (3,600 blocks),
  without work, at full load and with deterministic random work. Every epoch matches the
  reference exactly, cumulative minting never exceeds the cumulative schedule, and the
  schedule stays below 21,000,000 ATC.

```rust
use ac_primitives::emission::{EmissionSchedule, Phase};

let schedule = EmissionSchedule::new(3_600)?;
let totals = ac_sim::run(&schedule, 10, Phase::Poa, |_| (0, 0), |_, _, _| {});
assert_eq!(totals.minted + totals.reserve, totals.scheduled);
# Ok::<(), ac_primitives::emission::EmissionError>(())
```

Run with `cargo test -p ac-sim`.
