> 🌐 [English](README.md) | **简体中文**

# ac-provider

AgentCoin 推理提供者代理（M5，规格 `market/provider-agent`）。它部署在任意 OpenAI 兼容推理引擎（vLLM、SGLang、llama.cpp server 等）之前，负责：

- 只接受链上已登记且活跃的网关发来的密封请求（`ac_crypto::sealed`，X-Wing + ML-DSA），且只处理提供者已登记、并在配置中映射到引擎的模型；
- 以流式并要求报告用量的方式转发给引擎，边生成边把每个块密封回传；
- 按引擎报告的 token 数与链上价格计费（向上取整到微美元），用提供者账户的 ML-DSA 密钥签署收据，并连同该请求的 TOPLOC 证明一起回传（见下文）；
- 保存网关追加签名后的收据及其证明，直到其挑战期结束；
- 每隔四分之三个心跳间隔发送一次心跳（总是免费），每个排放纪元领取一次已到期的款项；
- **从不记录或保存 prompt 与输出**：日志只包含请求 ID、模型 ID、token 数、费用、延迟与错误码，底层库的日志一律丢弃。

## 用法

命令示例见英文版 [README.md](README.md)：先用 `ac-provider keygen` 生成加密密钥并通过钱包登记，再在引擎旁运行 `ac-provider run`（钱包、口令文件、密钥文件、节点、引擎地址、`--model <模型 ID>=<引擎模型名>`、监听地址、数据目录与 `--toploc-socket`）。

启动时，若密钥文件的公钥与链上登记的不同，或映射的模型未登记，代理拒绝运行。登记的服务地址必须能到达 `--listen`；可以在反向代理处终止 TLS，但请求内容本身已经密封。

## TOPLOC 证明

设置 `--toploc-socket <路径>` 时，代理在该本机 Unix 套接字（权限 0600）上等待引擎的 TOPLOC 插件连接（`plugins/vllm`，规格 `market/engine-plugin`；启动 vLLM 时把 `AGENTCOIN_TOPLOC_SOCKET` 设为同一路径）。代理转发每个请求时把 `X-Request-Id` 设为市场请求 ID，收集插件为它发来的每步 top-k 候选（只保存在内存中），引擎应答完毕后最多等待 2 秒的插件结束标记。随后按引擎报告的用量检查候选（预填充元素数为 prompt token 数乘以隐藏维度；除最后一个输出 token 外，每个输出 token 一个解码步骤），用 `ac-toploc` 构造证明（top-k 128，解码每 32 步一块），并把承诺写入收据；证明随收据交给网关，并与双签收据一起保存。

未设置套接字、请求要求多个候选回答（`n > 1`）、插件未能及时发送结束标记、或候选与用量不符（例如引擎抢占后重算了请求）时，收据承诺为全零且不带证明，日志中记录一行 `toploc_missing`，只含请求 ID 前缀与原因。在 M6 审计另行决定之前，不带证明的收据仍然有效（问题 I-008）。

## 运维

- **时钟**：握手有效期为 ±120 秒，请启用 NTP。
- **接口**：`POST /ac/v1/sealed`（网关使用）、`GET /health`。
- **数据目录**：`receipts/` 保存双签收据及其 TOPLOC 证明（SCALE 十六进制，每个请求 ID 一个 JSON 文件），在挑战期结束三个排放纪元后删除。
- **引擎**：必须支持带 `stream_options.include_usage` 的 `stream: true`（vLLM、SGLang 与 llama.cpp 均支持）；没有给出用量就结束的流视为失败，不计费。

## Feature

无。
