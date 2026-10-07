> 🌐 [English](gpu-calibration.md) | **简体中文**

# GPU 校准

如何在租用的 GPU 上运行 TOPLOC 复核校准（OpenSpec 变更 `m6-toploc-gpu-calibration`，问题 I-012）：一款消费级 GPU（RTX 3090）与一款数据中心 GPU（A100），各自证明并复核自己的回答与对方的回答。结果交回仓库；CPU 的格子在校准工作流中运行，合并后判定阈值是否成立（[TOPLOC 复核校准](toploc-calibration.zh-CN.md#跨硬件校准)）。

机器上的一切都由 `scripts/gpu-calibration.sh` 完成，与固定版本不符的任何情况都会让它停止：GPU 不原生支持 bfloat16、驱动不支持 CUDA 13.0、vLLM 或 PyTorch 版本不同、wheel、模型或 TOPLOC 参考实现的摘要不符，或插件在该 GPU 上检查不通过。

## 1. 租机器（AutoDL）

| | 消费级 | 数据中心 |
|---|---|---|
| GPU | RTX 3090（24 GB），单卡 | A100（40 或 80 GB），单卡 |
| 主机 | “最高 CUDA 版本” **13.0 或以上**——固定的 PyTorch 2.13.0 为 CUDA 13.0 构建，主机驱动须为 580 或更新 | 同左 |
| 镜像 | 任一 Ubuntu 22.04、Python 3.10–3.13 的基础镜像（镜像自带的 CUDA 与 PyTorch 不会被使用：脚本在虚拟环境中安装自己的） | 同左 |
| 磁盘 | 数据盘 `/root/autodl-tmp`（脚本在这里工作），约 25 GB | 同左 |

想在两台机器之间直接拷贝文件，就租在同一地区。每台机器预计 1.5–2 小时，大部分是安装；按约 ¥1.7/小时（3090）与 ¥6/小时（A100），整个运行花费几十元。中途暂停就关机：关机后 GPU 不计费，数据盘保留。

## 2. 安装每台机器

在每台机器上打开终端（JupyterLab → 终端）：

```bash
source /etc/network_turbo                 # AutoDL 学术加速：GitHub、crates、rustup
cd /root/autodl-tmp
git clone https://github.com/ixiejun/agentcoin.git
cd agentcoin
git checkout claude/determined-johnson-f8thb8   # 或给你的那个提交
# 可选的国内镜像（脚本校验每个摘要，镜像不在信任链内）：
export CARGO_MIRROR=sparse+https://rsproxy.cn/index/
export RUSTUP_DIST_SERVER=https://rsproxy.cn RUSTUP_UPDATE_ROOT=https://rsproxy.cn/rustup
scripts/gpu-calibration.sh setup           # 30–60 分钟；最后输出 "verify: ok"
```

`setup` 通过机器的 PyPI 镜像下载 vLLM 0.30.0 的 CUDA wheel（连同 PyTorch 2.13.0），通过 `https://hf-mirror.com` 下载模型（设置 `HF_ENDPOINT` 可换），从 GitHub 下载 TOPLOC 参考实现，并用 Rust 构建 `ac-auditor`。想省去第二台机器的 Rust 构建，可把第一台的 `target/debug/ac-auditor` 拷过去并运行 `AC_AUDITOR=/path/to/ac-auditor scripts/gpu-calibration.sh setup`。命令停止时打印 `STOP:` 与原因；解决后（或把信息发给我）再运行即可：每个命令都可以重复运行。

## 3. 检查、生成、复核

在每台机器上：

```bash
scripts/gpu-calibration.sh check      # 约 10 分钟：插件在该 GPU 上的检查，然后是快速回归
scripts/gpu-calibration.sh generate   # 3,000 个诚实 + 每类作弊 100 个；案例包在 /root/autodl-tmp/agentcoin-gpu
scripts/gpu-calibration.sh recheck /root/autodl-tmp/agentcoin-gpu/bundle-rtx-3090-seed11   # 复核自己的案例包
scripts/gpu-calibration.sh pack       # 打包并打印 SHA-256，在 /root/autodl-tmp/agentcoin-gpu/out
```

（A100 上案例包名为 `bundle-a100-…-seed12`，`ls /root/autodl-tmp/agentcoin-gpu` 可见。）`check` 停止时，这台 GPU 上不会生成任何东西：把它指出的日志发给我。

然后把各自的案例包交给另一台机器复核：

```bash
# 在 3090 上，先把 A100 的打包文件拷过来（scp、JupyterLab 文件浏览器，或同一地区共享的
# AutoDL 文件存储 /root/autodl-fs）：
scripts/gpu-calibration.sh recheck /root/autodl-tmp/bundle-a100-…-seed12.tar.gz
scripts/gpu-calibration.sh pack
# A100 上反过来做一遍
```

两台 AutoDL 机器之间拷贝：在源机器上运行
`scp -P <端口> /root/autodl-tmp/agentcoin-gpu/out/bundle-*.tar.gz root@<另一台机器的主机>:/root/autodl-tmp/`
（端口与主机见控制台上另一台机器的 SSH 登录命令）。

此后不再需要这两台机器，关机即可。

## 4. 交回结果

把两台机器 `out/` 目录中的所有文件（案例包、复核、检查；合计几十 MB）上传为仓库某个 Release 的附件：在 GitHub 上打开 **Releases → Draft a new release**，新建标签例如 `calibration-gpu-1`，勾选 **Set as a pre-release**，拖入文件并发布。把 Release 名称与 `pack` 打印的 `sha256sum` 各行发给我。之后：

1. 两个案例包由校准工作流在 CPU 上复核（`plugins/vllm/ci/calibration.env` 中的 `CALIBRATION_BUNDLE_*`）：即“GPU → CPU AMX off”各格；
2. 所有报告按格合并；精简报告提交到 `plugins/vllm/ci/calibration-results/`，结论写入校准报告。

## 各命令检查什么

| 命令 | 何时停止 |
|---|---|
| `setup` | 计算能力低于 8.0；驱动支持的 CUDA 低于 13.0；Python 低于 3.10；vLLM wheel 的 SHA-256 不是固定值；TOPLOC 参考实现归档的 SHA-256 不符；模型 revision 解析到其他提交或快照摘要不符 |
| `verify` | `setup` 未通过；vLLM 或 PyTorch 不是固定版本；没有支持 bfloat16 的 CUDA 设备；模型快照不符；没有 `ac-auditor` |
| `check` | `verify` 未通过；插件检查不通过（候选不是激活值的前 k 个，由候选构造的证明与由完整激活值构造的或与参考实现不同）；快速回归不通过（在该 GPU 上诚实回答复核不通过或作弊通过） |
| `generate`、`recheck` | 这台机器上 `verify` 或 `check` 未通过 |

`scripts/gpu-calibration.sh --self-test` 在没有 GPU 的机器上检验这些停止条件（CI 运行它）。
