> 🌐 [English](07-constitution-immutability.md) | **简体中文**

# 第七轮：宪法条款如何“不可变”——Polkadot、BTC、Cardano 对比

> 状态：讨论稿（Round 7）。日期：2026-09。

## 新增已确认决策

| # | 决策 |
|---|---|
| D27 | 私有能力：MVP 做 L0（私有转账、匿名推理凭证）；全量做 L1（私密投票、密封竞价算力采购、屏蔽池内私有兑换），电路自研、一次审计；L2 通用私有合约只预留接口，约 2028 年后再评估；不做 L3 |
| D28 | 训练者 5% 分润：激励代替强制（血统声明 + 路由优先） |
| D29 | 美元计价的过渡期：由治理设定参考汇率（设调整幅度上限），流动性足够后切换为预言机 |

---

## 1. Polkadot 的做法：**没有不可变条款，一切都可以投票修改**

- **机制**：OpenGov 多轨道公投。
  - **Root 轨道**：权限最高，可以替换整个 runtime。门槛极高：第 1 天要达到全网发行量 46.8% 的支持率和 88% 以上的赞成率；支持率曲线在 7 天内线性降到 25%，14 天时趋近于 0。准备期和执行延迟都很长。
  - **Wish For Change 轨道**：**不执行任何代码**，只是链上的“意向表决”，用于在正式提案之前凝聚共识。
  - **Technical Fellowship 白名单**：技术委员会可以把紧急修复加入白名单，走更短的流程（用于安全修复）。
- **2.1B 硬顶的落地过程**：先由 Ref 1710 在 Wish For Change 轨道上以 81% 赞成通过，表达意向；再由 Ref 1828 等正式提案，通过 runtime 升级 v2.1.0（2026-03-12）写入协议。
- **关键事实**：这个硬顶**写在 runtime 里**，而 runtime 可以通过 Root 公投整体替换。所以分析师普遍指出：“**治理驱动的供给变化存在执行风险，未来的投票理论上可以撤销它；而比特币的减半在数学上是必然的。**”
- **结论**：Polkadot 的“宪法”是**高门槛的可变规则**，不是不可变条款。它依靠的是投票门槛高和时间长，而不是技术上的不可能。

## 2. 比特币的做法：**节点客户端强制，改变需要硬分叉**

- 2100 万上限不存在于任何治理系统里，而是由**每一个全节点在验证区块时独立检查**。一旦超发，这个区块就会被所有诚实节点拒绝。
- 修改上限唯一的办法，是说服绝大多数节点运营者**自愿安装新的客户端软件**（硬分叉）。拒绝升级的人会继续留在原链上。
- **结论**：不可变的本质是“**把修改成本提高到需要全社会自愿同意**”。技术上并没有绝对的不可变，但这是目前最强的形式。

## 3. Cardano 的做法：**成文宪法 + 宪法委员会 + 护栏脚本**

- 2025 年 Plomin 硬分叉之后，链上正式记录了宪法文本的哈希。
- **宪法委员会**审查每一项治理行动是否违宪。
- **护栏脚本（Guardrails Script）**：一个链上验证脚本，**自动拒绝**超出宪法规定范围的参数修改（例如某参数只能在 X–Y 之间调整）。
- 修改宪法或护栏脚本本身，需要 65–90% 区间的更高门槛。
- **结论**：介于 Polkadot 和 BTC 之间，是“**可变但有机器强制的边界**”。

## 4. 对比

| | Polkadot | Cardano | Bitcoin |
|---|---|---|---|
| 核心条款所在的位置 | runtime（可升级） | 链上宪法 + 护栏脚本 | 节点客户端 |
| 怎样才能修改 | Root 公投 | 更高门槛的宪法修改 | 硬分叉（节点自愿升级） |
| 机器强制 | 否 | 部分（参数边界） | 是 |
| 修改难度 | 高 | 更高 | 极高 |
| 适应性 | 最好 | 中 | 最差 |

---

## 5. AgentCoin 的设计：三层防护

