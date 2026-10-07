> 🌐 [English](gpu-calibration.md) | **简体中文**

# GPU 校准

如何在租用的 GPU 上运行 TOPLOC 复核校准（OpenSpec 变更 `m6-toploc-gpu-calibration`，问题 I-012）：一款消费级 GPU（RTX 5090，Blackwell 架构；RTX 3090 或 4090 也可以）与一款数据中心 GPU（H800，Hopper 架构；A100 也可以），各自证明并复核自己的回答与对方的回答。结果交回仓库；CPU 的格子在校准工作流中运行，合并后判定阈值是否成立（[TOPLOC 复核校准](toploc-calibration.zh-CN.md#跨硬件校准)）。

机器上的一切都由 `scripts/gpu-calibration.sh` 完成，与固定版本不符的任何情况都会让它停止：GPU 不原生支持 bfloat16、驱动不支持 CUDA 13.0、vLLM 或 PyTorch 版本不同、wheel、模型或 TOPLOC 参考实现的摘要不符，或插件在该 GPU 上检查不通过。

## 1. 租机器（AutoDL）

| | 消费级 | 数据中心 |
|---|---|---|
| GPU | RTX 5090（32 GB），单卡；或 RTX 3090 / 4090（24 GB） | H800（80 GB），单卡；或 A100（40 或 80 GB） |
| 主机 | “最高 CUDA 版本” **13.0 或以上**——固定的 PyTorch 2.13.0 为 CUDA 13.0 构建，主机驱动须为 580 或更新 | 同左 |
| 镜像 | 任一 Ubuntu 22.04、Python 3.10–3.13 的基础镜像（镜像自带的 CUDA 与 PyTorch 不会被使用：脚本在虚拟环境中安装自己的） | 同左 |
| 磁盘 | 数据盘 `/root/autodl-tmp`（脚本在这里工作），约 25 GB | 同左 |

RTX 5090 与 H800 属于不同架构（Blackwell 与 Hopper），vLLM 在两者上使用不同的计算核与注意力实现：在它们之间成立的阈值，对很大范围的提供者都成立。RTX 5090（计算能力 12.0）是固定版本的 PyTorch 与 vLLM 所支持的最新一代显卡；万一缺少它的计算核，`check` 几分钟内就会发现。单卡即可（模型只有 0.5B 参数；脚本只用第一张卡）。

两台机器可以在不同地区：它们之间只需传两个案例包压缩文件（几十 MB），用 `scp` 经各自的公网 SSH 地址传输，跨地区也可以。每台机器预计 1.5–2 小时，大部分是安装。H800 的价格是 5090 的好几倍（约 ¥10–15/小时对约 ¥3–4/小时，以控制台当前价格为准），所以先在 5090 上完成安装与检查，等 5090 通过 `check` 再开 H800。中途暂停就关机：关机后 GPU 不计费，数据盘保留。但再次开机时这块 GPU 可能已被别人租走（AutoDL 届时只提供无卡模式），所以 5090 要一直开着，直到它复核完 H800 的案例包，即下面的顺序。

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
export PIP_INDEX_URL=https://pypi.tuna.tsinghua.edu.cn/simple   # PyTorch，以及按 PyPI 路径下载的 vLLM wheel
export MODELSCOPE=1                       # 模型权重从 ModelScope 下载
# 学术加速只用于 GitHub：其他资源走它会慢到几十 KB/s。
export no_proxy="$no_proxy,pypi.tuna.tsinghua.edu.cn,hf-mirror.com,rsproxy.cn,modelscope.cn"
scripts/gpu-calibration.sh setup           # 30–60 分钟；最后输出 "verify: ok"
```

`setup` 通过机器的 PyPI 镜像下载 vLLM 0.30.0 的 CUDA wheel（连同 PyTorch 2.13.0），通过 `https://hf-mirror.com` 下载模型（设置 `HF_ENDPOINT` 可换），从 GitHub 下载 TOPLOC 参考实现，并用 Rust 构建 `ac-auditor`。Hub 镜像可能把大的权重文件重定向到 Hugging Face 自己的存储，在部分地区很慢或无法访问：设置 `MODELSCOPE=1` 后改从 ModelScope 下载，按 Hub 列出的 SHA-256 校验，快照摘要照常校验。它还会在虚拟环境中安装 CUDA 13.0 编译器，供 FlashInfer 运行时编译计算核（镜像自带的 CUDA，AutoDL 镜像上是 12.8，无法为 RTX 5090 编译）。为节省 H800 的时间，可把 5090 上先构建好的 `target/debug/ac-auditor` 拷过去，在 H800 上运行 `AC_AUDITOR=/path/to/ac-auditor scripts/gpu-calibration.sh setup`（插件检查仍会在那里构建一个小的 Rust 示例）。命令停止时打印 `STOP:` 与原因；解决后（或把信息发给我）再运行即可：每个命令都可以重复运行。

## 3. 检查、生成、复核

在每台机器上：

```bash
scripts/gpu-calibration.sh check      # 约 10 分钟：插件在该 GPU 上的检查，然后是快速回归
scripts/gpu-calibration.sh generate   # 3,000 个诚实 + 每类作弊 100 个；案例包在 /root/autodl-tmp/agentcoin-gpu
ls /root/autodl-tmp/agentcoin-gpu     # 案例包名：bundle-rtx-5090-seed11，H800 上为 bundle-h800…-seed12
scripts/gpu-calibration.sh recheck /root/autodl-tmp/agentcoin-gpu/<自己的案例包>   # 复核自己的案例包
scripts/gpu-calibration.sh pack       # 打包并打印 SHA-256，在 /root/autodl-tmp/agentcoin-gpu/out
```

（案例包名取自 GPU 型号，例如 `bundle-h800-pcie-seed12` 或 `bundle-h800-sxm-seed12`；A100 同样用种子 12。）`check` 停止时，这台 GPU 上不会生成任何东西：把它指出的日志发给我。

然后把各自的案例包交给另一台机器复核：

```bash
# 在 5090 上，先把 H800 的打包文件拷过来（scp，或 JupyterLab 文件浏览器：
# 下载到你的电脑，再上传到另一台机器）：
scripts/gpu-calibration.sh recheck /root/autodl-tmp/bundle-h800-…-seed12.tar.gz
scripts/gpu-calibration.sh pack
# 在 H800 上，拷入 5090 的打包文件：
scripts/gpu-calibration.sh recheck /root/autodl-tmp/bundle-rtx-5090-seed11.tar.gz
scripts/gpu-calibration.sh pack
```

两台 AutoDL 机器之间拷贝：在源机器上运行
`scp -P <端口> /root/autodl-tmp/agentcoin-gpu/out/bundle-*.tar.gz root@<另一台机器的主机>:/root/autodl-tmp/`
（端口与主机见控制台上另一台机器的 SSH 登录命令，例如 `ssh -p 12345 root@connect.westb.seetacloud.com`；会询问一次密码）。跨地区同样可用。若不通，就在 JupyterLab 文件浏览器中下载打包文件，再以同样方式上传到另一台机器。顺序是：5090 生成并复核自己的案例包，发给 H800；H800 生成、复核自己的与 5090 的，再把自己的案例包发回；5090 复核它。

此后不再需要这两台机器，关机即可。

## 4. 交回结果

把两台机器 `out/` 目录中的所有文件（案例包、复核、检查；合计几十 MB）上传为仓库某个 Release 的附件：在 GitHub 上打开 **Releases → Draft a new release**，新建标签例如 `calibration-gpu-1`，勾选 **Set as a pre-release**，拖入文件并发布。把 Release 名称与 `pack` 打印的 `sha256sum` 各行发给我。之后：

1. 两个案例包由校准工作流在 CPU 上复核（`plugins/vllm/ci/calibration.env` 中的 `CALIBRATION_BUNDLE_*`）：即“GPU → CPU AMX off”各格；
2. 所有报告按格合并；精简报告提交到 `plugins/vllm/ci/calibration-results/`，结论写入校准报告。

## 长 prompt 实验

第一次运行停在了 `check`（见 [gpu-quick-2026-10-07](../../plugins/vllm/ci/calibration-results/gpu-quick-2026-10-07/README.zh-CN.md)）：两台 GPU 上诚实回答都过不了 prefill 上限，大多是短 prompt；而 prompt 不少于 64 个词时，诚实样本的 prefill 误差都不超过 0.65，int8 作弊都不低于 1.15。在决定复核怎么改之前，先做一个小实验：只用长 prompt，在每台 GPU 上以及两台之间测量这一点。它只要求插件检查通过（不要求快速回归通过），其案例包（`exp-long-…`）永远不计入校准。

```bash
cd /root/autodl-tmp/agentcoin && git pull     # 实验需要最新的脚本
# 在此版本之前已通过插件检查的机器上：现在会保留插件检查的结果；可以重新运行 `check`
# （它仍会如预期停在快速回归），或者，由于 check-<gpu>/check-vllm-plugin.log 以
# "all checks passed" 结尾，手动标记：
touch /root/autodl-tmp/agentcoin-gpu/state/plugin.ok
scripts/gpu-calibration.sh experiment   # 100 个诚实 + 每类作弊 16 个，prompt 不少于 100 个词，本机复核
scripts/gpu-calibration.sh pack         # out/exp-long-<gpu>-seed2x.tar.gz 及其复核
```

RTX 5090 上的案例包为 `exp-long-rtx-5090-seed21`，H800 上为 `exp-long-h800-…-seed22`。然后把各自的案例包交给另一台机器复核（拷贝方法同第 3 节），再打包一次：

```bash
scripts/gpu-calibration.sh recheck /root/autodl-tmp/exp-long-h800-…-seed22.tar.gz   # 在 5090 上
scripts/gpu-calibration.sh recheck /root/autodl-tmp/exp-long-rtx-5090-seed21.tar.gz # 在 H800 上
scripts/gpu-calibration.sh pack
```

交回两台机器的 `recheck-exp-long-*.tar.gz`（Release 附件，或像第一次运行那样提交）。合并报告按格与 prompt 长度给出 prefill 误差（`by_prompt_length`），据此判断用长 prompt 审计能否在单台 GPU 上以及跨 GPU 时把诚实回答与 int8 作弊分开。每台机器约 30 分钟（`MIN_PROMPT_WORDS`、`HONEST`、`CHEAT` 可改默认值）。

## 各命令检查什么

| 命令 | 何时停止 |
|---|---|
| `setup` | 计算能力低于 8.0；驱动支持的 CUDA 低于 13.0；Python 低于 3.10；vLLM wheel 的 SHA-256 不是固定值；TOPLOC 参考实现归档的 SHA-256 不符；模型 revision 解析到其他提交或快照摘要不符 |
| `verify` | `setup` 未通过；vLLM 或 PyTorch 不是固定版本；没有支持 bfloat16 的 CUDA 设备；模型快照不符；虚拟环境中的 CUDA 编译器不是 CUDA 13.0；没有 `ac-auditor` |
| `check` | `verify` 未通过；插件检查不通过（候选不是激活值的前 k 个，由候选构造的证明与由完整激活值构造的或与参考实现不同）；快速回归不通过（在该 GPU 上诚实回答复核不通过或作弊通过） |
| `generate`、`recheck` | 这台机器上 `verify` 或 `check` 未通过 |
| `experiment`、复核 `exp-…` 案例包 | 这台机器上 `verify` 或 `check` 中的插件检查未通过 |

`scripts/gpu-calibration.sh --self-test` 在没有 GPU 的机器上检验这些停止条件（CI 运行它）。
