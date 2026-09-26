> 🌐 **English** | [简体中文](README.zh-CN.md)

# ac-consensus-bft

AC-BFT, AgentCoin's post-quantum finality gadget (plan §4.1). Validators vote on the blocks that
Aura-PQ produces in two phases, sign every message with their ML-DSA-65 key (context
`agentcoin/bft-vote/v1`) and finalize a block once commit votes worth more than two thirds of the
authority set agree. The resulting finality proof can be checked by anyone with
`ac_primitives::ac_bft::verify_finality_proof`.

## Protocol

With `n` members of total weight `W`, a certificate needs weight `q = ⌊2W/3⌋ + 1`; the set
tolerates faulty weight up to `W − q` (one faulty member out of four).

1. The leader of round `r` (member `r mod n`) proposes its best block, capped at the first
   unfinalized authority-set change, extending the highest prepare certificate it has seen.
2. Members prepare-vote a proposal that extends the last finalized block and passes the
   **locking rule**: no lock, or the target extends the locked block, or the proposal is
   justified by a prepare certificate of a higher round (from an earlier round, whose votes the
   member has seen) that the target extends.
3. On a prepare certificate for the current round, members commit-vote, lock on it and move to
   the next round at once, so the next prepare phase runs alongside this commit phase.
4. A commit certificate finalizes its target and all its ancestors.

Rounds without a prepare certificate time out after two slots, backing off by ×1.5 up to 30 s.
`q` timeouts, or messages of a higher round from members worth more than `W − q`, move a member
forward; members relay the certificate that moved them on to peers still in an earlier round, and
their highest certificate to peers whose timeouts show a lower one. Before every signature the
member durably records what it signs, so a restarted member never signs two different messages of
one kind in one round. Conflicting messages from one member are reported as double-signing
evidence.

The safety argument is in the rustdoc of `protocol`; the deterministic simulator in `sim`
checks safety and liveness over 1,000 random networks with delays, losses, reordering and up to
`f` Byzantine members (silent or equivocating), plus property tests.

## Modules

| Module | Contents |
|---|---|
| `protocol` | The pure state machine `Voter`: events in, actions (`Persist`, `Broadcast`, `Finalize`, `Report`, `Relay`) out. Uses no node-client (`sc-*`) crate — checked by `scripts/check-license-boundary.sh` — so it can be reused by light clients and formal tools. |
| `sim` | Deterministic network simulator and in-memory block tree (tests and the `test-utils` feature only). |
| `network` | `MessageFilter`: validation, de-duplication, round window and next-set buffering of messages on the notification protocol `/<genesis hash hex>/acbft/1`. |
| `tracker` | `TrackerState`: authority-set changes pending on every fork and the set that must finalize a block; persisted in auxiliary storage. |
| `import` | `AcBftBlockImport`: verifies finality proofs arriving with blocks or on their own, finalizes and stores valid ones, and requests proofs for set-change blocks. |
| `chain` | The client's block tree as the `Chain` seen by the state machine. |
| `gadget` | The node task: drives `Voter` with block imports, finality, the network and a timer; persists before signing; exports the `acbft_*` metrics. |

## Features

| Feature | Default | Purpose |
|---|---|---|
| `test-utils` | no | Exposes `sim` to other crates' tests and to this README's example. |

## Example

Four honest members on a calm network finalize every block:

```rust
# #[cfg(feature = "test-utils")] {
use ac_consensus_bft::sim::{Scenario, run};

let scenario = Scenario {
    n: 4,
    byzantine: Vec::new(),
    slot_ms: 1000,
    gst_ms: 0,
    chaos_delay_ms: 0,
    drop_percent: 0,
    calm_delay_ms: 50,
    duration_ms: 20_500,
};
let outcome = run(&scenario, 7);
outcome.assert_safe();
for finalized in outcome.finalized.values() {
    assert!(finalized.last().map_or(0, |b| b.number) >= 19);
}
# }
```

Run the full simulation suite with `cargo test -p ac-consensus-bft`; set `AC_BFT_SIM_SEEDS` to
change the number of seeds per network size (default 334).
