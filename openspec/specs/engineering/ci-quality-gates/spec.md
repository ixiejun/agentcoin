# engineering/ci-quality-gates Specification

## Purpose
规定 AgentCoin Rust 工作区在每次提交和拉取请求时必须通过的自动化质量门禁，保证一人 + AI 的开发方式下，代码格式、静态检查、测试、依赖许可证与安全公告始终受控。

## Requirements

### Requirement: 固定的工具链
工作区 SHALL 在仓库中固定 Rust 工具链版本及所需组件（rustfmt、clippy）和目标（`wasm32-unknown-unknown`）；本地与 CI MUST 使用同一版本。

#### Scenario: 工具链一致
- **WHEN** 在全新环境中进入仓库并执行任意 cargo 命令
- **THEN** 使用的是仓库固定的工具链版本

### Requirement: 每次推送与拉取请求都运行门禁
CI SHALL 在每次推送和每个拉取请求时运行以下检查，任一失败 MUST 使整个 CI 失败：
1. 格式检查（不允许未格式化的代码）；
2. 对全部目标和功能的 lint 检查，任何警告都视为错误；
3. 全部单元测试与测试向量测试，包括 AC-BFT 协议的确定性模拟测试、8 年排放模拟和选举基准规模测试；
4. 依赖策略检查：许可证白名单、禁止未知来源、安全公告；
5. 安全公告审计；
6. 密码学库以 no_std 方式为 `wasm32-unknown-unknown` 构建；
7. runtime 的 WASM 构建成功，且构建产物可被节点加载；
8. 多节点端到端测试：本地 4 节点网络出块并最终确定、停止 1 个节点时终局性不中断、停止 2 个节点后恢复、同一密钥双签被记录并在下一纪元禁用、随机数可独立复算、ML-DSA 签名转账、密钥轮换、多个排放纪元的铸币与国库入账、超发的 runtime 升级被节点拒绝、质押达标后 PoA 自动切换到 PoS、PoS 阶段选举与奖励发放、PoS 阶段双签罚没自质押；
9. EVM 端到端测试：用 Foundry 编译仓库中的合约，经钱包签名部署并调用 ERC-20 与官方 Uniswap V2（含工厂用 CREATE2 创建配对、添加流动性、兑换），经 eth-RPC 适配器读回状态、回执与日志，合约调用 PQ 预编译，以太坊签名交易被拒绝，且整个过程中节点的发行量检查全部通过；
10. 市场登记端到端测试：在开发链上经钱包设置参考汇率（管理多签）、登记模型、提供者与网关、提供者心跳并出现在“可服务提供者”查询中、用户托管并在等待期后取回，全程节点的发行量检查通过；
11. 工作结算端到端测试：在开发链上经钱包完成托管、签发凭证、提供者与网关双签收据、网关提交工作报告，挑战期内领取不到款项，挑战期满后提供者领取到费用与市场工作排放、网关领取到网关费，销毁计入累计销毁量、国库入账随工作量增长，全程节点的发行量检查通过；
12. TOPLOC 测试向量可复现：用固定提交的参考实现重新生成向量，与仓库中的向量文件逐字节相同；
13. 推理端到端测试：在开发链上登记模型、提供者与网关，启动提供者代理（连接确定性的模拟推理引擎）、网关与钱包本地代理，用官方 OpenAI Python SDK 经代理完成流式与非流式调用；网关自动提交工作报告，挑战期满后提供者与网关领取到款项；相对直连模拟引擎的额外首 token 延迟 p95 不超过 150 毫秒；网关与提供者的日志和数据目录中不出现请求与输出内容；全程节点的发行量检查通过。

#### Scenario: 格式错误导致失败
- **WHEN** 提交一段未经格式化的 Rust 代码
- **THEN** CI 的格式检查步骤失败

#### Scenario: lint 警告导致失败
- **WHEN** 提交一段会触发 lint 警告的代码
- **THEN** CI 失败

#### Scenario: 不允许的许可证导致失败
- **WHEN** 引入一个许可证不在白名单中的依赖
- **THEN** 依赖策略检查失败

#### Scenario: 端到端测试失败导致 CI 失败
- **WHEN** 提交一个使节点无法出块、无法最终确定区块、转账失败、节点接受超发区块或无法完成 PoA→PoS 切换的改动
- **THEN** CI 的端到端测试步骤失败

#### Scenario: EVM 端到端测试失败导致 CI 失败
- **WHEN** 提交一个使合约部署失败、适配器返回与链上不一致的数据、或合约部署导致节点拒块的改动
- **THEN** CI 的 EVM 端到端测试步骤失败

