> 🌐 [English](README.md) | **简体中文**

# ac-wallet

AgentCoin 的命令行钱包（M1），也是 EVM 合约的外部签名器（M4）。

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

## 库示例

示例代码见英文版 [README.md](README.md)（英文版中的示例作为 doctest 运行）：在 ATC 小数字符串与最小单位之间互相转换。
