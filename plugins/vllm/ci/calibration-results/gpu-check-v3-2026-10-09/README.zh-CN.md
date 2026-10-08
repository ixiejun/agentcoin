> 🌐 [English](README.md) | **简体中文**

# GPU `check`，`08ffbbf`（阈值第 3 版），2026-10-09

在 RTX 5090（实例 `7b5b47989f-a83f36cc`）与 H800 PCIe 上运行 `scripts/gpu-calibration.sh check`，先重新编译了 `ac-auditor`（机器上的二进制早于 `ac-auditor thresholds`：`git pull` 之后 `check` 与 `generate` 不会重新编译它，只有 `setup` 会）。

阈值第 3 版（`ac-auditor thresholds`）：prompt 为 150–300 个 token 时，prefill（指数不匹配 6、均值 0.85、中位数 1）；区间之外（15、5.00、4）；decode（20、8.00、8）。

| | H800 PCIe | RTX 5090 |
|---|---|---|
| 插件检查 | all checks passed | all checks passed |
| 快速回归，诚实：通过 / 失败 / 不确定 | 通过（`check: ok`） | 47 / 1 / 0 |

RTX 5090 唯一的诚实失败是 `honest-00034`：prompt 208 个 token（在区间内），prefill 尾数误差均值 0.922（指数不匹配 0、中位数 1），界为 0.85；区间内其他诚实回答最多 0.661。有一个 int8 作弊通过（`int8-00003`）：它的 prompt 有 312 个 token，在区间之外，适用较宽的界。

`check-rtx-5090.tar.gz`：插件检查日志、快速回归的日志与报告（不含案例文件）以及本次运行的控制台日志。SHA-256 `ef8f93e67afc6caf057fafc14e6129b8d503ed928b6bd944702dc3715450a5e3`。