#### Scenario: 市场端到端测试失败导致 CI 失败
- **WHEN** 提交一个使模型或提供者登记失败、可服务提供者查询结果与链上状态不一致、或托管取回改变总发行量的改动
- **THEN** CI 的市场端到端测试步骤失败

#### Scenario: 工作结算端到端测试失败导致 CI 失败
- **WHEN** 提交一个使报告在汇总与凭证不一致时仍被接受、挑战期内可领取款项、或结算改变总发行量（排放结算之外）的改动
- **THEN** CI 的工作结算端到端测试步骤失败

#### Scenario: TOPLOC 向量被改动导致 CI 失败
- **WHEN** 仓库中的 TOPLOC 向量文件与参考实现重新生成的结果不同
- **THEN** CI 的 TOPLOC 向量检查失败

#### Scenario: 推理端到端测试失败导致 CI 失败
- **WHEN** 提交一个使 OpenAI SDK 经代理调用失败、网关不提交报告、收据费用与链上价格不符、额外首 token 延迟超过上限、或日志中出现请求内容的改动
- **THEN** CI 的推理端到端测试步骤失败

#### Scenario: 全部通过
- **WHEN** 在本变更完成后的主干上运行 CI
- **THEN** 所有检查步骤都通过

### Requirement: 默认禁止不安全代码
工作区中的 crate SHALL 默认禁止 `unsafe` 代码；任何例外 MUST 在代码中逐处注释说明理由。

#### Scenario: 引入 unsafe
- **WHEN** 在未声明例外的 crate 中加入 `unsafe` 代码块
- **THEN** 编译或 lint 失败

### Requirement: 测试向量可追溯
仓库中提交的每个外部测试向量文件 SHALL 记录上游来源 URL、上游文件的 SHA-256 以及筛选规则，并提供可复现筛选过程的脚本；重新运行脚本 MUST 得到逐字节相同的子集。

#### Scenario: 复现向量子集
- **WHEN** 运行向量获取脚本重新下载并筛选
- **THEN** 生成的文件与仓库中提交的文件逐字节相同，且上游校验和匹配

### Requirement: 按目录划分的许可证边界
仓库 SHALL 按目录分为两个许可证区：
- **GPL 区**（GPL-3.0-or-later）：`node/`、`services/`、`clients/wallet-cli/`、`tests/`、`scripts/`、`contracts/acceptance/`；
- **宽松区**（MIT OR Apache-2.0）：GPL 区以外的所有目录，新增目录默认属于宽松区。

仓库 SHALL 登记一份**内部开关**清单及其**放行依赖**清单：
- 内部开关是只用于开发者本机的 Cargo feature。打开内部开关构建出的产物 MUST NOT 对外分发。
- 首批内部开关只有 `runtime-benchmarks`，放行依赖只有 `pallet-revive-fixtures`。
- 两份清单的任何增删 MUST 经过 OpenSpec 变更。

CI MUST 在以下任一情况下失败：
1. **会分发的构建**中出现 GPL-only 依赖。会分发的构建指宽松区 crate 打开除内部开关以外全部 feature 时的构建。其依赖闭包（常规与构建依赖，含传递依赖，也包括本仓库 GPL 区的 crate）中不得出现只能以 GPL 家族许可证使用的依赖。带非 GPL 备选的表达式（如 `Apache-2.0 OR GPL-3.0`）视为允许。
2. 宽松区 crate 打开全部 feature（含内部开关）时，依赖闭包中出现不在放行依赖清单里的 GPL-only 依赖。
3. 任一工作区 crate 声明的许可证与其所在区不一致：宽松区必须为 `MIT OR Apache-2.0`，GPL 区必须为 `GPL-3.0-or-later`。
4. 宽松区中的 Solidity 源文件缺少 SPDX 许可证标识，或其标识为 GPL 家族许可证；宽松区的 Solidity 文件 MUST NOT 导入 GPL 区的 Solidity 文件。

GPL 区的 crate MAY 依赖任意 GPL 兼容许可证的依赖；GPL 区中从上游原样引入的第三方源码 SHALL 保留其原许可证，并记录来源 URL、固定版本与 SHA-256。

依赖策略检查 SHALL NOT 把放行依赖的许可证加入全局白名单，只对放行依赖逐个登记例外，并写明理由。

#### Scenario: 库 crate 引入 GPL 依赖
- **WHEN** 让 `crates/` 下的某个 crate 依赖一个 GPL-3.0 许可证的 crate
- **THEN** 许可证边界检查失败

