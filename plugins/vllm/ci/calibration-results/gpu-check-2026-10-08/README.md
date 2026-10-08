> 🌐 **English** | [简体中文](README.zh-CN.md)

# GPU `check` at `67d2e22`, 2026-10-08

`scripts/gpu-calibration.sh check` on the RTX 5090 (a new clone, instance `7b5b47989f-a83f36cc`)
and on the H800 PCIe of the [first run](../gpu-quick-2026-10-07/README.md), at `67d2e22`
("fit the cases' segments by the provider's rule").

## Plugin check: passed on both

Every item passes on both GPUs, among them the new ones on the end boundary ("every answer's
decode segments fit the provider's rule", "one decode segment per output token but the last",
"no answer cut by the length limit has a decode segment more", "the batch has answers that end
with the end token").

## Quick regression: still fails on both, on the prefill chunk only

| | H800 PCIe | RTX 5090 |
|---|---|---|
| Honest: pass / fail / inconclusive | 8 / 39 / 1 | 2 / 45 / 1 |
| Honest failures on chunk 0 (prefill) | 39 of 39 | 45 of 45 |
| Honest decode chunks (131): exponent mismatches max, mean error max | 6, 1.75 | 8, 3.74 |
| Cheats (swap, int8, int4, prompt) | all fail (1 int8 inconclusive) | all fail (1 prompt inconclusive) |

No honest answer fails on a decode chunk any more: the end-boundary fix holds on both GPUs. What
remains is the prefill bound of thresholds version 2 (mean 0.50, calibrated on CPUs): the honest
prefill error is as in the first run (H800 median 0.70, max 1.36; RTX 5090 median 1.42, max
3.22, overlapping the int8 cheat's minimum of 1.17), since the quick regression's prompts are
the same short ones.

## Files

`logs/check-<gpu>.tar.gz`: the plugin check's log, the quick regression's log and report
(without the case files), and the run's console log (`check-run.log`). SHA-256:

```
6a61785298adbce75f2dc0f95ca08647e7580e275085f89eb0b3c782e2e459fb  check-h800-pcie.tar.gz
41c0215222aacdaa8b58448b06523229039c60656d39f80e6e21a2ace5513d8b  check-rtx-5090.tar.gz
```
