> 🌐 [English](README.md) | **简体中文**

# GPU 快速回归，2026-10-07：停在 `check`

在租用的 AutoDL 机器上首次运行 [GPU 校准](../../../../../docs/guides/gpu-calibration.zh-CN.md)（OpenSpec 变更 `m6-toploc-gpu-calibration`，问题 I-012）。两台机器都通过了插件检查，都**未通过快速复核回归**，因此按指南没有生成任何东西：没有案例包，也没有交叉复核。

| | 数据中心 | 消费级 |
|---|---|---|
| GPU | NVIDIA H800 PCIe（计算能力 9.0，80 GB） | NVIDIA GeForce RTX 5090（12.0，32 GB） |
| 驱动 / CUDA | 580.82.07 / 13.0 | 580.105.08 / 13.0 |
| 主机 CPU | Xeon Platinum 8458P | Xeon Platinum 8470Q |
| 软件 | torch 2.13.0、vLLM 0.30.0、插件 0.1.0、仓库 `a26111a` | 同左 |
| 插件检查 | all checks passed | all checks passed |
| 快速回归，诚实 | 通过 8、失败 39、不确定 1 | 通过 2、失败 45、不确定 1 |
| 快速回归，作弊 | 全部失败（swap、int8、int4、prompt） | 全部失败 |

## 诚实回答为何失败

阈值第 2 版（`crates/ac-market-proto/src/toploc.rs`，临时值，在 CPU 上校准）对 prefill 块的尾数误差均值上限为 0.50（指数不匹配至多 2，中位数 1），对每个 decode 块为 8.00（20、8）。两块 GPU 上诚实复核都远在 decode 界内，失败都出在 **prefill** 界：

| 尾数误差均值 | H800 PCIe | RTX 5090 |
|---|---|---|
| 诚实，prefill 块：中位 / 最大 | 0.70 / 1.36 | 1.42 / 3.22 |
| 诚实，单个回答的最差块：最大（指数不匹配最大） | 1.75（6） | 3.74（9） |
| int8 作弊，prefill 块：最小 / 中位 | 1.15 / 2.89 | 1.17 / 3.24 |
| swap / int4 / prompt，prefill 块：最小 | 7.99 / 9.36 / 12.26 | 7.91 / 9.77 / 11.45 |

在 RTX 5090 上，诚实回答的 prefill 误差（最大 3.22）与 int8 作弊的（最小 1.17）重叠：仅放宽 prefill 界，无法在那里既放过所有诚实回答又拦住所有 int8 作弊。swap、int4、prompt 三类作弊在两块 GPU 上都与诚实回答相距很远。“不确定”的样本原因是 "tokens cannot be re-created"（重新分词后 id 改变）。

复核像提供者那样以 prefill 计算提示，但在这两块 GPU 上两次 prefill 并非逐位一致，这是 CPU 校准没有遇到的；原因（计算核、CUDA graph、批形状）尚未查明。如何处理（阈值、复核路径、其他指标）是 `m6-toploc-gpu-calibration` 的设计决定；本次运行只记录数据。

## 途中发现的修正

在 `check` 能运行之前，机器需要 `scripts/gpu-calibration.sh` 现在会做的这些事（本次运行是手工完成的，见压缩包中的 `run-setup.sh` 与 `run-calib.sh`）：

- `VLLM_USE_V2_MODEL_RUNNER=0`：vLLM 0.30 在 GPU 上默认使用 V2 运行器，插件拒绝它。
- FlashInfer 在运行时用 `CUDA_HOME` 或 `PATH` 中的 nvcc 编译采样计算核。镜像的 CUDA 12.8 不能面向 sm_120（RTX 5090），而 vLLM wheel 的依赖带来的 CUDA 13.4 编译器生成的 PTX 被固定的 CUDA 13.0 运行时拒绝。现在虚拟环境会装上 13.0 编译器（`nvidia-cuda-nvcc`、`-crt`、`-cccl`、`nvidia-nvvm`）和无版本号的 `libcudart.so`，并让 `CUDA_HOME` 指向它；`verify` 会检查其版本。
- 下载：`HF_HUB_DISABLE_XET=1`（经 hf-mirror 的 Xet 传输被拒绝）；vLLM wheel 按其 PyPI 路径从 `PIP_INDEX_URL` 镜像下载；权重用 `MODELSCOPE=1`（hf-mirror 会把它们重定向到 Hugging Face 的存储，从国内只有几百 KB/s）；AutoDL 学术加速只用于 GitHub。

## 文件

- `h800-pcie/calibration.json`、`rtx-5090/calibration.json`：快速回归的报告。
- `logs/gpu-calib-logs-*.tar.gz`：每台机器的插件检查日志、快速回归的日志、报告、证明方指纹与各变体日志（不含案例文件）、安装与运行日志，以及手写的运行脚本。
