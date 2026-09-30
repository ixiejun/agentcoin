> 🌐 [English](m5-inference-acceptance.md) | **简体中文**

# 使用真实模型的 M5 推理验收

CI 的端到端测试（`tests/e2e/tests/inference.rs`）用模拟引擎运行整个市场。本说明在 GPU 机器上用真实模型手动重复这一过程，对应 MVP 方案（§10）的 M5 验收：OpenAI SDK 经市场流式调用 Qwen，额外首 token 延迟不超过 150 毫秒，收据完成结算。

## 前置条件

- 带 NVIDIA GPU 的 Linux 机器（Qwen2.5-0.5B-Instruct 用 8 GB 显存即可），Python 3.10 及以上。
- 已安装 `vllm`（`pip install vllm`）；任何支持 `stream_options.include_usage` 的 OpenAI 兼容引擎都可以同样使用。
- 已构建本仓库：`cargo build --release -p ac-node -p ac-wallet -p ac-provider -p ac-gateway`。
- `pip install -r tests/e2e/python/requirements.txt`（官方 `openai` 包）。

## 步骤

完整命令见英文版 [m5-inference-acceptance.md](m5-inference-acceptance.md)，依次为：

1. 启动开发链（`ac-node --dev --tmp`）。
2. 启动引擎（`vllm serve Qwen/Qwen2.5-0.5B-Instruct --port 8000`）。
3. 创建钱包：开发账户为提供者、网关与用户转入资金。
4. 登记模型（清单中是真实的分片哈希）；用 `ac-provider keygen` 生成密钥，登记提供者并运行 `ac-provider run`（`--engine http://127.0.0.1:8000`，`--model <模型 ID>=Qwen/Qwen2.5-0.5B-Instruct`）。
5. 用 `ac-gateway keygen` 生成密钥，登记网关并运行 `ac-gateway run --report-interval 5`。
6. 用户托管额度，并运行本地代理 `ac-wallet market serve --max-usd 1`。

## 检查项

1. **经 OpenAI SDK 流式调用与延迟**：用同一个客户端分别直连引擎与经代理访问，比较首 token 时间的 p95。命令见英文版。两者 `ttft_ms.p95` 之差必须不超过 150 毫秒；记录两个值、GPU 型号与构建方式（release）。
2. **结算**：几个区块后，`ac-wallet market report show --id 0` 显示网关提交的报告；挑战期（两个排放纪元）结束后，`ac-wallet market work --account <提供者地址>` 显示已领取的款项（提供者代理会自动领取）。
3. **隐私**：`ac-provider`、`ac-gateway` 与 `ac-wallet market serve` 的日志中不含任何 prompt 或输出文本，`--log-level debug` 时也一样。

本阶段收据中的 TOPLOC 承诺为全零；引擎内证明在下一个变更中实现。
