> 🌐 [English](README.md) | **简体中文**

# 阈值版本 4 下的跨硬件校准，2026-10-15

`m6-toploc-gpu-calibration` 的全部格子（任务 8.3–8.5），按阈值版本 4 判定（单次审计抓换模型、int4 与篡改 prompt；int8 交给按提供者的统计判定）：

- **GPU 格**：RTX 5090（种子 11）与 H800 PCIe（种子 12）在 `d860981` 上各生成一个案例包（4,000 个诚实回答、每类作弊 150 个，记录 token ID），复核自己的与对方的。vLLM 0.30.0，PyTorch 2.13.0，驱动 595.71.05（RTX 5090）与 580.82.07（H800 PCIe）。两台机器都通过了 `check`。
- **GPU → CPU**：校准工作流在 GitHub 的 CPU 运行器上复核两个案例包（oneDNN 不使用 AMX），运行 [38414362924](https://github.com/ixiejun/agentcoin/actions/runs/38414362924)（RTX 5090）与 [38416432275](https://github.com/ixiejun/agentcoin/actions/runs/38416432275)（H800 PCIe）。
- **CPU 格**：种子 6–11 在运行器上生成（生成用引擎允许 AMX），复核时不用 AMX，运行 [38416519408](https://github.com/ixiejun/agentcoin/actions/runs/38416519408)、[38497678235](https://github.com/ixiejun/agentcoin/actions/runs/38497678235)、[38497689716](https://github.com/ixiejun/agentcoin/actions/runs/38497689716)、[38531073559](https://github.com/ixiejun/agentcoin/actions/runs/38531073559)、[38531106857](https://github.com/ixiejun/agentcoin/actions/runs/38531106857) 与 [38669723452](https://github.com/ixiejun/agentcoin/actions/runs/38669723452)。

格子的 CPU 一边只分“CPU AMX on”与“CPU AMX off”，不论型号（用户决定，2026-10-14）；`summary.json` 中的 `by_model` 按处理器细分每一格。

## 结果

诚实回答：不通过 / 无法判定（占样本数）；作弊：被放过的个数（占样本数）。输出文本重新分词后 token ID 不同的回答（I-023）单列，不计入。

| 格子（证明方 → 审计员） | 区间内：诚实 | int8 放过 | 换模型、int4、篡改 prompt 放过 | 区间外：诚实 | int8 放过 | 换模型、int4、篡改 prompt 放过 |
|---|---|---|---|---|---|---|
| RTX 5090 → RTX 5090 | 3,179 个中 0 / 0 | 119 个中 119 | 356 个中 0 | 791 个中 0 / 0 | 31 个中 31 | 94 个中 0 |
| RTX 5090 → H800 PCIe | 3,179 个中 0 / 0 | 119 个中 119 | 356 个中 0 | 791 个中 0 / 0 | 31 个中 31 | 94 个中 0 |
| RTX 5090 → CPU AMX off | 3,179 个中 0 / 0 | 119 个中 119 | 356 个中 0 | 791 个中 0 / 0 | 31 个中 31 | 94 个中 0 |
| H800 PCIe → RTX 5090 | 3,114 个中 0 / 0 | 118 个中 117 | 359 个中 0 | 848 个中 0 / 0 | 32 个中 32 | 91 个中 0 |
| H800 PCIe → H800 PCIe | 3,114 个中 0 / 0 | 118 个中 117 | 359 个中 0 | 848 个中 0 / 0 | 32 个中 32 | 91 个中 0 |
| H800 PCIe → CPU AMX off | 3,114 个中 0 / 0 | 118 个中 117 | 359 个中 0 | 848 个中 0 / 0 | 32 个中 32 | 91 个中 0 |
| CPU AMX off → CPU AMX off | 17,231 个中 0 / 0 | 431 个中 428 | 1,311 个中 0 | 4,211 个中 0 / 0 | 109 个中 107 | 309 个中 0 |
| CPU AMX on → CPU AMX off | **1,918** 个中 0 / 0 | 48 个中 48 | 145 个中 0 | **464** 个中 0 / 0 | 12 个中 12 | 35 个中 0 |

裁决所带的统计量（尾数误差均值，区间内）：

| 格子 | 诚实预填充 p99 / 最大 | int8 预填充 最小 / 中位 | 诚实解码 最大 | int8 解码 最小 |
|---|---|---|---|---|
| RTX 5090 → RTX 5090 | 0.70 / 1.40 | 0.95 / 1.25 | 2.65 | 1.90 |
| RTX 5090 → H800 PCIe | 0.70 / 1.30 | 0.85 / 1.20 | 2.30 | 1.80 |
| RTX 5090 → CPU AMX off | 0.65 / 1.60 | 0.85 / 1.25 | 2.60 | 1.75 |
| H800 PCIe → RTX 5090 | 0.75 / 1.35 | 0.90 / 1.30 | 3.25 | 1.55 |
| H800 PCIe → H800 PCIe | 0.55 / 1.25 | 0.80 / 1.20 | 2.25 | 1.40 |
| H800 PCIe → CPU AMX off | 0.65 / 1.35 | 0.80 / 1.25 | 2.35 | 1.50 |
| CPU AMX off → CPU AMX off | 0.00 / 0.05 | 0.80 / 1.25 | 1.05 | 1.45 |
| CPU AMX on → CPU AMX off | 0.45 / 1.05 | 0.90 / 1.25 | 1.80 | 1.75 |

（以 0.05 分箱，取下沿。）证明方为 GPU 时诚实与 int8 有重叠，与 [gpu-cal-v3-2026-10-09](../gpu-cal-v3-2026-10-09/README.zh-CN.md) 一致：没有一个上限能把两者分开，因此用统计判定。

- **I-023**（无法从文本复原 token ID）：所有格子合计 380 次诚实复核，列在 `token_mismatch` 中：327 次无法判定，47 次通过，6 次不通过（H800 的两个案例，在它的三个审计员上各不通过一次）。若没有这项记录，这 6 次会算作诚实误判。
- **无证明缺失**：每个生成的诚实回答都带着证明（两个案例包与每个分片的 `unproven` 均为空）。
- **重放**：合并时按版本 4 由每块指标重新判定每个样本，结果与审计员逐个一致。

## 结论

`too few samples`，样本充足时为 `keep`：每一格都没有诚实回答不通过，换模型、int4、篡改 prompt 也没有一个被放过，因此版本 4 成立。只有一格样本不足：“CPU AMX on → CPU AMX off”区间内 1,918 个诚实回答（要求 3,000），区间外 464 个（要求 500）。只有抽到带 AMX 运行器的分片才计入这一格，而运行器无法选择：种子 6–8 的 30 个分片抽到 5 个，种子 9–11 的 30 个一个也没抽到，即使这类分片已按 5 倍生成。按用户决定（2026-10-14），如实记录这一不足，结论仍为 `keep`。该格 2,382 个样本 0 误判，诚实误判率 95% 置信上界约 0.13%，略高于 0.1% 的目标；这一格对应的是用带 AMX 的 Intel CPU 出证明的提供者。

## 统计判定参数

`audit-stats-v1.json` 即 `scripts/export-audit-stats.py summary.json` 的结果（版本 1，阈值版本 4，区间 150–300），也就是 `ac-primitives` 中发布的 `STATS_V1`（有测试检查两者一致）。`audit-stats-v1-simulation.json`（`--simulate`，种子 1，1,000 次）：最不利的诚实 `E[e^λ]` 为 0.618（RTX 5090 → RTX 5090；门槛 0.8），误开争议的概率不超过每个提供者每年 9.85 × 10⁻⁷；int8 占请求的 100% / 50% / 30% / 20% 时，CUSUM 越界需要 9 / 19 / 41 / 102 次审计（中位数；p95 为 9 / 32 / 87 / 326）。

## 文件

`summary.json`：四份 GPU 复核与 80 个 CPU 分片的 `calibrate-toploc.py --merge`（`--summary-out`）：带 `by_band` 与 `by_model` 的格子、硬件指纹、统计量直方图、最小阈值、结论、`token_mismatch`，以及所有结果不符预期的样本的块指标；只含案例名与数字。案例包、GPU 复核与检查结果是预发布版 [calibration-gpu-2](https://github.com/ixiejun/agentcoin/releases/tag/calibration-gpu-2) 的附件；CPU 分片是各次运行的产物。

| 文件 | SHA-256 |
|---|---|
| `bundle-rtx-5090-seed11.tar.gz` | `a97fc90cb8a7cda1c808db70349de1cefd7a4fdb55e279128413aabfb5feb3c9` |
| `bundle-h800-pcie-seed12.tar.gz` | `60072e0bde1a1de64330a6ba983796108f7a5bc466c4c743f27bd3791f7bc74c` |
| `check-rtx-5090.tar.gz` | `e44559caa80dd3e61bac92ad6b963e4042c60d8ccc0eca22e39c17113314c8ed` |
| `check-h800-pcie.tar.gz` | `5d2a8ab9d90f6b29bc34494f3ab8fa9a00aa466cf0773b78e8ff32334e7d73dd` |
| `recheck-bundle-rtx-5090-seed11-on-rtx-5090.tar.gz` | `270fc983ffe27607cf2de339ca7588ea47c934c0ba2a67d2c8e9f0b3124451a3` |
| `recheck-bundle-rtx-5090-seed11-on-h800-pcie.tar.gz` | `6d65b3383ff53c5fcaecc488c40c95589212245e69aa4dd0c0c26e903f3b675b` |
| `recheck-bundle-h800-pcie-seed12-on-rtx-5090.tar.gz` | `3e6e6cd849290073e469448d9f23adb62dab5d8aeb984d27d484640efc4c46dd` |
| `recheck-bundle-h800-pcie-seed12-on-h800-pcie.tar.gz` | `4f91f33bbbf32279de906a6afbbe91467347a71da9b035a89d51479f949679c8` |
