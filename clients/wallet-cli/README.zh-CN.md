> 🌐 [English](README.md) | **简体中文**

# ac-wallet

AgentCoin 的命令行钱包（M1）。

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

## 库示例

示例代码见英文版 [README.md](README.md)（英文版中的示例作为 doctest 运行）：在 ATC 小数字符串与最小单位之间互相转换。
