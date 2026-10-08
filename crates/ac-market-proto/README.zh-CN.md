> 🌐 [English](README.md) | **简体中文**

# ac-market-proto

AgentCoin 推理市场的线协议：钱包本地代理、网关（`ac-gateway`）与提供者（`ac-provider`）在密封通道（`ac_crypto::sealed`）中交换的内容。本 crate 不做 I/O，采用宽松许可，将来的客户端 SDK 可以直接复用。

## HTTP 传输

- `POST /ac/v1/sealed`，`Content-Type: application/x-agentcoin-sealed`。请求体是一串帧（`frame`）：4 字节大端长度加载荷，单帧至多 128 KiB。第一帧是签名握手，其余是密封的请求块。响应体以同样的分帧流式传回密封的响应块。
- `GET /ac/v1/key`（网关）：网关的封装公钥，由网关账户密钥签名（`announce::GatewayKey`，上下文 `agentcoin/gateway-kem/v1`）。客户端先用该账户的链上公钥核对，再向它密封任何内容。

## 消息

每条消息都是版本字节（`PROTOCOL_VERSION` = 2；版本 1 不带 TOPLOC 证明，已不再接受）加 SCALE 编码。枚举序号即线格式，永不改变。

| 方向 | 消息 | 序号 | 含义 |
|---|---|---|---|
| 用户 → 网关 | `UserMsg::Chat` | 0 | Chat Completions 请求（JSON）与一张恰好等于通道已计费总额的凭证 |
| | `UserMsg::Pay` | 1 | 单独的凭证，在收到账单后发送 |
| 网关 → 用户 | `GatewayMsg::Delta` | 0 | 一个流式块（JSON） |
| | `GatewayMsg::Completion` | 1 | 完整的非流式响应（JSON） |
| | `GatewayMsg::Billing` | 2 | 双签收据、本次费用、新的已计费总额，以及 TOPLOC 证明（若有） |
| | `GatewayMsg::Error` | 3 | 错误码与说明（从不包含请求内容） |
| | `GatewayMsg::Paid` | 4 | 付款已被接受 |
| 网关 → 提供者 | `ProviderReq::Infer` | 0 | 请求 ID、任务类型、模型 ID、请求（JSON） |
| | `ProviderReq::Cosigned` | 1 | 网关追加签名后的收据，供提供者留存 |
| 提供者 → 网关 | `ProviderMsg::Delta` | 0 | 一个流式块（JSON） |
| | `ProviderMsg::Receipt` | 1 | 收据、提供者公钥与签名、引擎报告的用量，以及 TOPLOC 证明（若有） |
| | `ProviderMsg::Error` | 2 | 错误码与说明 |
| | `ProviderMsg::Ack` | 3 | 确认收到 `Cosigned` |

`Payment` 目前只有 `Transparent`（序号 0，累计式通道凭证，D54）；序号 1 预留给屏蔽凭证。

## TOPLOC 证明

`toploc::ToplocProofs` 携带一次推理的证明及其参数（规格 `market/toploc`）。市场使用 `MARKET_PARAMS`：top-k 128、解码每 32 步一块、预填充单独一块，因此输出 `n` 个 token 的推理有 `1 + ⌈(n − 1) / 32⌉` 份各 258 字节的证明。`toploc::check` 是网关与钱包代理对每张收据运行的检查：承诺为全零时不得带证明；否则参数、证明份数、编码与复算出的承诺都必须与收据一致。

`toploc::fit_segments` 在构造证明之前，把插件为一个请求发来的段与其用量对齐（规格 `market/provider-agent`）：预填充元素数为 prompt token 数乘以隐藏维度，其后除最后一个输出 token 外每个输出 token 一个单行解码段。恰好多出一个解码段，说明引擎在得知回答结束之前把最后一个 token 又喂回了模型（异步调度，I-022）：丢弃这一行。其他数目返回 `SegmentsError`。提供者与校准案例都使用它。

