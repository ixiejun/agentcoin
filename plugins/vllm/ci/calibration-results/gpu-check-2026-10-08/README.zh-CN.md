> 🌐 [English](README.md) | **简体中文**

# GPU `check`，`67d2e22`，2026-10-08

在 RTX 5090（新克隆的实例 `7b5b47989f-a83f36cc`）与[第一次运行](../gpu-quick-2026-10-07/README.zh-CN.md)的 H800 PCIe 上运行 `scripts/gpu-calibration.sh check`，仓库为 `67d2e22`（"fit the cases' segments by the provider's rule"）。

## 插件检查：两台都通过

两块 GPU 上每一项都通过，包括关于结束边界的新检查项（"every answer's decode segments fit the provider's rule"、"one decode segment per output token but the last"、"no answer cut by the length limit has a decode segment more"、"the batch has answers that end with the end token"）。

## 快速回归：两台仍不通过，只败在 prefill 块

| | H800 PCIe | RTX 5090 |
|---|---|---|
| 诚实：通过 / 失败 / 不确定 | 8 / 39 / 1 | 2 / 45 / 1 |
| 诚实失败中出在 chunk 0（prefill）的 | 39 / 39 | 45 / 45 |
| 诚实 decode 块（131 个）：指数不匹配最大、误差均值最大 | 6、1.75 | 8、3.74 |
| 作弊（swap、int8、int4、prompt） | 全部失败（1 个 int8 不确定） | 全部失败（1 个 prompt 不确定） |

已没有诚实回答败在 decode 块：结束边界的修复在两块 GPU 上都成立。剩下的是阈值第 2 版的 prefill 界（均值 0.50，在 CPU 上校准）：诚实回答的 prefill 误差与第一次运行相同（H800 中位 0.70、最大 1.36；RTX 5090 中位 1.42、最大 3.22，与 int8 作弊的最小值 1.17 重叠），因为快速回归用的是同一批短 prompt。

## 文件

`logs/check-<gpu>.tar.gz`：插件检查日志、快速回归的日志与报告（不含案例文件），以及本次运行的控制台日志（`check-run.log`）。SHA-256：

```
6a61785298adbce75f2dc0f95ca08647e7580e275085f89eb0b3c782e2e459fb  check-h800-pcie.tar.gz
41c0215222aacdaa8b58448b06523229039c60656d39f80e6e21a2ace5513d8b  check-rtx-5090.tar.gz
```
