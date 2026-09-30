> 🌐 [English](README.md) | **简体中文**

# ac-provider

AgentCoin 推理提供者代理（M5，规格 `market/provider-agent`）。它部署在任意 OpenAI 兼容推理引擎（vLLM、SGLang、llama.cpp server 等）之前，负责：

- 只接受链上已登记且活跃的网关发来的密封请求（`ac_crypto::sealed`，X-Wing + ML-DSA），且只处理提供者已登记、并在配置中映射到引擎的模型；
- 以流式并要求报告用量的方式转发给引擎，边生成边把每个块密封回传；
- 按引擎报告的 token 数与链上价格计费（向上取整到微美元），用提供者账户的 ML-DSA 密钥签署收据并回传；本阶段 TOPLOC 承诺为全零（引擎内证明在下一个变更）；
- 保存网关追加签名后的收据，直到其挑战期结束；
- 每隔四分之三个心跳间隔发送一次心跳（总是免费），每个排放纪元领取一次已到期的款项；
- **从不记录或保存 prompt 与输出**：日志只包含请求 ID、模型 ID、token 数、费用、延迟与错误码，底层库的日志一律丢弃。

## 用法

命令示例见英文版 [README.md](README.md)：先用 `ac-provider keygen` 生成加密密钥并通过钱包登记，再在引擎旁运行 `ac-provider run`（钱包、口令文件、密钥文件、节点、引擎地址、`--model <模型 ID>=<引擎模型名>`、监听地址与数据目录）。

启动时，若密钥文件的公钥与链上登记的不同，或映射的模型未登记，代理拒绝运行。登记的服务地址必须能到达 `--listen`；可以在反向代理处终止 TLS，但请求内容本身已经密封。

## 运维

- **时钟**：握手有效期为 ±120 秒，请启用 NTP。
- **接口**：`POST /ac/v1/sealed`（网关使用）、`GET /health`。
- **数据目录**：`receipts/` 保存双签收据（SCALE 十六进制，每个请求 ID 一个 JSON 文件），在挑战期结束三个排放纪元后删除。
- **引擎**：必须支持带 `stream_options.include_usage` 的 `stream: true`（vLLM、SGLang 与 llama.cpp 均支持）；没有给出用量就结束的流视为失败，不计费。

## Feature

无。
