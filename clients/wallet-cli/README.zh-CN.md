> 🌐 [English](README.md) | **简体中文**

# ac-wallet

AgentCoin 的命令行钱包（M1），也是 EVM 合约的外部签名器（M4）和推理市场客户端（M5）。

- 密钥：以 256 位熵为基础的 24 词 BIP-39 助记词；账户密钥按算法和序号派生（`agentcoin 2026-09 wallet-key v1`）。账户默认算法为 ML-DSA-44。
- 钱包文件只保存经 Argon2id + XChaCha20-Poly1305 加密的熵和公开的记账信息；口令从终端或 `--password-file` 读取，从不来自命令行参数。
- 地址为 bech32m 格式的 `atc1…` 字符串；输错任意一个字符都会被拒绝。
- 交易为由 `PqAuthorize` 授权的 v5 General 交易；账户的首笔交易会自动携带公钥。
- `rotate` 切换到下一个派生序号（可以换成另一个 ML-DSA 参数集），地址始终不变。

## 用法

```bash
ac-wallet new --wallet alice.json            # 输出地址，并且只显示一次助记词
ac-wallet import --wallet restored.json      # 从标准输入读取助记词
ac-wallet address --wallet alice.json
ac-wallet balance --wallet alice.json --node http://127.0.0.1:9944
ac-wallet transfer --wallet alice.json --to atc1... --amount 1.5
ac-wallet rotate --wallet alice.json --alg ml-dsa-65
```

`scripts/wallet-smoke.sh` 会针对开发节点执行上述命令。

## EVM 合约

合约交易是用账户 ML-DSA 密钥签名的 AgentCoin 交易：Foundry 负责编译和读取，钱包负责签名。账户的 EVM 地址为 `keccak256(account)[12..]`。

```bash
ac-wallet evm address --wallet alice.json    # EIP-55 格式，例如 0x05a0…9F70
forge build                                  # 在 Foundry 工程中执行
ac-wallet evm deploy --wallet alice.json --artifact out/ERC20.sol/ERC20.json \
  --constructor "constructor(string,string,uint256)" Token TKN 1000000000000000000000
ac-wallet evm send --wallet alice.json --to 0x… --sig "transfer(address,uint256)" 0x… 5
cast call 0x… "balanceOf(address)(uint256)" 0x… --rpc-url http://127.0.0.1:8545   # ac-eth-rpc
```

| 命令 | 作用 |
|---|---|
| `evm address` | 显示账户的 EVM 地址（EIP-55） |
| `evm deploy` | 部署 `forge build` 产物（`--artifact`）或初始化代码（`--bytecode`）；构造参数可在 `--constructor <签名>` 后以文本给出，或用 `--constructor-args` 给出编码结果 |
| `evm send` | 以 `--sig <签名>` 加参数或以 `--data` 调用合约 |
| `evm raw deploy`、`evm raw send` | 只签名不提交；输出交易哈希和可交给 `eth_sendRawTransaction` 的字节 |
| `evm broadcast --file run-latest.json` | 逐笔发送 `forge script` 模拟结果中的交易 |
| `evm sign-message` | 以 `agentcoin/evm-verify/v1` 为消息签名，供 `pq_verify` 预编译使用 |

签名前，钱包在最新状态上模拟执行交易：权重上限取所需值加 20%，存储押金上限取峰值加 10%（可用 `--ref-time-limit`、`--proof-size-limit`、`--deposit-limit` 覆盖）。模拟显示会回滚的交易不提交，并输出回滚原因（`--force` 可强制提交）。金额（`--value`、`--deposit-limit`）以 ATC 为单位。

`evm broadcast` 要求模拟时以钱包地址作为发送者（`forge script <脚本> --sender <EVM 地址> --rpc-url <适配器>`，不加 `--broadcast`）；遇到第一笔失败的交易、或第一笔创建地址与脚本预测不同的部署时立即停止，后续交易不再发送。注意：原生部署会使账户 nonce 增加 2（交易本身一次，`pallet-revive` 一次），因此脚本只有第一笔部署的地址预测正确。

## 推理市场

`ac-wallet market …` 用于在推理市场（M5）中登记和操作：模型、提供者、网关、托管与凭证。美元金额为最多 6 位小数的十进制数（如 `0.5`）；ATC 金额沿用通常格式。交易失败时输出链上的错误名称，例如 `Providers(BelowThreshold)`。

```bash
ac-wallet market rate                                            # 参考汇率（每美元多少 ATC）
ac-wallet market model id --file model.json                      # 离线计算模型 ID
ac-wallet market model register --wallet alice.json --file model.json
ac-wallet market provider register --wallet p.json --tier t2 --endpoint https://p.example \
  --kem-key 0x01… --model 0x<模型 ID>:0.1:0.2                     # 每百万输入:输出 token 的美元价格
ac-wallet market providers --model 0x<模型 ID>                   # 可服务提供者
ac-wallet market gateway register --wallet g.json --endpoint https://g.example --fee-bps 300
ac-wallet market escrow deposit --wallet alice.json --gateway atc1… --amount 5
ac-wallet market voucher sign --wallet alice.json --gateway atc1… --usd 0.5
ac-wallet market voucher check --voucher 0x…
```

