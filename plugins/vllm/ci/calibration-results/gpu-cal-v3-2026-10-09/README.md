> 🌐 **English** | [简体中文](README.zh-CN.md)

# GPU calibration under thresholds version 3, 2026-10-09

The four GPU cells of `m6-toploc-gpu-calibration` (tasks 8.1 and 8.2): an RTX 5090 (seed 11) and an
H800 PCIe (seed 12) each generated a bundle at `08ffbbf` (4,000 honest answers and 150 per
cheating variant, prompts fitted to target lengths around the audit length band 150–300 tokens),
re-checked its own and the other's. vLLM 0.30.0, PyTorch 2.13.0, drivers 580.105.08 (RTX 5090)
and 580.82.07 (H800 PCIe); fingerprints in `summary.json`.

How it ran: `ac-auditor` was rebuilt by hand on both machines (the binary `setup` had built
predated `ac-auditor thresholds`; `check`, `generate` and `recheck` now rebuild it and the
calibration script refuses a binary whose thresholds are not the checkout's). The H800 passed
`check`. The RTX 5090's quick regression failed on one honest answer
([gpu-check-v3-2026-10-09](../gpu-check-v3-2026-10-09/README.md)); its bundle was generated anyway
(`state/check.ok` written by hand), since what the regression measures is what the calibration
is to decide.

## Results

Honest answers: fail / inconclusive of the samples; cheats: passed (missed) of the samples.

| Cell (prover → auditor) | Inside the band: honest | int8 | swap, int4, prompt | Outside: honest | int8 | swap, int4, prompt |
|---|---|---|---|---|---|---|
| RTX 5090 → RTX 5090 | 9 / 20 of 3,202 | 0 of 119 | 0 | 0 / 6 of 798 | 31 of 31 | 0 |
| RTX 5090 → H800 PCIe | 9 / 20 of 3,202 | 0 of 119 | 0 | 0 / 6 of 798 | 31 of 31 | 0 |
| H800 PCIe → RTX 5090 | 13 / 24 of 3,142 | 0 of 117 | 0 | 1 / 8 of 858 | 32 of 32 | 0 |
| H800 PCIe → H800 PCIe | 8 / 24 of 3,142 | 1 of 117 | 0 | 1 / 8 of 858 | 32 of 32 | 0 |

The same bundle re-checked on the two GPUs gives other chunk metrics (identical for under 1% of
the cases) but the same outcome for 99.7% of them: the prover's GPU, not the auditor's, decides.

Prefill chunk, mean mantissa error (judged samples):

| Cell | Inside: honest median / p99 / p99.9 / max | int8 min / p10 / median | Outside: honest p99 / max | int8 min |
|---|---|---|---|---|
| RTX 5090 → RTX 5090 | 0.53 / 0.70 / 1.09 / 1.40 | 0.99 / 1.07 / 1.28 | 3.36 / 3.88 | 0.83 |
| RTX 5090 → H800 PCIe | 0.54 / 0.70 / 0.97 / 1.31 | 0.87 / 1.06 / 1.23 | 1.63 / 2.17 | 0.79 |
| H800 PCIe → RTX 5090 | 0.54 / 0.75 / 1.25 / 1.38 | 0.94 / 1.09 / 1.31 | 2.98 / 3.30 | 0.82 |
| H800 PCIe → H800 PCIe | 0.43 / 0.56 / 1.08 / 1.30 | 0.85 / 1.08 / 1.24 | 1.24 / 1.48 | 0.74 |

Another model, int4 and a changed prompt stay far away on both sides of the band (int4's
smallest prefill mean 8.31).

- **Inside the band, the honest tail reaches int8.** About 0.1% of honest answers have a prefill
  mean above 0.97, up to 1.40 (with up to 9 exponent mismatches), while int8's median is 1.24–1.31.
  No bound gives both no honest failure and no int8 miss. All four cells together (12,688 honest
  answers inside the band, 474 int8), with the other prefill bounds at the honest maximum (9
  exponent mismatches, median 1) and the decode bounds of version 3:

  | Prefill mean bound inside the band | Honest fails | int8 missed |
  |---|---|---|
  | 0.85 (version 3's provisional mean; with its 6 exponent mismatches: 39 fails) | 38 (0.30%) | 1 |
  | 1.00 | 27 (0.21%) | 14 |
  | 1.10 | 20 (0.16%) | 63 |
  | 1.20 | 15 (0.12%) | 161 |
  | 1.41 | 2 (0.02%, the decode case below) | 382 |

  The earlier long-prompt experiment (100 honest answers per cell) had seen an honest maximum of
  0.72; with 3,000 per cell the tail is longer.
- **Two honest answers fail a decode chunk on both auditors**: seed 12 `honest-03727` (chunk 1:
  83 of 128 exponents differ, mean 27.7) and `honest-00036` (chunk 2: 32 differ, mean 4.6).
  Identical on both GPUs, so they come from the tokens, not from arithmetic; most likely answers
  whose text re-tokenizes to the same number of tokens but other IDs (the generation logs counted
  6 such answers on the H800 and 4 on the RTX 5090), which the auditor then recomputes as other
  tokens. Without them the honest decode chunks stay within (19, 7.96, 4), inside the decode bounds.
- **Inconclusive**: 116 of 16,000 honest re-checks (0.73%), all `tokens cannot be re-created`
  (I-013).

## Conclusion

`no thresholds` (spec engineering/ci-quality-gates "GPU 跨硬件校准"): no set of bounds with the
band passes every honest answer and fails every int8 answer inside the band, in any cell. Version 3
is not finalized; the decision goes to the user (task 9.1). The CPU cells (tasks 8.3, 8.4) were not
run.

## Files

`summary.json`: `calibrate-toploc.py --merge` of the four re-checks (`--summary-out`): cells,
fingerprints, distributions, minimal thresholds, conclusion and the metrics of every sample that
did not go as it should; case names and numbers only. The bundles and re-checks are assets of the
pre-release [calibration-gpu-2](https://github.com/ixiejun/agentcoin/releases/tag/calibration-gpu-2):

| File | SHA-256 |
|---|---|
| `bundle-rtx-5090-seed11.tar.gz` | `a9d15b96092a31c78d5e8bda2f72741eb5fd28bcf644f0b4f4db024ce63c3a1a` |
| `bundle-h800-pcie-seed12.tar.gz` | `bbb7c04b95789cd2875c480648a8771264cb618c7f19a095f9a353b1dd7deefb` |
| `recheck-bundle-rtx-5090-seed11-on-rtx-5090.tar.gz` | `722cad37b4d8435db91b7917f08605a3259b229e9ec976c7d2510f01fb5b87b3` |
| `recheck-bundle-rtx-5090-seed11-on-h800-pcie.tar.gz` | `10744dd0c02b30e38fd59e25ab220fecbfaf0955ef03675bbf8031a2699eddca` |
| `recheck-bundle-h800-pcie-seed12-on-rtx-5090.tar.gz` | `01e34a127e139a6e82c5feba612774222719c785b6fe8957307871f01ce993af` |
| `recheck-bundle-h800-pcie-seed12-on-h800-pcie.tar.gz` | `138a7adb43d4dbb058c4e8b0907eaa14e5e4e155cf1b859fe9f708faf5bcac75` |

The release's `exp-long-*` assets are the long-prompt experiment's
([gpu-exp-long-2026-10-07](../gpu-exp-long-2026-10-07/README.md)), not part of this calibration.