把 BTC 的“节点强制”、Cardano 的“护栏”和 Polkadot 的“分轨治理”组合起来，分别用在不同重要程度的规则上。

### 第 1 层：宪法级不变式，由节点客户端强制（BTC 模式）

适用于 D24 中可以机械检查的条款：

| 不变式 | 节点每个区块检查的内容 |
|---|---|
| 总量上限 | `total_issuance ≤ 21,000,000 × 10^18` |
| 排放曲线 / 无预挖 | 本 epoch 铸币量 ≤ 计划额 + 可动用储备（按创世写死的公式计算）；不存在排放通道之外的铸币 |
| PoA → PoS 切换 | 条件满足后，下一个 era 必须完成切换，否则区块被拒绝 |

实现要点：
- 检查逻辑写在**节点客户端（native 代码）**里，**不在 runtime 里**，因此 runtime 升级改不了它。
- 节点在执行完区块后，读取约定好的存储键（例如总发行量）并校验。如果 runtime 升级后这些存储键不见了或者格式变了，就**默认拒绝**（fail-closed）。
- 想修改这些条款，只能**硬分叉**：发布新的客户端，由节点运营者自愿升级。

### 第 2 层：宪法 + 护栏（Cardano 模式）

适用于**无法完全机械化**或**需要一定灵活性**的条款：

- **链上宪法文本**（哈希上链），其中包括“协议中立：协议层不得加入内容审查或地域封锁”。
- **护栏 pallet**：所有参数修改都必须落在宪法规定的范围内。例如：
  - 金库比例只能在 0–20% 之间；
  - 销毁比例只能在规定区间内；
  - 参考汇率单次调整不超过 ±20%。
- **禁止类检查**：runtime 升级提案在执行前，需要附带公开的代码差异报告；由贡献者院 + 代币院双院审议是否违宪。
- 修改宪法本身：**两院都要超级多数**（例如各 75%），外加更长的延迟期（例如 90 天）。

### 第 3 层：日常治理（Polkadot OpenGov 模式）

- 分轨道公投：Root（runtime 升级）、参数、金库、安全委员会等轨道，各自设定门槛曲线和执行延迟。
- **意向轨道**（类似 Wish For Change）：只表达意向、不执行代码，用于在重大提案前凝聚共识。
- **安全委员会快速通道**（类似 Fellowship 白名单）：**只能暂停模块和吊销 TEE 证明**，不能触碰第 1、2 层的条款，并且事后必须经过全民投票追认。
- runtime 升级通过后，强制等待 28 天才生效，让不同意的人有时间退出。

### 诚实的说明

绝对的不可变并不存在。BTC 的 2100 万上限，本质上也是由社会共识维护的。我们能做到的是：
- 第 1 层条款达到 **BTC 同等级别**的修改难度；
- 第 2 层达到 **Cardano 级别**；
- 其余规则保持 **Polkadot 式的可演进性**。

## 来源

- Polkadot 硬顶落地：https://www.bitget.com/news/detail/12560605240453 · https://rwatimes.substack.com/p/polkadot-dot-price-prediction-2026-10b · https://www.kucoin.com/news/articles/polkadot-set-to-implement-landmark-2-1-billion-dot-supply-cap-in-march-2026
- OpenGov：https://wiki.polkadot.com/learn/learn-polkadot-opengov/ · https://wiki.polkadot.com/learn/learn-polkadot-opengov-origins/ · https://wiki.polkadot.com/learn/learn-polkadot-technical-fellowship/ · https://docs.polkadot.com/reference/governance/
- Cardano：https://cips.cardano.org/cip/CIP-1694 · https://docs.intersectmbo.org/archive/cardano-governance-archive/cardano-constitution/read-the-cardano-constitution · https://www.theblock.co/post/337680/cardano-plans-transition-to-full-decentralized-governance-after-wednesdays-plomin-hard-fork · https://developers.cardano.org/docs/governance/cardano-governance/submitting-governance-actions/
