> 🌐 **English** | [简体中文](gpu-calibration.zh-CN.md)

# GPU calibration

How to run the TOPLOC re-check calibration on rented GPUs (OpenSpec change
`m6-toploc-gpu-calibration`, issue I-012): a consumer GPU (RTX 5090, Blackwell; an RTX 3090 or 4090 works as well) and a data-center
GPU (H800, Hopper; an A100 works as well), each proving and re-checking its own answers and the
other's. The results go back to the repository, where the CPU cells run in the calibration
workflow and the merge decides whether the thresholds hold
([TOPLOC re-check calibration](toploc-calibration.md#cross-hardware-calibration)).

Everything on the machines is done by `scripts/gpu-calibration.sh`, which stops on anything
that differs from the pins: a GPU without native bfloat16, a driver without CUDA 13.0, another
vLLM or PyTorch version, a wheel, model or TOPLOC reference with another digest, or a plugin that
fails its check on the GPU.

## 1. Rent the machines (AutoDL)

| | Consumer | Data center |
|---|---|---|
| GPU | RTX 5090 (32 GB), one card; or an RTX 3090 / 4090 (24 GB) | H800 (80 GB), one card; or an A100 (40 or 80 GB) |
| Host | "最高 CUDA 版本" (highest CUDA) **13.0 or later** — the pinned PyTorch 2.13.0 is built for CUDA 13.0, so the host driver must be 580 or newer | same |
| Image | any base image with Python 3.10–3.13 on Ubuntu 22.04 (the image's own CUDA and PyTorch are not used: the script installs its own in a virtual environment) | same |
| Disk | the data disk `/root/autodl-tmp` (the script works there); about 25 GB | same |

An RTX 5090 and an H800 are different architectures (Blackwell and Hopper), and vLLM runs other
kernels and attention backends on each: thresholds that hold between them hold for a wide range
of providers. The RTX 5090 (compute capability 12.0) is the newest card the pinned PyTorch and
vLLM build for; if a kernel were missing for it, `check` would show it within minutes. One card is enough (the model has 0.5B parameters; the script uses the first card).

The two machines may be in different regions: only two bundle archives (tens of MB) travel
between them, by `scp` over each machine's public SSH address, which works across regions. Expect about
1.5–2 hours per machine, most of it the installation. The H800 costs several times the 5090
(roughly ¥10–15/h against about ¥3–4/h; see the console for the current price), so set up and check the
5090 first, and start the H800 once the 5090 has passed `check`. Shut a machine down (关机)
whenever you pause: a stopped machine is not billed for its GPU and keeps its data disk. When it
is started again its GPU may be taken by someone else (AutoDL then offers only a mode without
GPU), so keep the 5090 running until it has re-checked the H800's bundle; that is the order
below.

## 2. Set up each machine

Open a terminal (JupyterLab → Terminal) on each machine:

```bash
source /etc/network_turbo                 # AutoDL's academic proxy: GitHub, crates, rustup
cd /root/autodl-tmp
git clone https://github.com/ixiejun/agentcoin.git
cd agentcoin
git checkout claude/determined-johnson-f8thb8   # or the commit you were given
# Optional mirrors in China (the script checks every digest, so mirrors are not trusted):
export CARGO_MIRROR=sparse+https://rsproxy.cn/index/
export RUSTUP_DIST_SERVER=https://rsproxy.cn RUSTUP_UPDATE_ROOT=https://rsproxy.cn/rustup
export PIP_INDEX_URL=https://pypi.tuna.tsinghua.edu.cn/simple   # PyTorch, and the vLLM wheel by its PyPI path
export MODELSCOPE=1                       # the models' weights from ModelScope
# The academic proxy for GitHub only: it slows everything else down to tens of KB/s.
export no_proxy="$no_proxy,pypi.tuna.tsinghua.edu.cn,hf-mirror.com,rsproxy.cn,modelscope.cn"
scripts/gpu-calibration.sh setup           # 30–60 minutes; ends with "verify: ok"
```

`setup` downloads the CUDA wheel of vLLM 0.30.0 through the machine's PyPI mirror (PyTorch 2.13.0
comes with it), the models through `https://hf-mirror.com` (set `HF_ENDPOINT` for another), the
TOPLOC reference from GitHub, and builds `ac-auditor` with Rust. A mirror of the hub can redirect
the large weight files to Hugging Face's own storage, which is slow or unreachable from some
regions: with `MODELSCOPE=1` they come from ModelScope instead, checked against the SHA-256 the
hub lists, and the snapshot digest is checked as before. It also installs the CUDA 13.0 compiler
into the environment, for the kernels FlashInfer compiles at run time (the image's own CUDA, 12.8
on AutoDL's images, cannot build for an RTX 5090). To save the H800's time, copy
`target/debug/ac-auditor` from the 5090 (built there first) and run
`AC_AUDITOR=/path/to/ac-auditor scripts/gpu-calibration.sh setup` on the H800 (the plugin check
still builds a small Rust example there). A command that stops prints `STOP:` and the reason; fix
it (or send the message) and run it again: every command can be repeated.

## 3. Check, generate, re-check

On each machine:

```bash
scripts/gpu-calibration.sh check      # ~10 min: the plugin on this GPU, then the quick regression
scripts/gpu-calibration.sh generate   # 3,000 honest + 100 per cheat; a bundle in /root/autodl-tmp/agentcoin-gpu
ls /root/autodl-tmp/agentcoin-gpu     # the bundle's name: bundle-rtx-5090-seed11, or bundle-h800…-seed12 on the H800
scripts/gpu-calibration.sh recheck /root/autodl-tmp/agentcoin-gpu/<its bundle>   # its own bundle
scripts/gpu-calibration.sh pack       # archives and their SHA-256 in /root/autodl-tmp/agentcoin-gpu/out
```

(The name comes from the GPU's model, e.g. `bundle-h800-pcie-seed12` or `bundle-h800-sxm-seed12`;
an A100 also gets seed 12.) If `check` stops, nothing is generated on that GPU: send the log it
names.

Then give each machine the other's bundle and re-check it:

```bash
# on the 5090, with the H800's archive copied over (scp, or the JupyterLab file browser:
# download it to your computer, upload it to the other machine):
scripts/gpu-calibration.sh recheck /root/autodl-tmp/bundle-h800-…-seed12.tar.gz
scripts/gpu-calibration.sh pack
# and on the H800, with the 5090's archive:
scripts/gpu-calibration.sh recheck /root/autodl-tmp/bundle-rtx-5090-seed11.tar.gz
scripts/gpu-calibration.sh pack
```

A copy between two AutoDL machines: on the source machine,
`scp -P <port> /root/autodl-tmp/agentcoin-gpu/out/bundle-*.tar.gz root@<host of the other machine>:/root/autodl-tmp/`
(the port and host are in the other machine's SSH command on the console, e.g.
`ssh -p 12345 root@connect.westb.seetacloud.com`; the password is asked once). This works
between regions. If it does not, download the archive in JupyterLab's file browser and upload
it to the other machine the same way. In order: the 5090 generates and re-checks its own
bundle and sends it to the H800; the H800 generates, re-checks its own and the 5090's, and sends
its bundle back; the 5090 re-checks it.

The machines are not needed after this; shut them down.

## 4. Hand back the results

Attach every file of both machines' `out/` directory (bundles, re-checks, checks; tens of MB in
all) to a release of the repository: on GitHub open **Releases → Draft a new release**, create a
tag such as `calibration-gpu-1`, tick **Set as a pre-release**, drop the files in and publish.
Send the release name and the `sha256sum` lines `pack` printed. From there:

1. the two bundles are re-checked on CPUs by the calibration workflow (`CALIBRATION_BUNDLE_*` in
   `plugins/vllm/ci/calibration.env`): the cells "GPU → CPU AMX off";
2. all reports are merged by cell; the condensed report goes to
   `plugins/vllm/ci/calibration-results/` and the conclusion to the calibration report.

## What the commands check

| Command | Stops when |
|---|---|
| `setup` | compute capability below 8.0; the driver's CUDA below 13.0; Python below 3.10; the vLLM wheel's SHA-256 is not the pinned one; the TOPLOC reference archive's SHA-256 differs; a model revision resolves to another commit or its snapshot digest differs |
| `verify` | `setup` has not passed; vLLM or PyTorch is not the pinned version; no CUDA device with bfloat16; a model snapshot differs; the environment's CUDA compiler is not CUDA 13.0; no `ac-auditor` |
| `check` | `verify` has not passed; the plugin check fails (candidates are not the activations' top k, proofs from candidates differ from the whole activations' or from the reference); the quick regression fails (an honest answer does not pass or a cheat passes its re-check, on this GPU) |
| `generate`, `recheck` | `verify` or `check` has not passed on this machine |

`scripts/gpu-calibration.sh --self-test` exercises these stops without a GPU (CI runs it).
