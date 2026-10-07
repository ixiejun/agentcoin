> 🌐 **English** | [简体中文](README.zh-CN.md)

# GPU quick regression, 2026-10-07: stopped at `check`

The first run of [GPU calibration](../../../../../docs/guides/gpu-calibration.md) on rented AutoDL
machines (OpenSpec change `m6-toploc-gpu-calibration`, issue I-012). Both machines passed the
plugin check and both **failed the quick re-check regression**, so, as the guide requires,
nothing was generated: there are no bundles and no cross re-checks.

| | Data center | Consumer |
|---|---|---|
| GPU | NVIDIA H800 PCIe (compute capability 9.0, 80 GB) | NVIDIA GeForce RTX 5090 (12.0, 32 GB) |
| Driver / CUDA | 580.82.07 / 13.0 | 580.105.08 / 13.0 |
| Host CPU | Xeon Platinum 8458P | Xeon Platinum 8470Q |
| Software | torch 2.13.0, vLLM 0.30.0, plugin 0.1.0, repository at `a26111a` | same |
| Plugin check | all checks passed | all checks passed |
| Quick regression, honest | 8 pass, 39 fail, 1 inconclusive | 2 pass, 45 fail, 1 inconclusive |
| Quick regression, cheats | all fail (swap, int8, int4, prompt) | all fail |

## Why the honest answers fail

Thresholds version 2 (`crates/ac-market-proto/src/toploc.rs`, provisional, calibrated on CPUs)
bound the prefill chunk at a mean mantissa error of 0.50 (at most 2 exponent mismatches, median
1) and every decode chunk at 8.00 (20, 8). On both GPUs the honest re-check stays well inside the
decode bounds and fails on the **prefill** bound:

| Mean mantissa error | H800 PCIe | RTX 5090 |
|---|---|---|
| Honest, prefill chunk: median / max | 0.70 / 1.36 | 1.42 / 3.22 |
| Honest, worst chunk of an answer: max (exp. mismatches max) | 1.75 (6) | 3.74 (9) |
| int8 cheat, prefill chunk: min / median | 1.15 / 2.89 | 1.17 / 3.24 |
| swap / int4 / prompt, prefill chunk: min | 7.99 / 9.36 / 12.26 | 7.91 / 9.77 / 11.45 |

On the RTX 5090 the honest prefill errors (max 3.22) overlap the int8 cheat's (min 1.17):
widening the prefill bound alone cannot both pass every honest answer and fail every int8 cheat
there. The swap, int4 and prompt cheats stay far apart from the honest answers on both GPUs. The
"inconclusive" samples are "tokens cannot be re-created" (re-tokenizing changes the ids).

The re-check computes the prompt in a prefill like the provider did, yet on these GPUs the two
prefills are not bit-identical, which the CPU calibration did not see; the cause (kernels, CUDA
graphs, batch shapes) has not been determined. What to do about it (bounds, the re-check path, another metric)
is a design decision for `m6-toploc-gpu-calibration`; this run only records the data.

## Fixes found on the way

Before `check` could run at all, the machines needed what `scripts/gpu-calibration.sh` now does
(the run applied them by hand, see `run-setup.sh` and `run-calib.sh` in the archives):

- `VLLM_USE_V2_MODEL_RUNNER=0`: vLLM 0.30 runs its V2 runner on GPUs; the plugin refuses it.
- FlashInfer compiles its sampling kernels at run time with the nvcc of `CUDA_HOME` or `PATH`.
  The image's CUDA 12.8 cannot target sm_120 (RTX 5090), and the CUDA 13.4 compiler the vLLM
  wheel's dependencies bring emits PTX the pinned CUDA 13.0 runtime rejects. The environment now
  gets the 13.0 compiler (`nvidia-cuda-nvcc`, `-crt`, `-cccl`, `nvidia-nvvm`), the unversioned
  `libcudart.so`, and `CUDA_HOME` pointing at it; `verify` checks its version.
- Downloads: `HF_HUB_DISABLE_XET=1` (Xet transfers are refused through hf-mirror); the vLLM wheel
  by its PyPI path on the `PIP_INDEX_URL` mirror; `MODELSCOPE=1` for the weights (hf-mirror
  redirects them to Hugging Face's storage, a few hundred KB/s from China); AutoDL's academic
  proxy for GitHub only.

## Files

- `h800-pcie/calibration.json`, `rtx-5090/calibration.json`: the quick regression's reports.
- `logs/gpu-calib-logs-*.tar.gz`: per machine, the plugin check log, the quick regression's
  log, report, prover fingerprint and per-variant logs (without the case files), the setup and
  run logs, and the hand-written run scripts.
