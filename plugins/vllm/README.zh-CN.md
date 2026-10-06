> 🌐 [English](README.md) | **简体中文**

# agentcoin-vllm

[vLLM](https://github.com/vllm-project/vllm) 的 AgentCoin TOPLOC 插件（规格 `market/engine-plugin`，MVP 方案 §2.1，决策 D31）。它在 vLLM 引擎内部运行，把每个请求的最终层激活值的 top-k 交给本机的 AgentCoin 提供者代理（`ac-provider run --toploc-socket …`），由后者构造收据所承诺的 TOPLOC 证明。插件不含证明逻辑：证明由 Rust 的 `ac-toploc` 构造。

许可证：`MIT OR Apache-2.0`（宽松许可区，D47）。

## 工作方式

- vLLM 在它启动的每个进程中通过 `vllm.general_plugins` 入口加载插件。没有设置 `AGENTCOIN_TOPLOC_SOCKET` 时，插件什么也不做。
- 加载模型时，插件检查前提条件，任一不满足就使引擎启动失败（见下文）。
- 每个前向步骤之后，插件从 vLLM 的 V1 模型运行器读取模型最终归一化层的输出（TOPLOC 参考实现读取的就是这组激活值），按请求切分，并在设备上对每个请求的那一部分（一个*段*：本步计算的 prompt token，或一个解码 token）取按绝对值最大的 `k` 个值。下标与 bfloat16 编码异步拷到主机；后台线程经本机 Unix 套接字发给提供者，请求结束时发送结束标记。推理线程从不等待 I/O：提供者不可达或队列已满时，段被丢弃（并计数），插件每秒重连一次。
- 协议见 `crates/ac-market-proto/src/engine.rs`（版本 2）。提供者在连接时告知插件 `k`（市场中为 128）；提供者的协议版本不同时，插件停止发送。
- **模式。** `AGENTCOIN_TOPLOC_MODE` 决定候选的用途：`prove`（默认，供提供者使用，如上所述）或 `verify`（供审计员的复核 `ac-auditor recheck` 使用）。复核模式下，引擎只对“prompt + 输出”做预填充，插件为预填充的每个 token 行发送一段（该行的前 `k` 个值），按行序发送，解码步骤不发送；审计员由这些行重建原推理的分块。插件在连接时声明模式：提供者只接受 `prove`，审计员只接受 `verify`，模式被拒绝时插件停止发送（日志记为模式不符）。其他取值使引擎启动失败。
- **AMX。** 复核阈值是在 oneDNN 不使用 AMX 内核的条件下校准的：在支持 AMX 的 Intel CPU 上，AMX 内核会使诚实的预填充块不精确。在这类 CPU 上以复核模式启动时，除非把 `ONEDNN_MAX_CPU_ISA` 设为 AMX 以下（`AVX512_CORE_BF16`），否则引擎拒绝启动。
- 提供者转发请求时把 `X-Request-Id` 设为市场请求 ID，因此 vLLM 的请求 ID（`chatcmpl-<X-Request-Id>-<后缀>`）能标识每个段。

## 前提条件

| | |
|---|---|
| vLLM | `>=0.30.0,<0.31`（插件加载时检查） |
| 模型运行器 | V1。vLLM 0.30 在 GPU 上默认使用 V2 运行器：请设置 `VLLM_USE_V2_MODEL_RUNNER=0`。CPU 后端使用 V1 |
| 精度 | `--dtype bfloat16`（TOPLOC 只支持 bfloat16） |
| 前缀缓存 | 关闭：`--no-enable-prefix-caching`（被缓存的 prompt token 不会重新计算） |
| 投机解码 | 关闭 |
| 并行 | 张量并行可以（由 rank 0 发送）；流水线并行会被拒绝 |

被引擎抢占后重算的请求带有证明（m6-public-jobs，I-008）：在重算它的那一步，prompt 范围内的行组成一段预填充，prompt 之后的每一行各为一段解码，使提供者能还原未被抢占时的候选。要求多个候选回答（`n > 1`）的请求由网关拒绝（留待以后的事项见 `docs/issues.md` 的 I-009 至 I-011）。

## 安装与运行

```bash
pip install ./plugins/vllm          # 与 vLLM 0.30.x 装在同一环境
export AGENTCOIN_TOPLOC_SOCKET=/run/agentcoin/toploc.sock   # 提供者的 --toploc-socket
VLLM_USE_V2_MODEL_RUNNER=0 vllm serve Qwen/Qwen2.5-0.5B-Instruct \
  --dtype bfloat16 --no-enable-prefix-caching
```

用相同的套接字路径启动提供者（`ac-provider run … --toploc-socket /run/agentcoin/toploc.sock`）；二者谁先启动都可以。审计员的复核引擎以同样方式运行，只是设置 `AGENTCOIN_TOPLOC_MODE=verify`，并使用 `ac-auditor recheck --socket` 的套接字。

## 隐私

插件只发送下标与激活值，而且只发往本机套接字。它不写任何文件，日志只包含连接状态、请求 ID 与计数；从不包含 prompt、输出、token 或激活值。

## 测试

```bash
pip install pytest torch   # 提取与运行器的测试需要 PyTorch
python -m pytest plugins/vllm
```

`tests/test_client.py` 用 Rust 的向量（`crates/ac-market-proto/tests/vectors/engine_protocol.json`）检查编码，并用假的提供者检查发送线程；`tests/test_extract.py` 与 `tests/test_runner.py` 用替身 vLLM 对象检查提取与包装逻辑。`scripts/check-vllm-plugin.py` 在真实的 CPU 版 vLLM 中运行插件，并把证明与参考实现比对（CI 任务 `vllm-plugin`）。