| 命令 | 作用 |
|---|---|
| `market model register --file`、`model show --id`、`model id --file` | 从清单文件登记模型（提交前输出模型 ID）、查询模型，或离线计算 ID |
| `market provider register` | `--tier t1\|t2`、`--endpoint`、`--kem-key`（带 AlgId 的 X-Wing 公钥的十六进制）、一个或多个 `--model ID:输入美元:输出美元`、`--stake`（默认取当前门槛） |
| `market provider update`、`heartbeat`、`bond`、`unbond`、`exit`、`withdraw`、`show` | 修改地址、公钥或模型；心跳；追加或解绑质押；退出；取回到期质押；查询提供者及其是否可服务 |
| `market providers --model` | 某模型的全部可服务提供者 |
| `market gateway register`、`update`、`bond`、`unbond`、`exit`、`withdraw`、`show` | 网关的同类操作（`--fee-bps` 不超过 500） |
| `market escrow deposit`、`request-withdrawal`、`withdraw`、`change-key` | 向网关托管 ATC；等待期后取回；等待期后把钱包当前公钥设为通道的凭证密钥 |
| `market channel --gateway` | 通道的托管额、通道号、已兑付额、凭证密钥与待生效的申请 |
| `market voucher sign --gateway --usd` | 签发累计式凭证（`agentcoin/voucher/v1`）并输出其十六进制 |
| `market voucher check --voucher` | 以与兑付完全相同的规则在链上校验凭证 |

清单文件示例：

```json
{ "name": "Qwen2.5-0.5B-Instruct", "arch": "qwen2", "quant": "int4",
  "shards": ["0x…第 1 个分片的 32 字节 BLAKE3…", "0x…"],
  "lineage": { "parent": "0x<模型 ID>", "kind": "quantize" },
  "licenseTag": "apache-2.0" }
```

`quant` 取 `bf16`、`fp16`、`fp8`、`int8`、`int4` 之一；`lineage`（派生方式 `finetune`、`quantize`、`distill`、`merge`）与 `licenseTag` 可省略。

## 工作结算

提供者与网关对每次请求各签一条收据；网关提交工作报告，挑战期（两个排放纪元）过后各方领取（`pallet-work`）。收据文件是 JSON：收据内容的十六进制编码，加上两方各自的公钥与签名；其中从不包含 prompt、输出或用户。

```bash
ac-wallet market receipt new --wallet p.json --gateway atc1… --provider atc1… \
  --model 0x<模型 ID> --in-tokens 1000 --out-tokens 2000 --out r1.json    # 费用按链上价格计算
ac-wallet market receipt cosign --wallet g.json --file r1.json
ac-wallet market receipt check --file r1.json
ac-wallet market report submit --wallet g.json --receipt r1.json --receipt r2.json --voucher 0x…
ac-wallet market report show --id 0
ac-wallet market claim --wallet any.json --account atc1…                  # 领取全部已到期款项
ac-wallet market work --account atc1…                                     # 冻结款项与工作量
ac-wallet market work --epoch 12                                          # 某纪元的已核验工作量
```

| 命令 | 作用 |
|---|---|
| `market receipt new` | 构造收据（费用 = 提供者对该模型的价格，向上取整到微美元；`--toploc`、`--request-id`、`--ttft-ms`、`--total-ms` 可选），并以提供者或网关身份签名 |
| `market receipt cosign --file` | 以另一方身份追加签名 |
| `market receipt check --file` | 按链上的公钥与价格校验两方签名、创世哈希与费用 |
| `market report submit` | 计算 Merkle 根和按（提供者, 模型）的汇总，逐条校验收据并核对汇总等于凭证增量后提交；不一致时指出出错的文件或凭证，不提交任何交易 |
| `market report show --id` | 已存储的报告及其份额与工作量 |
| `market claim [--account] [--epochs]` | 为某账户（默认钱包自己的）领取已结算纪元的全部冻结款项与排放份额 |
| `market work [--account \| --epoch]` | 某账户的累计工作量、冻结款项与未领取的工作量，或某纪元的已核验工作量与市场排放 |

## 本地推理代理

`ac-wallet market serve` 在本机提供 OpenAI 兼容接口（`GET /v1/models`、`POST /v1/chat/completions`，流式与非流式），用你的额度通道向网关付费，使未修改的 OpenAI SDK 可以直接使用推理市场。命令与 Python 示例见英文版 [README.md](README.md)：先托管额度，再运行 `market serve --gateway <网关地址> --max-usd 5`，然后把 OpenAI SDK 的 `base_url` 指向 `http://127.0.0.1:8411/v1`（模型可写名称或 `0x…` ID）。

- 启动时检查：你与该网关有额度通道、钱包密钥是通道的凭证密钥、网关的加密公钥由网关账户签名。
- 每个请求都密封给网关（X-Wing + ML-DSA），并附带恰好等于你已付总额的凭证。响应结束后，代理校验双签收据（签名、链、网关、模型、token 数与返回的 `usage` 一致、费用等于提供者的链上价格、网关的已计费总额）及其 TOPLOC 证明（承诺为全零时不得带证明；否则参数为市场参数、每块输出一份证明、复算承诺等于收据中的承诺），通过后才付清该总额。任何一项校验失败，代理都会停止后续所有付款与请求。证明不会被保存。
- `--max-usd` 限制通道的累计付款；已付总额保存在钱包旁的 `serve-<网关>.json`（`--state`），重启后不会重复付款。
- 默认只监听 `127.0.0.1:8411`，且没有鉴权：切勿对外暴露。私钥从不离开本机，代理也不记录 prompt 或输出。

## 库示例

示例代码见英文版 [README.md](README.md)（英文版中的示例作为 doctest 运行）：在 ATC 小数字符串与最小单位之间互相转换。
