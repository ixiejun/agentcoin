> 🌐 **English** | [简体中文](README.zh-CN.md)

# pallet-audit

On-chain audits of the inference market (MVP plan §5.5; OpenSpec change `m6-audit-chain`; spec
`market/audit`). Auditors re-check inferences they requested themselves (with `ac-auditor`, off
chain); this pallet decides who audits whom, records verdicts, settles disputes, punishes and
pays. Prompts, answers and proofs never go on chain (red line 6): a verdict carries only the
commitment to its evidence and, when judged by the thresholds, a few integer statistics.

## How it works

1. **Auditors** register with a stake in dollars (converted at the reference rate, rounding up).
   Providers and gateways cannot register. Stake unbonds over a fixed period and stays slashable
   until then.
2. **Rounds.** The first block of each round stores the roster (auditors at or above the
   threshold, not exiting, in account order) and a seed from the chain's commit-reveal
   randomness. The auditors of a provider in a round are `sample(roster, seed, provider)` from
   `ac_primitives::market::audit`; anyone can recompute them (`AuditApi::assignment`).
3. **Verdicts.** An assigned auditor submits one verdict per provider and round: pass, fail
   (reason and evidence commitment) or inconclusive (reason), with the thresholds version and a
   receipt signed by provider and gateway. A verdict judged by the thresholds (a pass, or a
   chunk out of bounds) also carries an evidence commitment and its statistics (`AuditStats`
   under the accepted parameter version; OpenSpec change `m6-audit-sprt`); other verdicts carry
   none. The chain checks the receipt (this chain's genesis,
   the audited provider, the registered keys, both ML-DSA signatures, the fee at the provider's
   current price) and that its request ID was never used.
4. **Disputes.** Failing verdicts of two different auditors on one provider in this round or the
   previous one open a dispute: `N` reviewers are drawn (excluding the accusers, the provider and
   the receipts' gateways), re-check the evidence off chain and vote. `Q` votes on one side
   decide at once:
   - **confirmed**: the provider's stake is slashed and burned, and it is jailed through
     `ProviderPenalty` (its unsettled work is voided by `pallet-work`);
   - **rejected**: every accuser's stake is slashed and burned and the accuser is made to exit;
   - after the deadline anyone closes an undecided dispute; nobody is punished.
5. **Statistical judgment.** Each verdict's statistics add a log-likelihood ratio to the
   provider's CUSUM (`SprtState`, rules in `ac_primitives::market::audit::stats`). A state at the
   bound, with the judgment enabled and no dispute open, opens a **statistical dispute** on the
   state's verdicts (reviewers exclude their auditors, the provider and the receipts' gateways)
   and resets the state. Reviewers recompute the statistics from the evidence and replay the
   state. Confirmed: slashed and jailed as above. Rejected: nobody is slashed (the dispute is the
   chain's conclusion, not an auditor's accusation). Disabled, the states are computed and
   queryable but open nothing. Verdicts a state or an open dispute still refers to outlive their
   retention (`RetainedVerdicts`) and are removed once nothing does.
6. **Payments.** Each accepted verdict and each vote on the winning side is paid a fixed dollar
   amount (rounded down) from the audit pot (`PalletId(*b"ac/audit")`), which the treasury floor
   funds with "audit" spends. An empty pot skips the payment; nothing is ever minted.

## Parameters

| Parameter | Live draft | Test presets | Guardrail | Changed by |
|---|---|---|---|---|
| Round length | 1,200 blocks (20 minutes) | 20 | > 0 | genesis |
| Auditors per provider and round | 2 | 2 | 1–8 | genesis |
| Reviewers / quorum | 5 / 3 | 3 / 2 | 1 ≤ Q ≤ N ≤ 15, 2Q > N | genesis |
| Voting period | 600 blocks | 20 | > 0 | genesis |
| Provider / auditor slash | 10% / 10% | 10% / 10% | 1–100% | genesis |
| Auditor unbonding | 604,800 blocks | 20 | > 0 | genesis |
| Auditor stake | $1,000 | $1,000 | $100–$100,000 | administration |
| Payment per verdict or vote | $0.05 | $0.05 | $0–$1 | administration |
| Accepted thresholds version | `AUDIT_THRESHOLDS` (4: calibrated single-audit bounds) | same | only increases | administration |
| Statistical parameter version | latest built in (`CURRENT_STATS`) | same | built in, only increases | administration (`set_stats_config`) |
| Statistical judgment enabled | no (until calibration covers the main GPU classes) | yes | — | administration (`set_stats_config`) |

Changing the statistical parameter version empties every provider's state: states of the older
version count as empty at once and are removed over the next blocks, two per block.

The administration cannot slash, jail, open or decide anything (D6).

## Calls

`register`, `bond_extra`, `unbond`, `exit`, `withdraw_unbonded`, `submit_verdict`, `vote`,
`close_dispute`, `set_params` (administration), `set_endpoint` (an auditor's evidence endpoint and
X-Wing key, which reviewers reach; m6-auditor-agent), `set_stats_config` (administration; the
statistical judgment's version and switch). The wallet wraps them as `ac-wallet audit …`.
`AuditApi` version 2 adds the endpoint query and a paged list of open disputes; version 3 returns
verdicts with their statistics and disputes with their kind, and adds `sprt_state` and
`stats_config`.

## Storage version

Version 1 (`migrations::v1::MigrateToV1`, run by the runtime's `Migrations`): stored verdicts get
`stats: None`, disputes `kind: Fail`, and the statistical judgment starts at the latest built-in
version, disabled.

## Example

The guardrails of the genesis parameters:

```rust
use ac_primitives::market::audit::AuditParams;
use pallet_audit::AuditGenesis;

let (fixed, adjustable) = AuditGenesis::LIVE.split();
assert_eq!(fixed, AuditParams::LIVE);
assert!(fixed.check().is_ok());
assert!(adjustable.within_bounds());
let broken = AuditParams { quorum: 2, ..AuditParams::LIVE }; // 2 of 5 is no majority
assert!(broken.check().is_err());
```

## Features

`std` (default), `runtime-benchmarks`, `try-runtime`.
