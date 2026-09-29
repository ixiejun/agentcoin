> 🌐 [English](README.md) | **简体中文**

# ac-runtime

AgentCoin 的 WASM runtime：带计划排放和提名式权益证明的抗量子链（M1–M3）、EVM 合约（M4）和推理市场登记（M5）。

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
| 14 | `Revive` | `pallet-revive`：只接受 EVM 字节码合约，链 ID 4403，1 wei 等于 ATC 的最小单位；序号已发布（`ac_primitives::evm::REVIVE_PALLET_INDEX`） |
| 15 | `EvmSupport` | 合约付费账户、`Revive` 的不增发货币适配器、PQ 预编译 |
| 16 | `RefRate` | ATC/USD 参考汇率，由 PoA 管理多签设置，每天最多调整 ±20% |
| 17 | `ModelRegistry` | 按内容寻址的模型登记，收取存储押金 |
| 18 | `Providers` | 推理提供者：以美元计的质押、心跳、可服务判定；罚没/禁闭接口仅供 M6 使用 |
| 19 | `Gateways` | 推理网关：质押，费率上限 5% |
| 20 | `Credits` | 透明额度：托管通道与累计式 ML-DSA 凭证（`Credit` 接口，D20） |
| 21 | `Work` | 工作报告、销毁与款项分配、冻结款项、挑战期、领取；`Emission` 的 `WorkSource` 与 `MarketPayout` |

交易采用 v5 `General` 形式，首个扩展为 `PqAuthorize`（D36）；旧式 `Signed` 交易无法解码。`transaction` 模块为钱包和测试构造签名交易：`authorized_extensions`、`implicit_from`（由链上事实得到隐式数据）、`payload`（以 `agentcoin/tx/v1` 签名的 32 字节载荷）和 `assemble`。

创世预设 `development`（授权节点 alice）和 `local_testnet`（alice、bob、charlie、dave）为公开的开发账户 alice、bob、charlie、dave 分配余额；其他任何预设都不得分配 ATC。二者的排放纪元分别为 10 块和 20 块，PoA 管理权限分别为 alice（门限 1）和 alice、bob、charlie（门限 2），决议期限 20 块。两者的切换参数让测试链在一两分钟内进入 PoS：开发链——1 个合格候选人，从高度 20 起连续保持 20 个区块，`K` = 5；本地链——3 个候选人，高度 40，保持 40 个区块，`K` = 10；质押门槛均为发行量的 10%，解绑期和佣金延迟为数十个区块。正式链使用宪法值（10%、21、63,115,200 与 604,800 个区块），`K` = 100。

排放的安全预算交给 `StakingPos`（只在 PoS 阶段发放），双签也经它罚没。runtime API 在 M1–M3 的基础上新增 `StakingApi`（质押、候选人、最近一次选举、切换进度）。

EVM 合约（m4-evm）通过普通的 ML-DSA 签名交易部署和调用（`Revive::instantiate_with_code`、`Revive::call`）。调用过滤器 `RuntimeCallFilter`（`HolderTreasuryLock` 加 `EvmOnly`）拒绝以太坊交易入口、所有 PolkaVM 路径和地址映射变更；每个账户在创建时获得 EVM 地址（`keccak256(账户)[12..]`）。合约不会产生新发行量：revive 本应为新合约铸造的存在性押金改由交易签名者支付（`SetEvmPayer` 扩展），revive 自己的账户靠一个 provider 引用存在。`ReviveApi` 提供模拟执行、存储与代码查询；以太坊签名载荷、代码上传和追踪都被拒绝。开发版 runtime WASM 压缩前 4.8 MB。

推理市场（m5-market-registry）在序号 16–20 新增五个模块。`MarketApi` 提供参考汇率、模型、提供者（以及某模型的可服务提供者，每页最多 256 个）、质押门槛、网关、额度通道和 `check_voucher`（按兑付规则校验而不改变状态）。`development` 与 `local_testnet` 预设中 1 ATC = 1 美元，汇率调整间隔、心跳间隔和托管取回等待期均为 10 个区块，提供者与网关的解绑期为 20 个区块（活链分别为 1 天、600 个区块、1 天和 7 天）。

工作结算（m5-work-settlement）在序号 21 新增 `Work`。网关提交工作报告（收据 Merkle 根、按（提供者, 模型）汇总的合计、至多 16 张凭证和 128 项）：凭证兑付到网关账户，20% 经 `Emission` 销毁，网关费和提供者份额冻结在网关上，直到两个排放纪元后报告到期。每个纪元的已核验工作量是 `Emission` 的 `WorkSource`；市场排放铸入 `Work` 的领取账户（`MarketPayout`），与份额一起领取。到期前被禁闭的提供者失去待领取的份额与排放（`OnJail`）。`WorkApi` 提供参数、报告、冻结款项、各纪元工作量和累计合计。预设保留报告 20 个纪元（活链为 720）。`spec_version` 为 6，`transaction_version` 为 6。

管理权限来源是获得至少门限数成员批准的 `PoaCouncil` 决议（`PoaAdmin::dispatch_as_root` 以 Root 执行调用，例如 `System::set_code`）。持币人国库被锁定：`HolderTreasuryLock` 既是基础调用过滤器，也是 `dispatch_as_root` 的 Root 调用过滤器。被回收账户的尘埃同样经 `Emission` 销毁，所以发行量的变化始终恰好等于铸币减去销毁。

## Feature

| Feature | 默认 | 用途 |
|---|---|---|
| `std` | 是 | 原生 runtime 与 WASM 构建器。 |
| `runtime-benchmarks` | 否 | 所含模块的基准测试，包括 `pallet_revive`。 |

`runtime-benchmarks` 是内部开关（见 `LICENSE`）：它会引入 `pallet-revive-fixtures`（GPL-3.0-only），因此打开它构建的 runtime 只用于在开发者本机生成权重，从不分发；CI 会检查正式 runtime 不含基准测试 API（`scripts/check-release-runtime.sh`）。编译这些测试合约需要 nightly RISC-V 工具链、`solc` 与 `resolc`；没有这些工具时，设置 `SKIP_PALLET_REVIVE_FIXTURES=1` 即可用 `--all-features` 构建或测试（此时无法运行 revive 的基准，其他不受影响）。

## 示例

示例代码见英文版 [README.md](README.md)（英文版中的示例作为 doctest 运行）：为一笔转账构造扩展与隐式数据，计算待签名的 32 字节载荷。
