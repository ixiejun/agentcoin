> 🌐 **English** | [简体中文](README.zh-CN.md)

# Cross-hardware calibration under thresholds version 4, 2026-10-15

Every cell of `m6-toploc-gpu-calibration` (tasks 8.3–8.5), judged by thresholds version 4 (a
single audit catches another model, int4 and a changed prompt; int8 goes to the statistical
judgment per provider):

- **GPU cells**: an RTX 5090 (seed 11) and an H800 PCIe (seed 12) each generated a bundle at
  `d860981` (4,000 honest answers and 150 per cheating variant, token IDs recorded) and re-checked
  its own and the other's. vLLM 0.30.0, PyTorch 2.13.0, drivers 595.71.05 (RTX 5090) and
  580.82.07 (H800 PCIe). Both machines passed `check`.
- **GPU → CPU**: the calibration workflow re-checked both bundles on GitHub's CPU runners with
  oneDNN kept from AMX, runs [38414362924](https://github.com/ixiejun/agentcoin/actions/runs/38414362924)
  (RTX 5090) and [38416432275](https://github.com/ixiejun/agentcoin/actions/runs/38416432275) (H800 PCIe).
- **CPU cells**: seeds 6–11 generated on the runners with AMX allowed to the generating engines
  and re-checked without it, runs
  [38416519408](https://github.com/ixiejun/agentcoin/actions/runs/38416519408),
  [38497678235](https://github.com/ixiejun/agentcoin/actions/runs/38497678235),
  [38497689716](https://github.com/ixiejun/agentcoin/actions/runs/38497689716),
  [38531073559](https://github.com/ixiejun/agentcoin/actions/runs/38531073559),
  [38531106857](https://github.com/ixiejun/agentcoin/actions/runs/38531106857) and
  [38669723452](https://github.com/ixiejun/agentcoin/actions/runs/38669723452).

A CPU side of a cell is "CPU AMX on" or "CPU AMX off" whatever the model (user decision,
2026-10-14); `by_model` in `summary.json` breaks each cell down by processor.

## Results

Honest answers: fail / inconclusive of the samples; cheats: passed of the samples. Answers whose
text re-tokenizes to other token IDs (I-023) are listed apart and not counted.

| Cell (prover → auditor) | Inside the band: honest | int8 passed | swap, int4, prompt passed | Outside: honest | int8 passed | swap, int4, prompt passed |
|---|---|---|---|---|---|---|
| RTX 5090 → RTX 5090 | 0 / 0 of 3,179 | 119 of 119 | 0 of 356 | 0 / 0 of 791 | 31 of 31 | 0 of 94 |
| RTX 5090 → H800 PCIe | 0 / 0 of 3,179 | 119 of 119 | 0 of 356 | 0 / 0 of 791 | 31 of 31 | 0 of 94 |
| RTX 5090 → CPU AMX off | 0 / 0 of 3,179 | 119 of 119 | 0 of 356 | 0 / 0 of 791 | 31 of 31 | 0 of 94 |
| H800 PCIe → RTX 5090 | 0 / 0 of 3,114 | 117 of 118 | 0 of 359 | 0 / 0 of 848 | 32 of 32 | 0 of 91 |
| H800 PCIe → H800 PCIe | 0 / 0 of 3,114 | 117 of 118 | 0 of 359 | 0 / 0 of 848 | 32 of 32 | 0 of 91 |
| H800 PCIe → CPU AMX off | 0 / 0 of 3,114 | 117 of 118 | 0 of 359 | 0 / 0 of 848 | 32 of 32 | 0 of 91 |
| CPU AMX off → CPU AMX off | 0 / 0 of 17,231 | 428 of 431 | 0 of 1,311 | 0 / 0 of 4,211 | 107 of 109 | 0 of 309 |
| CPU AMX on → CPU AMX off | 0 / 0 of **1,918** | 48 of 48 | 0 of 145 | 0 / 0 of **464** | 12 of 12 | 0 of 35 |

The statistics a verdict carries (hundredths of the mean mantissa error, inside the band):

| Cell | Honest prefill p99 / max | int8 prefill min / median | Honest decode max | int8 decode min |
|---|---|---|---|---|
| RTX 5090 → RTX 5090 | 0.70 / 1.40 | 0.95 / 1.25 | 2.65 | 1.90 |
| RTX 5090 → H800 PCIe | 0.70 / 1.30 | 0.85 / 1.20 | 2.30 | 1.80 |
| RTX 5090 → CPU AMX off | 0.65 / 1.60 | 0.85 / 1.25 | 2.60 | 1.75 |
| H800 PCIe → RTX 5090 | 0.75 / 1.35 | 0.90 / 1.30 | 3.25 | 1.55 |
| H800 PCIe → H800 PCIe | 0.55 / 1.25 | 0.80 / 1.20 | 2.25 | 1.40 |
| H800 PCIe → CPU AMX off | 0.65 / 1.35 | 0.80 / 1.25 | 2.35 | 1.50 |
| CPU AMX off → CPU AMX off | 0.00 / 0.05 | 0.80 / 1.25 | 1.05 | 1.45 |
| CPU AMX on → CPU AMX off | 0.45 / 1.05 | 0.90 / 1.25 | 1.80 | 1.75 |

(Bins of 0.05, lower edges.) Honest and int8 overlap on GPU provers, as in
[gpu-cal-v3-2026-10-09](../gpu-cal-v3-2026-10-09/README.md): no single bound separates them, hence
the statistical judgment.

- **I-023** (token IDs not re-created from the text): 380 honest re-checks over all cells, listed
  in `token_mismatch`: 327 inconclusive, 47 pass, 6 fail (two H800 cases on each of its three
  auditors). Without the record these 6 would be honest failures.
- **No proof missing**: every generated honest answer carries its proofs (`unproven` empty in both
  bundles and every shard).
- **Replay**: the merge judged every sample again under version 4 from its chunk metrics and got
  the auditor's outcome for each.

## Conclusion

`too few samples`, would be `keep`: in every cell no honest answer fails and no other model, int4
or changed prompt passes, so version 4 holds. One cell is short: "CPU AMX on → CPU AMX off" has
1,918 honest answers inside the band (3,000 required) and 464 outside (500 required). Only shards
that draw a runner with AMX contribute, and runners cannot be chosen: seeds 6–8 drew 5 of 30, seeds
9–11 none of 30, even with shards on such runners generating five times their share. By user
decision (2026-10-14) the shortfall is recorded and the conclusion stands as `keep`. With 0
failures in 2,382 samples the 95% upper bound of that cell's honest failure rate is about 0.13%,
against the 0.1% target; the cell covers providers proving on Intel CPUs with AMX.

## Statistical judgment parameters

`audit-stats-v1.json` is `scripts/export-audit-stats.py summary.json` (version 1, thresholds
version 4, band 150–300) and is the published `STATS_V1` in `ac-primitives` (a test checks it).
`audit-stats-v1-simulation.json` (`--simulate`, seed 1, 1,000 runs): the worst honest `E[e^λ]` is
0.618 (RTX 5090 → RTX 5090; gate 0.8), false disputes at most 9.85 × 10⁻⁷ per provider and year;
a CUSUM crosses after 9 / 19 / 41 / 102 audits (median; p95 9 / 32 / 87 / 326) when 100 / 50 / 30
/ 20% of the requests use int8. The largest share × p95 detection time is at 20%: 326 audits
are 163 rounds of 20 minutes, × 0.2 = 10.9 hours, so the exposure horizon H is 11 epochs
(`exposure` in the report; OpenSpec change `m6-audit-hardening` design D3).

## Files

`summary.json`: `calibrate-toploc.py --merge` of the four GPU re-checks and the 80 CPU shards
(`--summary-out`): cells with `by_band` and `by_model`, fingerprints, statistics histograms,
minimal thresholds, conclusion, `token_mismatch` and the metrics of every sample that did not go
as it should; case names and numbers only. The bundles, GPU re-checks and checks are assets of
the pre-release [calibration-gpu-2](https://github.com/ixiejun/agentcoin/releases/tag/calibration-gpu-2);
the CPU shards are the runs' artifacts.

| File | SHA-256 |
|---|---|
| `bundle-rtx-5090-seed11.tar.gz` | `a97fc90cb8a7cda1c808db70349de1cefd7a4fdb55e279128413aabfb5feb3c9` |
| `bundle-h800-pcie-seed12.tar.gz` | `60072e0bde1a1de64330a6ba983796108f7a5bc466c4c743f27bd3791f7bc74c` |
| `check-rtx-5090.tar.gz` | `e44559caa80dd3e61bac92ad6b963e4042c60d8ccc0eca22e39c17113314c8ed` |
| `check-h800-pcie.tar.gz` | `5d2a8ab9d90f6b29bc34494f3ab8fa9a00aa466cf0773b78e8ff32334e7d73dd` |
| `recheck-bundle-rtx-5090-seed11-on-rtx-5090.tar.gz` | `270fc983ffe27607cf2de339ca7588ea47c934c0ba2a67d2c8e9f0b3124451a3` |
| `recheck-bundle-rtx-5090-seed11-on-h800-pcie.tar.gz` | `6d65b3383ff53c5fcaecc488c40c95589212245e69aa4dd0c0c26e903f3b675b` |
| `recheck-bundle-h800-pcie-seed12-on-rtx-5090.tar.gz` | `3e6e6cd849290073e469448d9f23adb62dab5d8aeb984d27d484640efc4c46dd` |
| `recheck-bundle-h800-pcie-seed12-on-h800-pcie.tar.gz` | `4f91f33bbbf32279de906a6afbbe91467347a71da9b035a89d51479f949679c8` |
