> 🌐 **English** | [简体中文](toploc-calibration.zh-CN.md)

# TOPLOC re-check calibration

How the re-check thresholds `AUDIT_THRESHOLDS` (`crates/ac-market-proto/src/toploc.rs`) were
chosen and how well they separate honest providers from cheating ones
(m6-toploc-verify, tasks 8.1 and 8.2). The M6 target (MVP plan §10) is an honest
misjudgment rate below 0.1% while model swaps and lower precision are caught.

## Method

`scripts/calibrate-toploc.py` generates and re-checks samples with real vLLM on CPU
(workflow `toploc-calibration`, ten shards on GitHub runners, merged by
`calibrate-toploc.py --merge`):

- **Pinned versions** (`plugins/vllm/ci/pins.env`): vLLM 0.30.0 (CPU wheel), model
  `Qwen/Qwen2.5-0.5B-Instruct` at `7ae55760…`, bfloat16.
- **Honest**: the registered model in prove mode, re-checked by `ac-auditor recheck` against a
  verify-mode vLLM of the same model.
- **Four kinds of cheating**: another model of the same architecture (`Qwen/Qwen2.5-0.5B` at
  `060db649…`), int8 and int4 weights of the registered model, and a hidden system prompt.
- Each sample's outcome and every chunk's metrics (exponent mismatches, mean and median
  mantissa error), plus per-CPU counts of honest outcomes and of prefill chunks that are not
  bit-exact.

## Thresholds (version 2)

| Chunk | Exponent mismatches ≤ | Mean mantissa error ≤ | Median mantissa error ≤ |
|---|---|---|---|
| Prefill (chunk 0) | 2 | 0.50 | 1 |
| Decode (each later chunk) | 20 | 8.00 | 8 |

An inference passes only if every chunk passes. The prefill chunk is strict: the auditor
computes the prompt exactly the way the provider did, so on the same hardware class it is
bit-exact. Decode chunks are wider: the auditor recomputes them inside one prefill, a different
computation. The int8 cheat shows why the prefill bound must stay strict. Its decode chunks fall
inside the decode bounds (worst-chunk exponent mismatches 2–11, mean 1.6–4.6), so it is caught
mainly by the prefill chunk.

## Thresholds version 3 (provisional)

On GPUs the honest prefill error depends on the prompt's length and on the batch's shape, and
for short prompts it overlaps the int8 cheat's (`calibration-results/gpu-quick-2026-10-07`,
`gpu-exp-long-2026-10-07`). Version 3 (`m6-toploc-gpu-calibration` design D8) judges the prefill
chunk by the prompt's token count:

| Chunk | Exponent mismatches ≤ | Mean mantissa error ≤ | Median mantissa error ≤ |
|---|---|---|---|
| Prefill, prompt of 150–300 tokens (the audit length band, both ends included) | 6 | 0.85 | 1 |
| Prefill, any other prompt | 15 | 5.00 | 4 |
| Decode (each later chunk) | 20 | 8.00 | 8 |

Auditors send only prompts in the band (`ac-auditor run`). These values were provisional; the
GPU calibration (`calibration-results/gpu-cal-v3-2026-10-09`) showed that no bounds catch int8 in
a single audit without failing honest providers, and version 4 replaced them (next section).

## Thresholds version 4: single-audit scope

User decision (2026-10-09, `m6-toploc-gpu-calibration` design D13–D15): a single audit only
catches gross deviations (another model, int4, a changed prompt; a missing proof fails anyway).
int8, including int8 only while decoding, is left to the statistical judgment per provider
(OpenSpec change `m6-audit-sprt`).

| Chunk | Exponent mismatches ≤ | Mean mantissa error ≤ | Median mantissa error ≤ |
|---|---|---|---|
| Prefill, any prompt length | 20 | 6.00 | 5 |
| Decode (each later chunk) | 28 | 12.00 | 12 |

