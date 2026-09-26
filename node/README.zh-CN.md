> 🌐 [English](README.md) | **简体中文**

# ac-node

AgentCoin 节点客户端（M1：抗量子链）。

- **共识**：Aura-PQ（`ac-consensus-aura-pq`）：ML-DSA-65 区块封印，时隙 1 秒，授权节点来自创世配置。M1 没有终局性组件（GRANDPA 写死为 Ed25519），分叉选择采用最长链，AC-BFT 终局性在 M2 引入。
- **哈希**：区块与状态都使用 BLAKE3-256。
- **宪法第 1 层**：链类型为 `Live` 且创世分配了任何 ATC 的链规格在启动时被拒绝（无预挖，D9）；没有 Aura-PQ 授权节点的链同样被拒绝。
- **密钥**：授权密钥来自加密文件（`--pq-key-file` + `--pq-password-file`），或仅在开发链和本地链上来自公开的开发名称（`--dev-key alice`；`--dev` 默认使用 alice）。正式链上拒绝开发密钥。

## 运行

```bash
# 单节点开发链（授权节点 alice，开发账户带余额）。
cargo build --release -p ac-node
target/release/ac-node --dev --tmp

# 三节点本地测试网（alice、bob、charlie）；RPC 分别在 127.0.0.1:9944/9945/9946。
scripts/run-local-testnet.sh
# 同上，但 40 秒后检查每个节点高度都达到 20，然后退出。
scripts/run-local-testnet.sh --check

# 生成加密的授权密钥；输出的公钥用于创世授权节点列表。
target/release/ac-node pq-key generate --output authority.json --password-file password.txt
target/release/ac-node --chain my-spec.json --validator \
  --pq-key-file authority.json --pq-password-file password.txt
```

只有 `node/` 可以依赖许可证为 `GPL-3.0-or-later WITH Classpath-exception-2.0` 的 Polkadot SDK 客户端 crate（决策 D37）；节点自身的源码为 MIT 许可证。分发 `ac-node` 二进制时须遵守这些组件的 GPL 条款。
