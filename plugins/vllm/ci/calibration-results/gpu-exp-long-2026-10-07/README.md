> 🌐 **English** | [简体中文](README.zh-CN.md)

# GPU long-prompt experiment, 2026-10-07

The [long-prompt experiment](../../../../../docs/guides/gpu-calibration.md#long-prompt-experiment)
after the [quick regression stopped at `check`](../gpu-quick-2026-10-07/README.md): on each GPU,
100 honest answers and 16 per cheat, every prompt at least 100 words (163 to 545 tokens),
re-checked on the same GPU and on the other one. Repository at `ff4be8c`; the machines and
software of the quick regression (the RTX 5090 is a clone of that machine on another host).
These bundles are an experiment and never part of the calibration.

## Results

Under thresholds version 2 every cheat fails in every cell; honest answers pass 22, 23, 20 and
81 times out of 100 (RTX 5090 → RTX 5090, RTX 5090 → H800, H800 → RTX 5090, H800 → H800), 4
inconclusive ("tokens cannot be re-created").

Prefill chunk mean mantissa error, by prover → auditor and prompt tokens (honest: median / max
(samples); int8 and swap: min (samples for int8)):

| Cell | Prompt tokens | Honest | int8 | swap |
|---|---|---|---|---|
| RTX 5090 → RTX 5090 | 128–255 | 0.54 / 0.72 (78) | 1.02 (14) | 7.60 |
| RTX 5090 → RTX 5090 | ≥ 256 | 0.54 / 0.66 (22) | 0.74 (2) | 4.63 |
| RTX 5090 → H800 PCIe | 128–255 | 0.54 / 0.68 (78) | 0.94 (14) | 7.48 |
| RTX 5090 → H800 PCIe | ≥ 256 | 0.52 / 0.62 (22) | 0.83 (2) | 4.60 |
| H800 PCIe → RTX 5090 | 128–255 | 0.54 / 0.69 (77) | 1.22 (15) | 6.23 |
| H800 PCIe → RTX 5090 | ≥ 256 | 0.50 / 0.61 (21) | 0.85 (1) | 6.36 |
| H800 PCIe → H800 PCIe | 128–255 | 0.42 / 0.55 (77) | 1.25 (15) | 6.32 |
| H800 PCIe → H800 PCIe | ≥ 256 | 0.42 / 0.50 (21) | 0.82 (1) | 6.33 |

- With long prompts the honest prefill error is far lower than in the quick regression (max
  0.72 here against 3.22 on the RTX 5090), and it is about the same whether the auditor's GPU
  is the prover's or the other one.
- In every cell and length bucket the largest honest error stays below the smallest int8
  error, but narrowly, and the int8 error falls with the prompt's length too: over all cells the
  honest maximum is 0.724 and the int8 minimum 0.740. The ≥ 256-token bucket holds only 1–2
  int8 samples per cell.
- The int8 cheat is not separated by the decode chunks: its worst chunk has a mean error of at
  least 1.87, while honest answers' worst chunks reach 2.52.
- The merge's smallest bounds that pass every honest answer here and fail every cheat:
  prefill (5 exponent mismatches, mean 0.73, median 1), decode (64, 8.00, 8). The decode
  exponent bound of 64 comes from a single honest case (seed 22, `honest-00050`): its last
  chunk has 64 positions, all with mismatched exponents, identically on both auditors, so it
  comes from the proof's side, not from the re-check's arithmetic. It needs a look before any
  bound is taken from this run.

This is data for the decision in `m6-toploc-gpu-calibration`, not a calibration: 100 honest
answers per bundle and 16 per cheat are too few for bounds.

## Files

- `summary.json`: the merge's condensed report (`scripts/calibrate-toploc.py --merge … --summary-out`),
  with `cells`, `by_prompt_length`, `minimal_thresholds`, `conclusion` and the unexpected cases.
- `rechecks/recheck-exp-long-<prover bundle>-on-<auditor>.tar.gz`: the four re-checks (report and
  logs). SHA-256:

```
95f2a1348a0456f760e519092b592434c53ec1db0a21455a56df1e1ffa7ede75  recheck-exp-long-h800-pcie-seed22-on-h800-pcie.tar.gz
77ccfb20d6a1848ea6f10bba9d43e56dcb03489252fd31377017a9073bb47332  recheck-exp-long-h800-pcie-seed22-on-rtx-5090.tar.gz
557db314d51f5377a35b1a02a777a75a4b2b3c84463c05e0e4c3b3dcc0374295  recheck-exp-long-rtx-5090-seed21-on-h800-pcie.tar.gz
2349ea5fed46948e6e45dab0ac97f2cce111ca37ecbbe5ce040743aca33143cb  recheck-exp-long-rtx-5090-seed21-on-rtx-5090.tar.gz
```

The bundles themselves (`exp-long-rtx-5090-seed21.tar.gz`, SHA-256 `181be572…5e57`, and
`exp-long-h800-pcie-seed22.tar.gz`, `12f0fbdb…c147`, about 1 MB each) stay on the machines.