## 引擎插件协议

`engine` 是推理引擎插件与接收方之间的本机协议（规格 `market/engine-plugin`，版本 `ENGINE_PROTOCOL_VERSION` = 2）：帧为 4 字节大端长度加固定的大端布局（至多 1 MiB），Python 插件用 `struct` 即可编码。插件发送 `Hello`（魔数 `ACTL`、版本、隐藏维度、模式），接收方回复 `Welcome`（版本、top-k）；之后插件发送 `Segment`（引擎请求 ID、阶段、元素个数、top-k 候选），每个请求结束时发送 `Finish`。**证明**模式（模式 0）下，提供者为每个前向步骤的每个请求收到一段，用于构造证明；**复核**模式（模式 1）下，审计员的复核程序为预填充的每个 token 行收到一段。`answer_hello` 是接收方的握手：版本不同的插件收到接收方的版本，模式不被接受的插件收到 top-k 为 0，两种情况都随后断开连接（不带模式字节的版本 1 `Hello` 仍可解码，以便回复）。`market_request_id` 从引擎请求 ID 中找出市场请求 ID：接收方提交请求时把 `X-Request-Id` 设为它的 64 位小写十六进制，vLLM 把请求命名为 `chatcmpl-<X-Request-Id>-<后缀>`（对话）或 `cmpl-<X-Request-Id>-<后缀>`（补全）。与插件测试共用的字节级向量在 `tests/vectors/engine_protocol.json`。

## 审计阈值

`toploc::judge` 是审计对 `ac_toploc::compare` 逐块结果的通过 / 不通过规则（规格 `market/toploc`“复核判定规则与阈值”）。`Thresholds` 包含两组 `ChunkBounds`：预填充块用严格的一组（prompt 由审计员确切知道，并以与提供者相同的方式复算），解码块用较宽的一组。一块通过，当且仅当指数不一致次数、尾数误差均值（以百分之一为单位，按“和 × 100 ≤ 上限 × 项数”计算）与尾数误差中位数都不超过所属一组的界限，且至少有一项指数相同；一次推理只有每块都通过才通过，否则在第一个超限的块上判不通过。`AUDIT_THRESHOLDS` 带版本号，取值来自 `scripts/calibrate-toploc.py` 的校准。

## 审计证据

`audit::AuditEvidence` 是不通过的审计裁决所承诺的内容（m6-audit-chain）：请求的 messages（紧凑 JSON）、回答、结束原因与用量、双签收据和 TOPLOC 证明。证据文件即其 SCALE 编码；承诺为 `derive("agentcoin 2026-10 audit-evidence v1", 字节)`，上链的只有承诺，证据留在审计员处（红线 6）。`AuditEvidence::open` 先按承诺检查字节再解码。`audit::evidence_from_case` 由 `ac-auditor` 的案例构造证据，使钱包与审计员算出相同的承诺。回归向量在 `tests/vectors/audit_evidence.json`。

## OpenAI 子集

`openai::ChatRequest` 把请求保持为 JSON 对象，只改写 `model`、`stream`（同时加上 `stream_options.include_usage`）与输出 token 上限，其余字段原样透传。`input_bound` 以消息的 UTF-8 字节数加每条消息 16 个 token 作为输入 token 上界；`route::max_fee` 据此算出请求的最大可能费用。解析错误从不引用输入内容，请求的 `Debug` 输出只显示模型。`SseDecoder`、`sse_data`、`usage_of`、`has_content` 与 `Assembler` 处理流式响应；`error_json` 生成 OpenAI 格式的错误。

## 路由

`route::RouteScore` 为候选提供者排序；默认的 `PriceThenLatency` 先按价格从低到高，再按平滑后的首 token 延迟从低到高。SLA 与声誉在 M6 审计之后加入。

## 示例

示例代码见英文版 [README.md](README.md)（作为 doctest 运行）：把一条 `GatewayMsg` 编码为帧，分两次喂给解码器，再解出原消息。
