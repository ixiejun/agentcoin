> 🌐 **English** | [简体中文](README.zh-CN.md)

# GPU `check` at `08ffbbf` (thresholds version 3), 2026-10-09

`scripts/gpu-calibration.sh check` on the RTX 5090 (instance `7b5b47989f-a83f36cc`) and the H800
PCIe, with `ac-auditor` rebuilt first (the binary on the machines predated `ac-auditor thresholds`:
`check` and `generate` do not rebuild it after a `git pull`, only `setup` does).

Thresholds version 3 (`ac-auditor thresholds`): prompts of 150–300 tokens, prefill (6 exponent
mismatches, mean 0.85, median 1); outside the band (15, 5.00, 4); decode (20, 8.00, 8).

| | H800 PCIe | RTX 5090 |
|---|---|---|
| Plugin check | all checks passed | all checks passed |
| Quick regression, honest: pass / fail / inconclusive | passed (`check: ok`) | 47 / 1 / 0 |

The RTX 5090's one honest failure is `honest-00034`: 208 prompt tokens (in the band), prefill
mean mantissa error 0.922 (0 exponent mismatches, median 1) against the bound 0.85; the other
honest answers in the band reach at most 0.661. One int8 cheat passed (`int8-00003`): its prompt
has 312 tokens, outside the band, where the wider bounds apply.

`check-rtx-5090.tar.gz`: the plugin check's log, the quick regression's log and report (without
the case files) and the run's console log. SHA-256
`ef8f93e67afc6caf057fafc14e6129b8d503ed928b6bd944702dc3715450a5e3`.
