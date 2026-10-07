> 🌐 [English](toploc-calibration.md) | **简体中文**

# TOPLOC 复核校准

复核阈值 `AUDIT_THRESHOLDS`（`crates/ac-market-proto/src/toploc.rs`）是怎么定的，以及它区分诚实与作弊提供者的效果（m6-toploc-verify 任务 8.1、8.2）。M6 的目标（MVP 方案 §10）是诚实误判率低于 0.1%，同时换模型和降精度都能被发现。

## 方法

`scripts/calibrate-toploc.py` 在 CPU 上用真实 vLLM 生成样本并复核（工作流 `toploc-calibration`，在 GitHub 运行器上分十片运行，由 `calibrate-toploc.py --merge` 合并）：

- **固定版本**（`plugins/vllm/ci/pins.env`）：vLLM 0.30.0（CPU 版 wheel），模型 `Qwen/Qwen2.5-0.5B-Instruct`（`7ae55760…`），bfloat16。
- **诚实样本**：登记的模型在证明模式下运行，由 `ac-auditor recheck` 用同一模型的复核模式 vLLM 复核。
- **四类作弊**：同架构的另一个模型（`Qwen/Qwen2.5-0.5B`，`060db649…`）、登记模型的 int8 与 int4 权重，以及隐藏的 system prompt。
- 记录每个样本的结果与每一块的指标（指数不一致次数、尾数误差均值与中位数），并按 CPU 统计诚实样本的结果与不精确的预填充块数。

## 阈值（版本 2）

| 块 | 指数不一致 ≤ | 尾数误差均值 ≤ | 尾数误差中位数 ≤ |
|---|---|---|---|
| 预填充（第 0 块） | 2 | 0.50 | 1 |
| 解码（之后每一块） | 20 | 8.00 | 8 |

每一块都通过，推理才算通过。预填充块用严格阈值：审计员按提供者相同的方式计算 prompt，同类硬件上结果逐位相同。解码块用较宽的阈值：审计员在一次预填充里重算它们，计算方式不同。int8 作弊说明了预填充阈值为什么必须严格：它的解码块落在解码阈值以内（最差块的指数不一致为 2–11，均值为 1.6–4.6），主要靠预填充块被抓到。

## 运行记录

