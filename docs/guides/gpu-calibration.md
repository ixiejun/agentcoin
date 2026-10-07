> 🌐 **English** | [简体中文](gpu-calibration.zh-CN.md)

# GPU calibration

How to run the TOPLOC re-check calibration on rented GPUs (OpenSpec change
`m6-toploc-gpu-calibration`, issue I-012): a consumer GPU (RTX 3090) and a data-center GPU
(A100), each proving and re-checking its own answers and the other's. The results go back to the
repository, where the CPU cells run in the calibration workflow and the merge decides whether the
thresholds hold ([TOPLOC re-check calibration](toploc-calibration.md#cross-hardware-calibration)).

Everything on the machines is done by `scripts/gpu-calibration.sh`, which stops on anything
that differs from the pins: a GPU without native bfloat16, a driver without CUDA 13.0, another
vLLM or PyTorch version, a wheel, model or TOPLOC reference with another digest, or a plugin that
fails its check on the GPU.

## 1. Rent the machines (AutoDL)

| | Consumer | Data center |
|---|---|---|
| GPU | RTX 3090 (24 GB), one card | A100 (40 or 80 GB) or H800 (80 GB), one card |
| Host | "最高 CUDA 版本" (highest CUDA) **13.0 or later** — the pinned PyTorch 2.13.0 is built for CUDA 13.0, so the host driver must be 580 or newer | same |
| Image | any base image with Python 3.10–3.13 on Ubuntu 22.04 (the image's own CUDA and PyTorch are not used: the script installs its own in a virtual environment) | same |
| Disk | the data disk `/root/autodl-tmp` (the script works there); about 25 GB | same |

Rent both in the same region if you want to copy files between them directly. Expect about
1.5–2 hours per machine, most of it the installation; at about ¥1.7/h (3090) and ¥6/h (A100)
the whole run costs a few tens of yuan. Shut a machine down (关机) whenever you pause: a stopped
machine is not billed for its GPU and keeps its data disk.

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
scripts/gpu-calibration.sh setup           # 30–60 minutes; ends with "verify: ok"
```

`setup` downloads the CUDA wheel of vLLM 0.30.0 through the machine's PyPI mirror (PyTorch 2.13.0
comes with it), the models through `https://hf-mirror.com` (set `HF_ENDPOINT` for another), the
TOPLOC reference from GitHub, and builds `ac-auditor` with Rust. To skip the second Rust build,
copy `target/debug/ac-auditor` from the first machine and run
`AC_AUDITOR=/path/to/ac-auditor scripts/gpu-calibration.sh setup`. A command that stops prints
`STOP:` and the reason; fix it (or send the message) and run it again: every command can be
repeated.

## 3. Check, generate, re-check

On each machine:

```bash
scripts/gpu-calibration.sh check      # ~10 min: the plugin on this GPU, then the quick regression
scripts/gpu-calibration.sh generate   # 3,000 honest + 100 per cheat; a bundle in /root/autodl-tmp/agentcoin-gpu
scripts/gpu-calibration.sh recheck /root/autodl-tmp/agentcoin-gpu/bundle-rtx-3090-seed11   # its own bundle
scripts/gpu-calibration.sh pack       # archives and their SHA-256 in /root/autodl-tmp/agentcoin-gpu/out
```

(On the A100 or H800 the bundle is `bundle-a100-…-seed12` or `bundle-h800-…-seed12`; `ls /root/autodl-tmp/agentcoin-gpu` shows it.)
If `check` stops, nothing is generated on that GPU: send the log it names.

Then give each machine the other's bundle and re-check it:

```bash
# on the 3090, with the A100's archive copied over (scp, the JupyterLab file browser, or
# AutoDL's file storage /root/autodl-fs shared within a region):
scripts/gpu-calibration.sh recheck /root/autodl-tmp/bundle-a100-…-seed12.tar.gz
scripts/gpu-calibration.sh pack
# and the other way round on the A100
```

A copy between two AutoDL machines: on the source machine,
`scp -P <port> /root/autodl-tmp/agentcoin-gpu/out/bundle-*.tar.gz root@<host of the other machine>:/root/autodl-tmp/`
(the port and host are in the other machine's SSH command on the console).

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
| `verify` | `setup` has not passed; vLLM or PyTorch is not the pinned version; no CUDA device with bfloat16; a model snapshot differs; no `ac-auditor` |
| `check` | `verify` has not passed; the plugin check fails (candidates are not the activations' top k, proofs from candidates differ from the whole activations' or from the reference); the quick regression fails (an honest answer does not pass or a cheat passes its re-check, on this GPU) |
| `generate`, `recheck` | `verify` or `check` has not passed on this machine |

`scripts/gpu-calibration.sh --self-test` exercises these stops without a GPU (CI runs it).