Basis, from the four GPU cells (15,880 honest samples judged by the thresholds): the honest
worst is prefill (11, 3.88, 3) and decode (19, 7.96, 4); int4's smallest prefill mean is 8.31, a
changed prompt's smallest prefill exponent mismatches 56, another model's decode chunks at least
(22, 13.17, 10). The band (150–300 tokens) stays part of the constant: audit prompts and the
statistical judgment use it. Version 3's values are kept only to replay the runs judged under it.

Honest answers whose text re-tokenizes to other token IDs than the generated ones (issue I-023)
fail a decode chunk under any bounds; the generation records it per case (`token_ids.json`) and
the merge lists them apart (`token_mismatch`), out of the cells and the conclusion. Their root
cause (the provider returning the output token IDs) is a separate change.

## Statistical judgment parameters

The statistical judgment (OpenSpec change `m6-audit-sprt`) takes its parameters from the
calibration. `scripts/export-audit-stats.py` reads the merged reports' statistics histograms
(inside the band only, since auditors send only prompts there) and derives one version:

- **Bins** of each statistic (prefill mean, decode means averaged, in hundredths), with a
  log-likelihood ratio per bin in thousandths of a nat, rounded down. The alternative is int8,
  pooled over the cells. The null is, bin by bin, the largest honest frequency of any cell, with
  pseudo-counts 5 (honest) and 0.5 (int8).
- **Clamp** −0.5 / +3.0 nats per verdict, **bound** 24.7 nats (52,560 audits a year, 10⁻⁶),
  **per-auditor cap** a third of the bound, at most 128 verdicts per state.

With `--simulate` it checks every cell's honest `E[e^λ]` on the joint counts and simulates the
audits a CUSUM needs to cross by share of int8 requests. **Gate:** the worst cell's `E[e^λ]` must
be at most 0.8 for the parameters to cover a hardware class, and only then may a live chain
enable the judgment. Version 1 (provisional, the GPU cells of `gpu-cal-v3-2026-10-09`) gives a
worst `E[e^λ]` of 0.616. False disputes are then at most 9.9 × 10⁻⁷ per provider and year, and
crossing takes 9 / 19 / 45 / 103 audits (median) at 100 / 50 / 30 / 20% int8. The CPU cells and
the final report fix the published values; a test checks that the Rust constant equals what the
committed report exports.

The calibration's prompts are fitted to target lengths with the engine's own tokenizer: about
80% in the band, 10% shorter (from 20 tokens) and 10% longer (up to 600), deterministic per
seed. The band comes from the crate (`ac-auditor thresholds` prints it), so the script cannot
drift from it.

## Runs

