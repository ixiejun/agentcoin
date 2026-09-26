> 🌐 [English](README.md) | **简体中文**

# ac-node

AgentCoin 节点客户端（M2：带终局性的抗量子链）。

- **出块**：Aura-PQ（`ac-consensus-aura-pq`）：ML-DSA-65 区块封印，时隙 1 秒，授权节点来自验证人集合模块，只在纪元边界变更。
- **终局性**：AC-BFT（`ac-consensus-bft`），两阶段 BFT 组件，投票使用 ML-DSA-65（`agentcoin/bft-vote/v1`）。`n = 3f + 1` 个验证人中至多 `f` 个故障或离线时仍能最终确定；超过时继续出块但终局性暂停。每个变更授权集合的区块以及至少每 64 个区块保存一份终局性证明（commit 票集合）；同步的节点用应当签名的集合验证这些证明。验证人先持久化投票再发送，因此重启后不会重复签名。
- **双签**：同一密钥在同一时隙封印的两个区块头（由区块验证器发现），或两张冲突的 AC-BFT 投票（由终局性组件发现），会通过运行时构造成无签名举报交易，提交到本地交易池。链上记录违规，并在下一个纪元边界把违规者移出集合（M2 不罚没质押）。
- **随机数**：出块者以 inherent 数据提供 commit–reveal 秘密值；纪元 `e` 的随机数在纪元 `e + 2` 开始时公布。秘密值由验证人密钥派生，不会出现在日志中。
- **哈希**：区块与状态都使用 BLAKE3-256。
- **宪法第 1 层**：链类型为 `Live` 且创世分配了任何 ATC 的链规格在启动时被拒绝（无预挖，D9）；没有 Aura-PQ 授权节点的链同样被拒绝。
- **密钥**：验证人密钥来自加密文件（`--pq-key-file` + `--pq-password-file`），或仅在开发链和本地链上来自公开的开发名称（`--dev-key alice`；`--dev` 默认使用 alice）。正式链上拒绝开发密钥，且只接受 ML-DSA-65 密钥。出块封印、AC-BFT 投票和随机数共用一把密钥（签名与哈希上下文各不相同）。有密钥的节点只要密钥在当前授权集合中就参与投票；没有密钥的节点只跟随终局性。

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

## 监控

开启 Prometheus（默认开启，端口 9615）时，节点除 Substrate 指标外还导出：

| 指标 | 含义 |
|---|---|
| `acbft_round` | 当前 AC-BFT 轮次 |
| `acbft_finalized_number` | AC-BFT 最近最终确定的区块高度 |
| `acbft_finality_latency_seconds` | 从导入区块到最终确定该区块所用时间的直方图 |

日志目标：`ac-bft`（终局性组件；`-lac-bft=debug` 记录每条签名消息）、`aura-pq`（出块与封印双签告警）、`ac-offences`（双签举报）。

## 许可证

只有 `node/` 可以依赖许可证为 `GPL-3.0-or-later WITH Classpath-exception-2.0` 的 Polkadot SDK 客户端 crate（决策 D37）；节点自身的源码为 MIT 许可证。分发 `ac-node` 二进制时须遵守这些组件的 GPL 条款。
