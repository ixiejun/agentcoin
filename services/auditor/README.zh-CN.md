> 🌐 [English](README.md) | **简体中文**

# ac-auditor

AgentCoin 审计员代理（M6，MVP 方案 §5.5；规格 `market/auditor-agent`）。审计员只复核**自己**以普通用户身份发起的推理，因此任何用户的内容都不会进入审计。本版本一次复核一个已完成的推理；神秘顾客请求、抽样与判定上链由 M6 后续变更加入。

一次复核：

1. 检查双签收据与这次回答一致（模型、prompt 与输出 token 数），否则结果为**无法判定（输入不一致）**；
2. 只复核登记为 **bfloat16** 的模型；其他精度为**无法判定（精度不支持）**，永远不会判为不通过；
3. TOPLOC 承诺为全零时判**不通过（没有证明）**：审计请求没有证明，无论原因都不通过；证明与承诺不符，或参数、份数不是市场规定的，也不通过；
4. 用自己的引擎重现 token：prompt 由消息按模型的对话模板得到（`/tokenize`），输出由其文本得到（`/tokenize`，再经 `/detokenize` 必须得到相同的文本）。prompt 的 token 数必须等于收据中的值；回答因结束符停止（`stop`，结束符不在文本中）时，输出的 token 数必须比收据少 1，达到长度上限（`length`）时必须相等；否则结果为**无法判定（无法重现 token）**；
5. 经 `/v1/completions`、带随机的 `X-Request-Id`，预填充 prompt 与除最后一个之外的全部输出 token（最后一个 token 从不经过前向计算）；引擎中处于**复核**模式的 TOPLOC 插件为每个 token 行向审计员的套接字发送一段候选；
6. 由这些行重建原推理的分块（prompt 的各行为预填充，其后每行为一个解码步骤），与证明比对（`ac_toploc::compare_from_candidates`），并按市场带版本的阈值判定（`ac_market_proto::toploc::judge`）：**通过**，或在第一个超限的块上**不通过**。引擎出错或缺少行时为**无法判定（复核引擎出错）**。

## 用法

```bash
# 复核引擎：带插件的 vLLM，复核模式，同一模型，bfloat16。
AGENTCOIN_TOPLOC_SOCKET=/run/agentcoin/recheck.sock AGENTCOIN_TOPLOC_MODE=verify \
VLLM_USE_V2_MODEL_RUNNER=0 vllm serve Qwen/Qwen2.5-0.5B-Instruct \
  --served-model-name qwen --dtype bfloat16 --no-enable-prefix-caching --port 8000

# 复核一个案例，或一个目录下的全部 *.json；每个案例输出一行 JSON。
ac-auditor recheck --engine http://127.0.0.1:8000 --socket /run/agentcoin/recheck.sock \
  --node http://127.0.0.1:9944 --cases cases/
```

| 参数 | 含义 |
|---|---|
| `--case <file>` / `--cases <dir>` | 一个案例，或一个目录下按文件名排序的全部 `*.json` |
| `--engine <url>` | 复核引擎 |
| `--engine-model <name>` | 引擎上的模型名，代替各案例的 `engine_model` |
| `--socket <path>` | 引擎插件发送到的套接字（创建时只有所有者可读写） |
| `--node <url>` / `--quant <p>` | 从节点读取各模型登记的精度，或直接给出（`bf16`、`fp16`、`fp8`、`int8`、`int4`） |
| `--connect-wait <s>` | 等待插件连接的秒数（默认 120） |
| `--log-level <l>` | 日志级别；日志行从不包含请求内容 |

案例（`RecheckCase`）为 JSON：

```json
{
  "model": "0x<链上模型 ID>",
  "engine_model": "qwen",
  "messages": [{"role": "user", "content": "..."}],
  "output": "回答的文本",
  "finish_reason": "length",
  "usage": {"prompt_tokens": 40, "completion_tokens": 24},
  "receipt": "<SCALE 编码的 SignedReceipt，十六进制>",
  "toploc": "<SCALE 编码的 ToplocProofs，十六进制，或 null>"
}
```

每行输出包含 `case`、`outcome`（`pass`、`fail`、`inconclusive`）、`reason`、`request`（收据的请求 ID）、`thresholds_version` 与 `chunks`（每块的指数不一致次数、尾数误差之和与项数、中位数）。

`ac-auditor calibration-case` 把证明方引擎的候选（与上面相同的 JSON，只是用 `segments: [{phase, len, candidates: [[index, bits], …]}]` 或 `null` 代替 `receipt` 与 `toploc`）转为一个案例，其收据由本次运行生成的密钥签名。它用于阈值校准（`scripts/calibrate-toploc.py`）与测试，不用于审计。

## 隐私

复核只使用审计员自己的请求。代理的日志只包含请求 ID、模型 ID、计数、指标与判定结果；从不包含消息、输出、token 或候选，底层库的日志行一律丢弃。案例的 `Debug` 输出不含消息与输出。

## 测试

`cargo test -p ac-auditor` 复核确定性模拟引擎（`tests/mock-engine`）的回答：诚实提供者、换了模型、没有证明、int4 模型、不符的证明、输入与 token、引擎不在线，以及连到审计员套接字的证明模式插件。CI 任务 `vllm-plugin` 复核真实 vLLM 的回答（`scripts/calibrate-toploc.py --quick`）。
