> 🌐 **English** | [简体中文](audit-detection-latency.zh-CN.md)

# Audit detection latency

M6's acceptance target (MVP plan §10): a provider that swaps the model or lowers the precision
is **detected and slashed within 1 hour**. This report gives the latency measured end to end
on a development chain and converts it to the live parameters with a simulation.
(m6-auditor-agent, task 8.1; design D10.)

## How a cheat is found

1. Each round, 2 auditors are drawn per provider (`pallet-audit`). Each auditor agent
   (`ac-auditor run`) sends one request pinned to the provider at a random block of the first
   three quarters of the round. It re-checks the answer against the TOPLOC proofs and submits
   its verdict a few blocks later.
2. A request sent after the provider started cheating fails its re-check. The **second failing
   verdict from a distinct auditor**, in the same or the next round, opens a dispute.
3. Three reviewers (five on live chains) fetch the accusers' evidence from their endpoints and
   re-check it. Once a quorum (2 of 3; 3 of 5 live) confirms, the provider is slashed and jailed
   in the same block.

The worst case: the provider starts cheating right after both of a round's requests, and both
requests of the next round come at the end of their window. Then

    worst = round + 0.75 × round + submit + review

## Measured on the development chain

Six agents audit two providers on a dev chain (1-second blocks, 20-block rounds, 3 reviewers,
quorum 2, 20-block vote deadline). After a full round of honest service, one provider's engine
switches to another model in the middle of a round (`tests/e2e/tests/auditor_agent.rs`; the CI
job prints the numbers in its summary).

The measured blocks (switch → first failing verdict → dispute → jail) are added here from the first CI run of this test.

## Converted to live parameters

`scripts/sim-audit-latency.py` simulates 100,000 switches. The switch falls at a uniform block
of a round; requests come at uniform blocks of the first 75% of a round; the verdict lands 6
blocks after its request; reviewers take `review` blocks. All times assume one-second blocks.

| Round | Review | Mean | p95 | Max (simulated) | Worst (analytic) |
|---|---|---|---|---|---|
| 1,800 blocks (30 min) | 2 min | 23.2 min | 38.7 min | 53.2 min | 54.6 min |
| 1,800 blocks (30 min) | whole vote deadline (10 min) | 31.2 min | 46.7 min | 61.2 min | **62.6 min** |
| 1,200 blocks (20 min) | 2 min | 16.1 min | 26.4 min | 35.8 min | 37.1 min |
| 1,200 blocks (20 min) | whole vote deadline (10 min) | 24.1 min | 34.4 min | 43.8 min | **45.1 min** |

Reproduce with `scripts/sim-audit-latency.py` and `scripts/sim-audit-latency.py --review 600`
(seed 1).

## Conclusion

- With 30-minute rounds the 1-hour target holds only if reviewers are quick: a vote that uses
  its whole 600-block deadline makes the worst case 62.6 minutes. With **20-minute rounds** the
  worst case is 45.1 minutes even then, so the live round length is 1,200 blocks
  (m6-auditor-agent design D10). The mean detection time is about 16 minutes with a quick
  review.
- Each provider is audited 6 times an hour (MVP plan §1 asks for at least once).
- On the dev chain, the measured jail latency stays within the analytic worst case of its
  parameters (20 + 15 + submit + review blocks) and within the test's bound of three rounds
  after the switch.

## Limits

- The simulation assumes a cheat that never stops. An intermittent cheater is caught with the
  probability that two audited requests fall into its cheating periods; that probability is not
  modeled here.
- The review time on live chains depends on the reviewers' re-check engines (a real vLLM
  re-check takes seconds to minutes) and their availability. The table brackets it between
  2 minutes and the whole deadline.
- The dev chain uses mock engines. Real re-check accuracy is the calibration's subject
  ([calibration report](toploc-calibration.md); GPUs: I-012).
- Assignment follows the commit–reveal randomness, with its known bias (I-004 class). Pinned
  requests and transparent payment accounts can hint at an audit (I-016).
