> 🌐 [English](README.md) | **简体中文**

# ac-node

AgentCoin 节点客户端（M3：带终局性与计划排放的抗量子链）。

- **出块**：Aura-PQ（`ac-consensus-aura-pq`）：ML-DSA-65 区块封印，时隙 1 秒，授权节点来自验证人集合模块，只在纪元边界变更。
- **终局性**：AC-BFT（`ac-consensus-bft`），两阶段 BFT 组件，投票使用 ML-DSA-65（`agentcoin/bft-vote/v1`）。`n = 3f + 1` 个验证人中至多 `f` 个故障或离线时仍能最终确定；超过时继续出块但终局性暂停。每个变更授权集合的区块以及至少每 64 个区块保存一份终局性证明（commit 票集合）；同步的节点用应当签名的集合验证这些证明。验证人先持久化投票再发送，因此重启后不会重复签名。
- **双签**：同一密钥在同一时隙封印的两个区块头（由区块验证器发现），或两张冲突的 AC-BFT 投票（由终局性组件发现），会通过运行时构造成无签名举报交易，提交到本地交易池。链上记录违规，并在下一个纪元边界把违规者移出集合（M2 不罚没质押）。
- **随机数**：出块者以 inherent 数据提供 commit–reveal 秘密值；纪元 `e` 的随机数在纪元 `e + 2` 开始时公布。秘密值由验证人密钥派生，不会出现在日志中。
- **哈希**：区块与状态都使用 BLAKE3-256。
- **宪法第 1 层**（`ac-invariants`，在节点中执行，不依赖 runtime）：
  - 启动时链规格的创世必须设置合法的排放纪元长度，且发行量不超过 21,000,000 ATC；`Live` 链规格还必须不分配任何 ATC（无预挖，D9），并至少设置一名 PoA 管理成员。没有 Aura-PQ 授权节点的链同样被拒绝；
  - 每个区块——无论本节点产出、从网络接收还是同步而来——在交给客户端之前都经过 `InvariantBlockImport`：发行量不超过上限，只有排放结算区块可以铸币，且不超过所结算纪元计划量的两倍，创世以来的铸币总量不超过累计计划量。铸币量按 Δ(`Balances::TotalIssuance` + `Emission::TotalBurned`) 计算，二者都是已发布的固定存储键。违规区块被拒绝（本节点产出的区块既不导入也不广播），日志写明违反的规则与数值。runtime 升级后依然如此；
  - PoA→PoS 切换：在每个 PoA 纪元边界，节点从父状态（`StakingPos::Ledger` 中全部有效质押之和、合格候选人数、发行量、高度）独立复算切换检查点，拒绝提前或拖延切换、退回 PoA、PoS 阶段仍保留 PoA 名单、或在其他区块改动切换状态的区块。`Live` 规格必须使用宪法规定的切换参数（10%、21 个候选人、63,115,200 与 604,800 个区块）；
  - fail-closed：固定存储键缺失或无法解码时拒绝区块；区块从不未经执行就导入，因此不支持 warp 同步和快速同步。
- **密钥**：验证人密钥来自加密文件（`--pq-key-file` + `--pq-password-file`），或仅在开发链和本地链上来自公开的开发名称（`--dev-key alice`；`--dev` 默认使用 alice）。正式链上拒绝开发密钥，且只接受 ML-DSA-65 密钥。出块封印、AC-BFT 投票和随机数共用一把密钥（签名与哈希上下文各不相同）。有密钥的节点只要密钥在当前授权集合中就参与投票；没有密钥的节点只跟随终局性。

## 运行

```bash
# 单节点开发链（授权节点 alice，开发账户带余额）。
cargo build --release -p ac-node
target/release/ac-node --dev --tmp

# 四节点本地测试网（alice、bob、charlie、dave）；RPC 在 127.0.0.1:9944-9947，
# Prometheus 指标在 127.0.0.1:9615-9618。
scripts/run-local-testnet.sh
# 同上，但 40 秒后检查每个节点高度达到 20、已最终确定高度达到 15，alice 导出了
# acbft_finalized_number，排放纪元 0 已铸造国库保底，PoA 理事会有三名成员，且切换进度显示为
# PoA 并使用本地链的切换参数，然后退出。
scripts/run-local-testnet.sh --check

# 查看 alice 的 AC-BFT 指标。
curl -s http://127.0.0.1:9615/metrics | grep '^acbft_'

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

`node/` 属于 GPL 许可证区（决策 D47）：节点源码（含 `node/consensus/*`）采用 `GPL-3.0-or-later`，可以依赖许可证为 `GPL-3.0-or-later WITH Classpath-exception-2.0` 的 Polkadot SDK 客户端 crate。分发 `ac-node` 二进制时，须按 GPL 提供完整的对应源码。分区表见仓库根目录的 [`LICENSE`](../LICENSE)。
