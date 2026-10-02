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

## Reproduce

Push a change to `plugins/vllm/ci/calibration.env` (sample counts and seed), or run locally
after `scripts/setup-vllm-cpu.sh` and `cargo build -p ac-auditor`:

```bash
python scripts/calibrate-toploc.py --honest 300 --cheat 10 --seed 4
python scripts/calibrate-toploc.py --quick   # the CI regression
```
