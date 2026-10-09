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
# 复核引擎：带插件的 vLLM，复核模式，同一模型，bfloat16。在 CPU 上让 oneDNN 不使用 AMX
# 内核（阈值按此校准；在支持 AMX 的 CPU 上插件会强制检查）。
ONEDNN_MAX_CPU_ISA=AVX512_CORE_BF16 \
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

### 证据（链上审计）

链上不通过的裁决承诺的是这次复核的证据（m6-audit-chain；规格 `market/auditor-agent`“审计证据”）。`ac-auditor evidence --case c.json --out e.bin` 写出证据（SCALE 字节：messages、回答、用量、收据、证明），并打印 `{"commitment": "<hex>"}`。争议的复核人用 `ac-auditor recheck --evidence e.bin --commitment <链上的十六进制承诺> --engine-model <名称> …` 复核：字节与承诺不符时输出 `{"outcome": "mismatch", …}`，不调用任何引擎；相符时按其案例复核。每一行复核结果还带有 `onchain`：链上编码的结果（十六进制），供 `ac-wallet audit verdict` 提交。

### 代理（`ac-auditor run`）

服务模式就是 MVP 方案 §5.5 的神秘顾客（m6-auditor-agent；规格 `market/auditor-agent`“审计员代理服务”及其后各条）。它以一个已登记的审计员账户运行（`ac-wallet audit register`），无需人工操作：

- **审计**：对本轮被分配的每个提供者审计一次，时刻随机，落在本轮前四分之三内的某个区块：
  - 发一个指定该提供者的非流式请求（`X-AgentCoin-Provider`，对所有用户开放），经随机的网关和随机的付款账户；
  - prompt 来自内置生成器或运营者的题库；
  - 模型从该提供者的模型中挑本代理有引擎的那一个。
  
  在本轮结束前复核并提交裁决。不通过时先保存证据，再提交裁决。
- **交付证据**：在启动时连同 X-Wing 公钥登记到链上的证据地址提供服务。只经密封通道把某条裁决的证据交给本审计员作为提出者的未关闭争议的复核人；拒绝时不说明任何原因。没有争议还能用到的证据会被删除；本审计员参与且已到期的争议由它关闭。
- **复核**：被抽为复核人时，取回每名提出者的证据，与链上的承诺和收据核对后复核。只有至少两名不同提出者的证据复核为不通过才投“确认”，否则投“驳回”。本机引擎不可用时重试，到期仍不可用则不投票。

```bash
ac-auditor keygen --out auditor-kem.json --password-file auditor.pass
ac-auditor run --config auditor.json
```

配置是 JSON 文件，相对路径以文件所在目录为基准（字段见英文版示例）。

运维说明：

- **付款账户**：每个付款账户都必须事先在所列的每个网关有透明额度通道（托管），且绝不能是审计员账户（启动时会拒绝）。网关看得到付款账户，托管存入在链上公开，因此要用与审计员无关的资金来源充值并定期轮换。M7 的匿名凭证将取代这一做法。
- **引擎**：每个模型一个复核模式的引擎（见上文，CPU 上关闭 AMX）。只提供本代理没有引擎的模型的提供者会被跳过并计数。
- **阈值**：代理按内置的 `AUDIT_THRESHOLDS` 版本判定。链上接受的版本与之不同时，代理既不审计也不投票，并说明原因：请升级。
- **题库**：JSONL，每行一个 `messages` 数组；`bank_percent` 比例的 prompt 取自题库。
- **prompt 长度**：只发出 token 数（按模型的对话模板，由代理的复核引擎计数）落在 `AUDIT_THRESHOLDS` 审计长度区间内的 prompt（150–300 个 token，含两端）。启动时按模型对题库逐条计数，区间外的条目被跳过（日志给出条数）；某个模型没有任何条目在区间内时拒绝启动。生成的 prompt 低于区间时用随机抽取的背景句加长，高于区间时重新生成；计数 20 次仍不在区间内则跳过这次审计并计数。每次审计在日志中记录 prompt 的 token 数，从不记录其内容。

## 隐私

复核只使用审计员自己的请求。代理的日志只包含请求 ID、模型 ID、计数、指标与判定结果；从不包含消息、输出、token 或候选，底层库的日志行一律丢弃。案例的 `Debug` 输出不含消息与输出。

## 测试

`cargo test -p ac-auditor` 复核确定性模拟引擎（`tests/mock-engine`）的回答：诚实提供者、换了模型、没有证明、int4 模型、不符的证明、输入与 token、引擎不在线，以及连到审计员套接字的证明模式插件。CI 任务 `vllm-plugin` 复核真实 vLLM 的回答（`scripts/calibrate-toploc.py --quick`）。代理的决策（`src/agent`）用链、网关、引擎与提出者的替身测试；`tests/e2e/tests/auditor_agent.rs` 让六个代理面对一个中途开始作弊的提供者（`AC_E2E=1 cargo test -p ac-e2e --test auditor_agent -- --test-threads 1`）。