| Run | Seed | Honest / per cheat | Setting | Honest: pass / fail / inconclusive | Cheats passed |
|---|---|---|---|---|---|
| [36826010034](https://github.com/ixiejun/agentcoin/actions/runs/36826010034) | 1 | 3,000 / 100 | default | 2,776 / 204 / 20 | 0 |
| [36843601960](https://github.com/ixiejun/agentcoin/actions/runs/36843601960) | 2 | 600 / 50 | default, CPUs recorded | 77 failures, all on AMX CPUs | 0 |
| [36854446006](https://github.com/ixiejun/agentcoin/actions/runs/36854446006) | 3 | 600 / 50 | AMX off | 599 / 0 / 1 | 0 |
| [36871671591](https://github.com/ixiejun/agentcoin/actions/runs/36871671591) | 4 | 3,000 / 100 | AMX off | **2,983 / 1 / 16** | **0** |

Seed 4 by kind of cheating: another model 98 fail and 2 inconclusive; int8 99 fail and 1
inconclusive; int4 100 fail; hidden prompt 98 fail and 2 inconclusive.

### What the runs showed

- **AMX.** On Intel CPUs with AMX (Xeon 6973P-C, Platinum 8573C), oneDNN's AMX kernels made
  honest prefill chunks inexact. That caused nearly all honest failures of seeds 1 and 2. With
  oneDNN kept from AMX (`ONEDNN_MAX_CPU_ISA=AVX512_CORE_BF16`) the same CPUs were exact: seed 3
  had 60 of 60 passing. Re-checks on CPU therefore run with AMX off, and the plugin refuses
  verify mode on an AMX CPU without the setting.
- **Seed 4 per CPU** (honest; AMX off where present):

  | CPU | Samples | Fail | Inconclusive | Inexact prefill chunks |
  |---|---|---|---|---|
  | AMD EPYC 7763 (AVX2) | 1,200 | 1 | 7 | 50 |
  | Intel Xeon Platinum 8370C (AVX-512) | 600 | 0 | 2 | 1 |
  | AMD EPYC 9V74 (AVX-512 BF16) | 600 | 0 | 2 | 0 |
  | Intel Xeon Platinum 8573C (AMX, off) | 300 | 0 | 4 | 1 |
  | AMD EPYC 9V74 (AVX2) | 300 | 0 | 1 | 0 |

  The one honest failure was on the AMD EPYC 7763, where about 4% of prefill chunks were not
  bit-exact. It most likely exceeded the strict prefill bound; the decode chunks of honest
  samples stay far inside their bounds (worst: 8 exponent mismatches, mean 3.4).
- **Inconclusive** answers are 0.5% of honest samples, all "tokens cannot be re-created"
  (I-013). Cheating samples are inconclusive at a similar rate and never pass.

## Decision

By user decision (2026-10-01), `AUDIT_THRESHOLDS` stays at version 2, and re-checks on CPUs run
with AMX off. The acceptance of task 8.1 is the M6 target: an honest misjudgment rate below
0.1%. Seed 4 measured 1 in 3,000 (0.033%), with no cheat passing.

This rate is per auditor. Punishing a provider takes two independent auditors failing it within
two rounds, then reviewers re-checking the evidence and reaching a quorum (`market/audit`), so
a wrong punishment is far rarer still.

## Limits

- Calibrated only on CPUs (GitHub runners), with prover and auditor on the same kind of CPU.
  Providers on GPUs, an auditor on other hardware than the provider, and a provider proving with
  AMX re-checked without it are not measured (I-012; required before the α testnet).
- The AMD EPYC 7763's inexact prefill chunks (about 4%) are not explained yet (I-012).
- One small model (0.5B). Larger models and other architectures need their own runs.

## Cross-hardware calibration

The prover's and the auditor's hardware can differ: a provider on a GPU, an auditor on a CPU or
another GPU (OpenSpec change `m6-toploc-gpu-calibration`, I-012). The calibration script splits
into steps that run on different machines:

```bash
# On the prover's machine: generate a case bundle (cases/, prover.json, MANIFEST.sha256)
python scripts/calibrate-toploc.py --generate-only --honest 3000 --cheat 100 --seed 5 --out bundle-a
# On the auditor's machine: re-check it (optionally one shard of it)
python scripts/calibrate-toploc.py --recheck-only bundle-a --out recheck-a-on-b [--shard 0/10]
# Anywhere: merge reports, by cell
python scripts/calibrate-toploc.py --merge recheck-*/calibration.json --summary-out summary.json \
    [--thresholds prefill=E,M,D decode=E,M,D]
```

- **Fingerprints.** The bundle's `prover.json` and every re-check report record the hardware and
  software they ran on: CPU model, kernel flags and `ONEDNN_MAX_CPU_ISA`; GPU model, compute
  capability, driver, CUDA and cuDNN; PyTorch, vLLM and plugin versions; the models' revisions.
  A side of a cell is the GPU model, or `CPU <model> AMX on|off` (AMX on: the CPU has AMX and
  oneDNN may use it).
- **Cells.** The merged report counts every sample in its "prover → auditor" cell: honest
  passes, fails, inconclusive answers and inexact prefill chunks, and per cheating variant its
  samples and misses, in all and per side of the audit length band (`by_band`: inside, outside,
  or unknown for reports without prompt token counts). Reports from before the split count as
  their own host on both sides.
- **Integrity.** `MANIFEST.sha256` catches transfer damage: a listed case that is missing stops
  the re-check, a case whose digest differs is re-checked and flagged. Whether a case was
  changed is decided by the re-check itself: changed proofs do not open the receipt's
  commitment, a changed prompt or answer fails on a chunk.
- **Thresholds replayed.** The merge judges every sample again from its chunk metrics, as the
  auditor does; under the version the auditor judged by this must give each sample the
  auditor's own outcome, or the merge fails. It reports the smallest thresholds every honest
  sample passes (the prefill bounds per side of the band), how each cell fares under
  `--thresholds` (`band=MIN,MAX prefill=… prefill_outside=… decode=…`), and a conclusion by
  the rules below.
- **Conclusion rules** (spec "GPU 跨硬件校准", single-audit scope, every cell and side of the
  band on its own): no honest fail, and no miss of another model, int4 or a changed prompt; int8
  passes are reported, never a miss. Each cell needs at least 3,000 honest samples inside the band
  and 500 outside. The conclusion is `keep` (the current values hold), `widen` (every bound
  widened to the honest maximum, the band kept, still satisfies the rules), `no thresholds` (with
  the cells and sides that break the rules, for the user to decide), or `too few samples` (with
  what it would be). The CI quick regression never counts int8 passes as failures, nor honest
  answers listed under I-023.
- **Statistics.** The merge also gives, per cell, side of the band and variant (honest and int8),
  histograms of the statistics a verdict carries (the prefill chunk's mean and the decode
  chunks' means averaged, in hundredths) and their joint counts (`pairs`): the input of the
  statistical judgment's parameters. `scripts/export-audit-stats.py` derives them from one or
  more merged reports and, with `--simulate`, checks the honest `E[e^λ]` of every cell against
  the gate (0.8) and simulates how many audits find int8.
- **Condensed report.** `--summary-out` writes what the repository keeps: fingerprints, cells,
  distributions, thresholds, conclusion and the chunk metrics of every sample that did not go
  as it should; case names and numbers only, never prompts or answers.
- **One machine.** Without `--generate-only` or `--recheck-only` the script runs both steps on
  one machine, as before. CI checks that re-checking the regression's bundle again on its own,
  in two shards, gives the same outcomes (`--compare`).

The calibration workflow can do two cross-hardware runs, set in `plugins/vllm/ci/calibration.env`
(a push that changes it runs the workflow):

- `CALIBRATION_BUNDLE_RELEASE`, `CALIBRATION_BUNDLE_FILE`, `CALIBRATION_BUNDLE_SHA256`: the shards
  download that asset of a release of this repository (a `tar.gz` of a bundle, as
  `scripts/gpu-calibration.sh pack` makes it), check its SHA-256 and re-check one tenth of its
  cases each, with oneDNN kept from AMX: the "GPU → CPU AMX off" cells. To attach a bundle, open
  the repository's Releases page, draft a new pre-release (e.g. tag `calibration-gpu-1`), drop the
  file in and publish it.
- `CALIBRATION_PROVER_ISA`: the generating engines' `ONEDNN_MAX_CPU_ISA` (e.g. `ALL`), while the
  re-checking ones keep `ONEDNN_MAX_CPU_ISA`: on runners with AMX this is the "CPU AMX on → CPU
  AMX off" cell. Runners cannot be chosen, so the cell fills over runs with different seeds.

GPU machines are set up and run with `scripts/gpu-calibration.sh` (see
[GPU calibration](gpu-calibration.md)).

## Reproduce

Push a change to `plugins/vllm/ci/calibration.env` (sample counts and seed), or run locally
after `scripts/setup-vllm-cpu.sh` and `cargo build -p ac-auditor`:

```bash
python scripts/calibrate-toploc.py --honest 300 --cheat 10 --seed 4
python scripts/calibrate-toploc.py --quick   # the CI regression
```
