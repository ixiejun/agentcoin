> 🌐 [English](README.md) | **简体中文**

# ac-runtime

AgentCoin 的 WASM runtime（M3：带计划排放和提名式权益证明的抗量子链）。

| 序号 | 模块 | 说明 |
|---|---|---|
| 0 | `System` | `Hashing = Blake3Hasher`：区块哈希、外部交易根和状态根都是 BLAKE3-256（D35） |
| 1 | `Timestamp` | 1 秒出块 |
| 2 | `AuraPq` | 来自创世配置的 ML-DSA-65 授权节点集合 |
| 3 | `Balances` | ATC，18 位小数，存在性押金 0.001 ATC；模块名是已发布的知名存储键（宪法第 1 层） |
| 4 | `TransactionPayment` | 权重费 + 长度费；每笔手续费和小费的 20%（向下取整）付给出块人，其余经 `Emission` 销毁 |
| 5 | `PqAccounts` | 公钥登记表、`rotate_key` |
| 6 | `ValidatorSet` | 按纪元变更的授权节点集合、PoA 名单与单向的 PoA→PoS 切换；模块名已发布（`Phase`、`QualifiedSince`、`PoaAuthorities`、`TransitionParams`、`EpochLength`） |
| 7 | `Offences` | 双签举报 |
| 8 | `RandomnessCr` | commit–reveal 随机数 |
| 9 | `Emission` | 计划排放与销毁记账；模块名已发布（`Emission::TotalBurned`、`Emission::EpochLength`） |
| 10 | `TreasuryDual` | 社区资助、锁定的持币人国库、线性解锁的保底 |
| 11 | `PoaCouncil` | PoA 多签使用的 `pallet-collective` 实例；模块名已发布（`PoaCouncil::Members`） |
| 12 | `PoaAdmin` | 门限来源、`dispatch_as_root`、成员与门限 |
| 13 | `StakingPos` | 候选人、提名、解绑队列、选举、奖励、罚没；模块名已发布（`Ledger`、`Candidates`） |

交易采用 v5 `General` 形式，首个扩展为 `PqAuthorize`（D36）；旧式 `Signed` 交易无法解码。`transaction` 模块为钱包和测试构造签名交易：`authorized_extensions`、`implicit_from`（由链上事实得到隐式数据）、`payload`（以 `agentcoin/tx/v1` 签名的 32 字节载荷）和 `assemble`。

创世预设 `development`（授权节点 alice）和 `local_testnet`（alice、bob、charlie、dave）为公开的开发账户 alice、bob、charlie、dave 分配余额；其他任何预设都不得分配 ATC。二者的排放纪元分别为 10 块和 20 块，PoA 管理权限分别为 alice（门限 1）和 alice、bob、charlie（门限 2），决议期限 20 块。两者的切换参数让测试链在一两分钟内进入 PoS：开发链——1 个合格候选人，从高度 20 起连续保持 20 个区块，`K` = 5；本地链——3 个候选人，高度 40，保持 40 个区块，`K` = 10；质押门槛均为发行量的 10%，解绑期和佣金延迟为数十个区块。正式链使用宪法值（10%、21、63,115,200 与 604,800 个区块），`K` = 100。

排放的安全预算交给 `StakingPos`（只在 PoS 阶段发放），双签也经它罚没。runtime API 在 M1–M3 的基础上新增 `StakingApi`（质押、候选人、最近一次选举、切换进度）。`spec_version` 为 3，`transaction_version` 为 3。

管理权限来源是获得至少门限数成员批准的 `PoaCouncil` 决议（`PoaAdmin::dispatch_as_root` 以 Root 执行调用，例如 `System::set_code`）。持币人国库被锁定：`HolderTreasuryLock` 既是基础调用过滤器，也是 `dispatch_as_root` 的 Root 调用过滤器。被回收账户的尘埃同样经 `Emission` 销毁，所以发行量的变化始终恰好等于铸币减去销毁。

## Feature

| Feature | 默认 | 用途 |
|---|---|---|
| `std` | 是 | 原生 runtime 与 WASM 构建器。 |
| `runtime-benchmarks` | 否 | 所含模块的基准测试。 |

## 示例

示例代码见英文版 [README.md](README.md)（英文版中的示例作为 doctest 运行）：为一笔转账构造扩展与隐式数据，计算待签名的 32 字节载荷。