| 运行 | 种子 | 诚实 / 每类作弊 | 设置 | 诚实：通过 / 不通过 / 无法判定 | 作弊被放过 |
|---|---|---|---|---|---|
| [36826010034](https://github.com/ixiejun/agentcoin/actions/runs/36826010034) | 1 | 3,000 / 100 | 默认 | 2,776 / 204 / 20 | 0 |
| [36843601960](https://github.com/ixiejun/agentcoin/actions/runs/36843601960) | 2 | 600 / 50 | 默认，记录 CPU | 77 个不通过，全部在 AMX CPU 上 | 0 |
| [36854446006](https://github.com/ixiejun/agentcoin/actions/runs/36854446006) | 3 | 600 / 50 | 关闭 AMX | 599 / 0 / 1 | 0 |
| [36871671591](https://github.com/ixiejun/agentcoin/actions/runs/36871671591) | 4 | 3,000 / 100 | 关闭 AMX | **2,983 / 1 / 16** | **0** |

种子 4 按作弊类型：换模型 98 个不通过、2 个无法判定；int8 99 个不通过、1 个无法判定；int4 100 个不通过；隐藏 prompt 98 个不通过、2 个无法判定。

### 运行结果说明了什么

- **AMX**：在支持 AMX 的 Intel CPU（Xeon 6973P-C、Platinum 8573C）上，oneDNN 的 AMX 内核使诚实的预填充块不精确，种子 1、2 的诚实误判几乎全部由此造成。让 oneDNN 不使用 AMX（`ONEDNN_MAX_CPU_ISA=AVX512_CORE_BF16`）后，同样的 CPU 是精确的：种子 3 中 60 个全部通过。因此 CPU 上的复核关闭 AMX；插件在复核模式下，若在 AMX CPU 上未设置该变量就拒绝启动。
- **种子 4 按 CPU 统计**（诚实样本；有 AMX 的已关闭）：

  | CPU | 样本 | 不通过 | 无法判定 | 不精确的预填充块 |
  |---|---|---|---|---|
  | AMD EPYC 7763（AVX2） | 1,200 | 1 | 7 | 50 |
  | Intel Xeon Platinum 8370C（AVX-512） | 600 | 0 | 2 | 1 |
  | AMD EPYC 9V74（AVX-512 BF16） | 600 | 0 | 2 | 0 |
  | Intel Xeon Platinum 8573C（AMX，已关闭） | 300 | 0 | 4 | 1 |
  | AMD EPYC 9V74（AVX2） | 300 | 0 | 1 | 0 |

  唯一的诚实误判发生在 AMD EPYC 7763 上，这款 CPU 约 4% 的预填充块不是逐位相同。它很可能超出了严格的预填充阈值；诚实样本的解码块离解码阈值都很远（最差：指数不一致 8，均值 3.4）。
- **无法判定**占诚实样本的 0.5%，全部是“token 无法重现”（I-013）。作弊样本中无法判定的比例相近，且从未被判为通过。

## 决定

按用户决定（2026-10-01），`AUDIT_THRESHOLDS` 保持版本 2，CPU 上的复核关闭 AMX。任务 8.1 的验收标准采用 M6 的目标：诚实误判率低于 0.1%。种子 4 测得 3,000 中 1 个（0.033%），且没有作弊被放过。

这是单个审计员的误判率。要处罚一个提供者，需要两名独立审计员在两轮内都判其不通过，再由复核人复核证据并达到票数门槛（`market/audit`），因此错误处罚的概率还要低得多。

## 局限

- 只在 CPU（GitHub 运行器）上校准过，证明方与审计员在同类 CPU 上。GPU 上的提供者、审计员与提供者硬件不同、提供者用 AMX 出证明而审计员不用 AMX 复核，这些情况都没有测量（I-012；α 测试网之前必须完成）。
- AMD EPYC 7763 上约 4% 的预填充块不精确，原因尚未查明（I-012）。
- 只用了一个小模型（0.5B）。更大的模型与其他架构需要各自校准。

## 跨硬件校准

证明方与审计员的硬件可以不同：提供者在 GPU 上，审计员在 CPU 或另一款 GPU 上（OpenSpec 变更 `m6-toploc-gpu-calibration`，I-012）。校准脚本拆成可在不同机器上运行的几步：

```bash
# 在证明方机器上：生成案例包（cases/、prover.json、MANIFEST.sha256）
python scripts/calibrate-toploc.py --generate-only --honest 3000 --cheat 100 --seed 5 --out bundle-a
# 在审计员机器上：复核它（也可只复核其中一个分片）
python scripts/calibrate-toploc.py --recheck-only bundle-a --out recheck-a-on-b [--shard 0/10]
# 任意机器：按格合并报告
python scripts/calibrate-toploc.py --merge recheck-*/calibration.json --summary-out summary.json \
    [--thresholds prefill=E,M,D decode=E,M,D]
```

- **指纹。**案例包的 `prover.json` 与每份复核报告都记录运行所在的硬件与软件：CPU 型号、决定计算核的标志与 `ONEDNN_MAX_CPU_ISA`；GPU 型号、计算能力、驱动、CUDA 与 cuDNN；PyTorch、vLLM 与插件版本；模型 revision。格子的一边是 GPU 型号，或 `CPU <型号> AMX on|off`（AMX on：CPU 有 AMX 且 oneDNN 可以使用）。
- **格子。**合并报告把每个样本计入其“证明方 → 审计员”格：诚实样本的通过、不通过、无法判定与预填充非精确块数，以及每类作弊的样本数与漏检数。拆分之前的报告两边都计为其所在主机。
- **完整性。**`MANIFEST.sha256` 发现传输损坏：清单中的案例缺失时复核停止，摘要不符的案例照常复核并标记。案例是否被改动由复核本身判定：改动的证明打不开收据的承诺，改动的提示或回答在某一块上不通过。
- **重放阈值。**合并时按每块的指标把每个样本重新判定一遍，与审计员的规则相同；按版本 2 重放必须与审计员给出的结果逐样本相同，否则合并失败。报告给出让所有诚实样本通过的最小阈值、`--thresholds` 下各格的结果，以及结论：保持版本 2（每一格都成立）、新版本（把版本 2 放宽到诚实样本的最大值后仍抓住全部作弊），或没有统一阈值（交用户决定）。
- **精简报告。**`--summary-out` 写出仓库保存的内容：指纹、格子、分布、阈值、结论，以及每个结果不符合预期的样本的块指标；只有案例名与数字，没有提示与回答。
- **单机。**不带 `--generate-only` 或 `--recheck-only` 时，脚本在一台机器上依次完成两步，与以前相同。CI 检查把回归的案例包单独再复核一遍（分两个分片）得到相同的结果（`--compare`）。

校准工作流可以做两种跨硬件运行，在 `plugins/vllm/ci/calibration.env` 中设置（修改并推送即运行工作流）：

- `CALIBRATION_BUNDLE_RELEASE`、`CALIBRATION_BUNDLE_FILE`、`CALIBRATION_BUNDLE_SHA256`：各分片下载本仓库某个 Release 的这个附件（案例包的 `tar.gz`，即 `scripts/gpu-calibration.sh pack` 的产物），核对 SHA-256，各复核其中十分之一的案例，oneDNN 不使用 AMX：即“GPU → CPU AMX off”各格。上传附件：打开仓库的 Releases 页面，新建一个预发布（例如标签 `calibration-gpu-1`），拖入文件并发布。
- `CALIBRATION_PROVER_ISA`：生成用引擎的 `ONEDNN_MAX_CPU_ISA`（例如 `ALL`），复核用引擎仍用 `ONEDNN_MAX_CPU_ISA`：在有 AMX 的运行器上即“CPU AMX on → CPU AMX off”格。运行器无法选择，因此该格要靠不同种子的多次运行累积。

GPU 机器的安装与运行用 `scripts/gpu-calibration.sh`（见 [GPU 校准](gpu-calibration.zh-CN.md)）。

## 复现

修改并推送 `plugins/vllm/ci/calibration.env`（样本数与种子）；或在运行 `scripts/setup-vllm-cpu.sh` 与 `cargo build -p ac-auditor` 之后在本地运行：

```bash
python scripts/calibrate-toploc.py --honest 300 --cheat 10 --seed 4
python scripts/calibrate-toploc.py --quick   # CI 中的回归检查
```
