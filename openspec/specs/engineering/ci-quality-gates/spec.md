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
8. 多节点端到端测试：本地 4 节点网络出块并最终确定、停止 1 个节点时终局性不中断、停止 2 个节点后恢复、同一密钥双签被记录并在下一纪元禁用、随机数可独立复算、ML-DSA 签名转账、密钥轮换、多个排放纪元的铸币与国库入账、超发的 runtime 升级被节点拒绝、质押达标后 PoA 自动切换到 PoS、PoS 阶段选举与奖励发放、PoS 阶段双签罚没自质押。

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
带 Classpath 例外的 GPL-3.0 许可证 SHALL 只允许出现在节点客户端（`node/`）的依赖闭包中；`crates/`、`pallets/`、`runtime/`、`clients/` 下任一 crate 的依赖闭包（含传递依赖）中出现 GPL 家族许可证的依赖时，CI MUST 失败。

#### Scenario: 库 crate 引入 GPL 依赖
- **WHEN** 让 `crates/` 下的某个 crate 依赖一个 GPL-3.0 许可证的 crate
- **THEN** 许可证边界检查失败

#### Scenario: 节点使用 GPL 依赖
- **WHEN** 节点客户端依赖带 Classpath 例外的 GPL-3.0 许可证的 SDK 客户端 crate
- **THEN** 许可证边界检查通过

### Requirement: 安全公告例外须逐条记录
依赖策略与安全公告审计中的任何忽略项 SHALL 逐条记录公告编号、受影响的依赖路径、不受影响或无法修复的理由，以及复查期限（不超过 90 天）；超过复查期限的忽略项 MUST 使 CI 失败，直到被复查更新或移除。

#### Scenario: 过期的忽略项
- **WHEN** 某安全公告忽略项的复查期限早于当前日期
- **THEN** CI 失败并指出该忽略项

#### Scenario: 缺少理由的忽略项
- **WHEN** 新增一个没有理由或复查期限的忽略项
- **THEN** CI 失败
