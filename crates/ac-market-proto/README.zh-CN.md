> 🌐 [English](README.md) | **简体中文**

# ac-market-proto

AgentCoin 推理市场的线协议：钱包本地代理、网关（`ac-gateway`）与提供者（`ac-provider`）在密封通道（`ac_crypto::sealed`）中交换的内容。本 crate 不做 I/O，采用宽松许可，将来的客户端 SDK 可以直接复用。

## HTTP 传输

- `POST /ac/v1/sealed`，`Content-Type: application/x-agentcoin-sealed`。请求体是一串帧（`frame`）：4 字节大端长度加载荷，单帧至多 128 KiB。第一帧是签名握手，其余是密封的请求块。响应体以同样的分帧流式传回密封的响应块。
- `GET /ac/v1/key`（网关）：网关的封装公钥，由网关账户密钥签名（`announce::GatewayKey`，上下文 `agentcoin/gateway-kem/v1`）。客户端先用该账户的链上公钥核对，再向它密封任何内容。

## 消息

每条消息都是版本字节（`PROTOCOL_VERSION` = 1）加 SCALE 编码。枚举序号即线格式，永不改变。

| 方向 | 消息 | 序号 | 含义 |
|---|---|---|---|
| 用户 → 网关 | `UserMsg::Chat` | 0 | Chat Completions 请求（JSON）与一张恰好等于通道已计费总额的凭证 |
| | `UserMsg::Pay` | 1 | 单独的凭证，在收到账单后发送 |
| 网关 → 用户 | `GatewayMsg::Delta` | 0 | 一个流式块（JSON） |
| | `GatewayMsg::Completion` | 1 | 完整的非流式响应（JSON） |
| | `GatewayMsg::Billing` | 2 | 双签收据、本次费用与新的已计费总额 |
| | `GatewayMsg::Error` | 3 | 错误码与说明（从不包含请求内容） |
| | `GatewayMsg::Paid` | 4 | 付款已被接受 |
| 网关 → 提供者 | `ProviderReq::Infer` | 0 | 请求 ID、任务类型、模型 ID、请求（JSON） |
| | `ProviderReq::Cosigned` | 1 | 网关追加签名后的收据，供提供者留存 |
| 提供者 → 网关 | `ProviderMsg::Delta` | 0 | 一个流式块（JSON） |
| | `ProviderMsg::Receipt` | 1 | 提供者签名的收据与引擎报告的用量 |
| | `ProviderMsg::Error` | 2 | 错误码与说明 |
| | `ProviderMsg::Ack` | 3 | 确认收到 `Cosigned` |

`Payment` 目前只有 `Transparent`（序号 0，累计式通道凭证，D54）；序号 1 预留给屏蔽凭证。

## OpenAI 子集

`openai::ChatRequest` 把请求保持为 JSON 对象，只改写 `model`、`stream`（同时加上 `stream_options.include_usage`）与输出 token 上限，其余字段原样透传。`input_bound` 以消息的 UTF-8 字节数加每条消息 16 个 token 作为输入 token 上界；`route::max_fee` 据此算出请求的最大可能费用。解析错误从不引用输入内容，请求的 `Debug` 输出只显示模型。`SseDecoder`、`sse_data`、`usage_of`、`has_content` 与 `Assembler` 处理流式响应；`error_json` 生成 OpenAI 格式的错误。

## 路由

`route::RouteScore` 为候选提供者排序；默认的 `PriceThenLatency` 先按价格从低到高，再按平滑后的首 token 延迟从低到高。SLA 与声誉在 M6 审计之后加入。

## 示例

示例代码见英文版 [README.md](README.md)（作为 doctest 运行）：把一条 `GatewayMsg` 编码为帧，分两次喂给解码器，再解出原消息。
