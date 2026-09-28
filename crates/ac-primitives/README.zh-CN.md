> 🌐 [English](README.md) | **简体中文**

# ac-primitives

AgentCoin 的链上共享类型，供 runtime、节点和客户端使用。

- `Blake3Hasher`：把 BLAKE3-256 适配到 Polkadot SDK 的哈希 trait。它是链的 `Hashing` 类型，因此区块哈希、外部交易根和状态根都使用 BLAKE3（决策 D35）。
- 地址：`encode_address` / `decode_address` 在 32 字节账户 ID 与 `atc1…` 形式的 bech32m（BIP-350）字符串之间互转；错一个字符一定能被检测出来。
- `NoClassicSignature`：一个不可能有值的类型，用来占住 SDK 旧式 `Signed` 交易的签名位置，保证任何经典签名算法都无法授权账户（决策 D36）。
- `ChainProfile` 与 `ChainProfileApi` runtime API：客户端可以借此核对链的哈希方案。
- `aura_pq`：Aura-PQ 的时隙、预运行摘要和封印摘要工具。
- `ac_bft`：AC-BFT 终局性类型：带版本号的已签名消息（提议、prepare / commit 投票、超时，签名上下文 `agentcoin/bft-vote/v1`）、终局性证明与 `verify_finality_proof`，以及授权节点集合变更摘要（引擎号 `acbf`）。未知的格式版本直接解码失败，不会被当作其他格式解析。
- `offences`：双签证据（同一时隙封印的两个区块，或同一轮中互相冲突的两条 AC-BFT 消息）与 `verify_evidence`，runtime 和节点共用。
- `epoch`：纪元编号（`epoch_of`、`is_boundary`）与纪元长度下限。
- `emission`：ATC 的排放曲线与纪元结算（方案 §5.1）。runtime、节点不变量检查器和经济模拟都使用它，因此三者算出的数完全一致。
- `staking`：PoA→PoS 切换规则及其宪法值、最低质押额、提名解绑队列、奖励拆分和选举输入。runtime 与节点不变量检查器共用，因此两边得出同样的切换结论。
- `market`：推理市场类型（M5）：美元金额与参考汇率及明确的取整方向（付款向下、门槛向上），模型清单与模型 ID（上下文 `agentcoin 2026-09 model-id v1`），以 `agentcoin/voucher/v1` 签名的累计式透明凭证与 `check_voucher`（兑付规则的唯一实现，链上链下共用），提供者、网关与通道记录，市场模块之间的接口（`PriceSource`、`Credit`、`ProviderPenalty` 等）以及 `MarketApi` runtime API。

AC-BFT 各种格式的字节级回归向量位于 `tests/vectors/`（见 `SOURCES.md`）。

## Feature

| Feature | 默认 | 用途 |
|---|---|---|
| `std` | 是 | 为节点、钱包和测试提供标准库支持。WASM runtime 构建时关闭。 |

## 示例

示例代码见英文版 [README.md](README.md)（英文版中的示例作为 doctest 运行）：对字节串计算 BLAKE3 哈希得到 32 字节输出；把账户 ID 编码为 `atc1…` 地址并解码回原值。

## 独立验证终局性证明

验证终局性证明只需要创世哈希和证明所属的授权节点集合，因此轻客户端无需信任任何节点就能确认终局性。

示例代码见英文版 [README.md](README.md)（作为 doctest 运行）：用 alice、bob、charlie、dave 四个开发密钥组成集合，其中 3 个成员（q = ⌊2·4/3⌋ + 1 = 3）对同一目标签署 commit 票，组成版本 1 的证明，`verify_finality_proof` 返回被最终确定的区块。

## 排放曲线

- 首个 4 年期（1 秒出块时为 126,230,400 个区块）排放 10,500,000 ATC，此后每期减半，整条曲线的总量低于 21,000,000 ATC 的上限（决策 D14、D15）。
- 排放按长度为 `L` 个区块的“排放纪元”结算；`L` 是创世参数，必须整除 126,230,400（正式链为 3600）。纪元 `e` 在第 `(e + 1) × L + 1` 块结算，只有这些区块铸币。
- `settle` 对一个纪元做分配：安全预算为计划量 `S` 的 10%（只在 PoS 阶段发放）；市场工作最多为 `S + min(储备, S)` 的 50%，公共工作最多为其 20%；国库为 `max(5% × S, 20/70 × 工作排放)`，取大、不相加。未铸造的部分滚入储备，每个纪元最多从储备取用 `S`。比例以基点整数计算，向下取整。

示例代码见英文版 [README.md](README.md)（作为 doctest 运行）：长度 3600 的排放曲线下，一个没有工作量的 PoA 纪元只铸造 5% 的国库保底，其余滚入储备；第 3601 块结算纪元 0。

## 质押与 PoA→PoS 切换

- 宪法值（决策 D19、D24）：全部有效质押 ≥ 总发行量的 10%，合格候选人至少 21 个，区块高度 ≥ 63,115,200（2 年），且三者在每个纪元边界连续成立 604,800 个区块（7 天）。`TransitionParams::CONSTITUTION` 保存这些值；`ChainPhase` 与 `TransitionParams` 是已发布固定存储键的值，编码永不改变。
- `transition_step` 执行一次纪元边界检查：检查点不达标时清除连续达标的起始高度，首次达标时记下，连续保持满时长后切换。切换是单向的。
- 最低自质押为发行量的 0.1%，最低提名为 0.001%，向上取整。
- `unbonding_unlock`：提名通过全网队列退出，队列在最长期限内能退完全部有效质押；每笔等待时间介于最短（正式链 2 天）和最长（28 天）之间。
- `split_by_points` 按工作分（出块数）分配安全预算，与质押多少无关；`split_reward` 先扣佣金，再按支撑额拆分其余部分。两者都向下取整，不会创造 ATC。

示例代码见英文版 [README.md](README.md)（作为 doctest 运行）：按宪法值，12% 质押、25 个候选人、满 2 年时开始计时，7 天后切换；10% 佣金后按 1 : 2 拆分。