#### Scenario: 库 crate 依赖本仓库的 GPL crate
- **WHEN** 让 `pallets/` 下的某个 crate 依赖 `clients/wallet-cli`
- **THEN** 许可证边界检查失败

#### Scenario: 新目录默认宽松
- **WHEN** 在新建的顶层目录中加入一个依赖 GPL-3.0 crate 的 crate
- **THEN** 许可证边界检查失败

#### Scenario: 声明与目录不一致
- **WHEN** `node/` 下的 crate 声明 `license = "MIT"`，或 `crates/` 下的 crate 声明 `license = "GPL-3.0-or-later"`
- **THEN** 许可证边界检查失败

#### Scenario: 节点使用 GPL 依赖
- **WHEN** 节点客户端或 `services/` 下的服务依赖 GPL-3.0 许可证的 crate
- **THEN** 许可证边界检查通过

#### Scenario: 当前仓库通过
- **WHEN** 在本变更完成后的主干上运行许可证边界检查
- **THEN** 检查通过

#### Scenario: 宽松区合约使用 GPL 标识
- **WHEN** 在 `contracts/src/` 中加入一个 SPDX 标识为 GPL-3.0 或缺少 SPDX 标识的 Solidity 文件，或让其导入 `contracts/acceptance/` 下的文件
- **THEN** 许可证边界检查失败

#### Scenario: 验收区引入官方 Uniswap V2
- **WHEN** `contracts/acceptance/` 中包含原样引入的官方 Uniswap V2 源码（GPL-3.0），并附来源记录
- **THEN** 许可证边界检查通过

#### Scenario: 放行依赖只在内部开关下出现
- **WHEN** 宽松区 crate 只在 `runtime-benchmarks` 开关下依赖 `pallet-revive-fixtures`（GPL-3.0-only）
- **THEN** 许可证边界检查通过

#### Scenario: 放行依赖出现在普通开关下
- **WHEN** 宽松区 crate 在 `std` 或默认 feature 下依赖 `pallet-revive-fixtures`
- **THEN** 许可证边界检查失败，并指出该依赖出现在会分发的构建中

#### Scenario: 内部开关引入未登记的 GPL 依赖
- **WHEN** 宽松区 crate 在 `runtime-benchmarks` 开关下依赖一个不在放行依赖清单中的 GPL-3.0 crate
- **THEN** 许可证边界检查失败

#### Scenario: 未登记的开关引入放行依赖
- **WHEN** 宽松区 crate 在一个未登记为内部开关的 feature（例如 `try-runtime`）下依赖 `pallet-revive-fixtures`
- **THEN** 许可证边界检查失败

#### Scenario: 依赖策略检查只对放行依赖开例外
- **WHEN** 依赖图中除放行依赖外还出现另一个 GPL-3.0-only 许可证的 crate
- **THEN** 依赖策略检查失败

### Requirement: 安全公告例外须逐条记录
依赖策略与安全公告审计中的任何忽略项 SHALL 逐条记录公告编号、受影响的依赖路径、不受影响或无法修复的理由，以及复查期限（不超过 90 天）；超过复查期限的忽略项 MUST 使 CI 失败，直到被复查更新或移除。

#### Scenario: 过期的忽略项
- **WHEN** 某安全公告忽略项的复查期限早于当前日期
- **THEN** CI 失败并指出该忽略项

#### Scenario: 缺少理由的忽略项
- **WHEN** 新增一个没有理由或复查期限的忽略项
- **THEN** CI 失败

### Requirement: 正式 runtime 不含内部开关
CI SHALL 以正式配置（release、默认 feature）构建 runtime 的 WASM 产物，并检查该产物导出的 runtime API 清单。清单中出现基准测试 API 时，CI MUST 失败。检查工具 SHALL 带自测，证明它能识别出含基准测试 API 的 runtime。

#### Scenario: 正式 runtime 通过
- **WHEN** CI 以正式配置构建 runtime 并运行检查
- **THEN** 检查通过，产物的 runtime API 清单中没有基准测试 API

#### Scenario: 基准开关混入正式构建
- **WHEN** 一个改动使正式配置构建出的 runtime 含有基准测试 API（例如把 `runtime-benchmarks` 加入默认 feature）
- **THEN** CI 的正式 runtime 检查失败

#### Scenario: 自测识别带基准接口的 runtime
- **WHEN** 对一个在 runtime API 清单中声明了基准测试 API 的 WASM 产物运行检查工具的自测
- **THEN** 检查工具判定该产物含基准测试 API 并返回失败；对清单中没有该 API 的产物则返回通过
