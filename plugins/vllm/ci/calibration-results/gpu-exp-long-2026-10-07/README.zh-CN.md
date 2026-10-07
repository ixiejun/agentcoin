> 🌐 [English](README.md) | **简体中文**

# GPU 长 prompt 实验，2026-10-07

在[快速回归停在 `check`](../gpu-quick-2026-10-07/README.zh-CN.md) 之后进行的[长 prompt 实验](../../../../../docs/guides/gpu-calibration.zh-CN.md)：每块 GPU 上 100 个诚实回答、每类作弊 16 个，每个 prompt 至少 100 个词（163 到 545 个 token），在同一块 GPU 和另一块 GPU 上复核。仓库为 `ff4be8c`；机器与软件同快速回归（RTX 5090 是那台机器在另一台主机上的克隆）。这些案例包是实验，不属于校准。

## 结果

在阈值第 2 版下，每个格子里所有作弊都失败；诚实回答分别通过 22、23、20、81 次（共 100 次；RTX 5090 → RTX 5090、RTX 5090 → H800、H800 → RTX 5090、H800 → H800），4 个不确定（"tokens cannot be re-created"）。

prefill 块的尾数误差均值，按“证明方 → 复核方”与 prompt token 数（诚实：中位 / 最大（样本数）；int8 与 swap：最小（int8 的样本数））：

| 格子 | prompt token | 诚实 | int8 | swap |
|---|---|---|---|---|
| RTX 5090 → RTX 5090 | 128–255 | 0.54 / 0.72（78） | 1.02（14） | 7.60 |
| RTX 5090 → RTX 5090 | ≥ 256 | 0.54 / 0.66（22） | 0.74（2） | 4.63 |
| RTX 5090 → H800 PCIe | 128–255 | 0.54 / 0.68（78） | 0.94（14） | 7.48 |
| RTX 5090 → H800 PCIe | ≥ 256 | 0.52 / 0.62（22） | 0.83（2） | 4.60 |
| H800 PCIe → RTX 5090 | 128–255 | 0.54 / 0.69（77） | 1.22（15） | 6.23 |
| H800 PCIe → RTX 5090 | ≥ 256 | 0.50 / 0.61（21） | 0.85（1） | 6.36 |
| H800 PCIe → H800 PCIe | 128–255 | 0.42 / 0.55（77） | 1.25（15） | 6.32 |
| H800 PCIe → H800 PCIe | ≥ 256 | 0.42 / 0.50（21） | 0.82（1） | 6.33 |

- 长 prompt 下诚实回答的 prefill 误差远低于快速回归（这里最大 0.72，快速回归中 RTX 5090 上为 3.22），且复核方 GPU 与证明方相同还是不同，结果相近。
- 每个格子、每个长度段里，诚实的最大误差都低于 int8 的最小误差，但差距很小，而且 int8 的误差也随 prompt 变长而下降：所有格子合计，诚实最大 0.724，int8 最小 0.740。≥ 256 token 一段每个格子只有 1–2 个 int8 样本。
- decode 块无法区分 int8 作弊：它的最差块误差均值至少 1.87，而诚实回答的最差块可达 2.52。
- 合并给出的、在本次数据上放过所有诚实回答并拦住所有作弊的最小界：prefill（指数不匹配 5、均值 0.73、中位数 1），decode（64、8.00、8）。decode 指数不匹配 64 来自单个诚实案例（种子 22，`honest-00050`）：它最后一个块有 64 个位置，指数全部不匹配，在两台复核机上完全相同，因此来自证明一侧，而非复核的数值误差。从本次运行取任何界之前需要先查明它。

这是给 `m6-toploc-gpu-calibration` 做决定用的数据，不是校准：每个案例包 100 个诚实回答、每类作弊 16 个，样本太少，不足以定阈值。

## 文件

- `summary.json`：合并的精简报告（`scripts/calibrate-toploc.py --merge … --summary-out`），含 `cells`、`by_prompt_length`、`minimal_thresholds`、`conclusion` 与意外案例。
- `rechecks/recheck-exp-long-<证明方案例包>-on-<复核方>.tar.gz`：四份复核（报告与日志）。SHA-256：

```
95f2a1348a0456f760e519092b592434c53ec1db0a21455a56df1e1ffa7ede75  recheck-exp-long-h800-pcie-seed22-on-h800-pcie.tar.gz
77ccfb20d6a1848ea6f10bba9d43e56dcb03489252fd31377017a9073bb47332  recheck-exp-long-h800-pcie-seed22-on-rtx-5090.tar.gz
557db314d51f5377a35b1a02a777a75a4b2b3c84463c05e0e4c3b3dcc0374295  recheck-exp-long-rtx-5090-seed21-on-h800-pcie.tar.gz
2349ea5fed46948e6e45dab0ac97f2cce111ca37ecbbe5ce040743aca33143cb  recheck-exp-long-rtx-5090-seed21-on-rtx-5090.tar.gz
```

案例包本身（`exp-long-rtx-5090-seed21.tar.gz`，SHA-256 `181be572…5e57`；`exp-long-h800-pcie-seed22.tar.gz`，`12f0fbdb…c147`；各约 1 MB）留在机器上。
