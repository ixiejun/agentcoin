> 🌐 [English](AGENT.md) | **简体中文**

# AGENT.md — AgentCoin 编码智能体工作守则

> 本文件是所有编码智能体（以及人类贡献者）在本仓库工作时必须遵循的规则。
> 开始任何工作前先完整阅读本文件；与本文件冲突的做法一律以本文件为准，除非用户在当前对话中明确给出不同指示。

**指令优先级**（高 → 低）：
1. 用户在当前对话中的明确指示
2. 本文件（`AGENT.md`）
3. `openspec/config.yaml` 中的项目上下文与规则
4. 当前 OpenSpec 变更的 proposal / specs / design / tasks
5. `docs/rust-guidelines/`（Rust 编码规范，本文件 §7 对个别条款有覆盖）
6. 社区通用惯例

---

## 1. 项目概览

AgentCoin（代币 **ATC**）是一个**抗量子、天生隐私、无许可**的 L1 公链，把全球异构算力（数据中心 GPU、消费级 GPU、CPU、存储）组织起来，为大模型的**推理 → 后训练 → 预训练**提供去中心化服务，并兼容 EVM。

- 链框架：Polkadot SDK（Substrate）独立链；EVM 用 `pallet-revive`
- 主要语言：**Rust**（D31）；Python 仅限推理/训练引擎内部的薄插件
- 开发方式：**规格驱动开发（SDD）+ OpenSpec**（D32）
- 当前阶段：MVP；M0（工程底座 + PQ 密码库）、M1（PQ 链）、M2（AC-BFT 终局性）、M3（排放、国库、PoA 多签、提名式 PoS 与 PoA→PoS 切换）和 M4（EVM：`pallet-revive`、PQ 预编译、eth-RPC 适配器、经钱包使用 Foundry）已完成

### 1.1 权威文档地图

| 文档 | 作用 | 何时读 |
|---|---|---|
| `docs/decisions.md`（中文：`.zh-CN.md`） | **全部已确认决策 D1–D59（最高设计依据）** | 每次开始新任务 |
| `docs/design/mvp-technical-plan.md` | MVP 架构、模块、数据结构、里程碑 | 做 MVP 任务时 |
| `docs/design/full-technical-plan.md` | 全量版架构与 MVP 必须预留的接口（§11） | 设计任何接口时 |
| `docs/research/01–07` | 决策的调研依据与讨论过程 | 需要理解“为什么”时 |
| `openspec/specs/` | 已归档的系统规范（行为契约） | 修改已有能力时 |
| `openspec/changes/<name>/` | 进行中的变更（本地，不入库） | 执行 apply 时 |
| `docs/rust-guidelines/INDEX.md` | Rust 编码规范索引 | 写任何 Rust 代码时 |

发现文档之间有冲突时：**不要自行裁决**，在回复中指出冲突并询问用户；决策以 `docs/decisions.md` 为准。

---

## 2. 红线（任何情况下不得违反）

违反下列任何一条的改动都不得提交。若任务要求与红线冲突，停止并向用户说明。

1. **100% 后量子**（D4、D13）
   - 账户授权与共识签名只用 ML-DSA（预留 SLH-DSA / FN-DSA / XMSS）；**禁止** secp256k1、Ed25519、sr25519、BLS 用于账户授权或共识。
   - 加密只用 ML-KEM-768 + X25519 混合（X-Wing）或纯 PQ KEM；禁止单独使用经典 ECDH 保护长期机密数据。
   - 零知识证明只用 STARK / FRI 系；**禁止** Groth16、PLONK-KZG、任何基于 BN254 / BLS12 配对的方案。
   - 所有哈希输出 256 位（BLAKE3 / SHA3-256 / Poseidon2）。
2. **算法可插拔**：所有公钥、签名、密文、证明都必须带 AlgId（`ac-crypto` 的带标签类型）。**不得**在 `ac-crypto` 之外直接调用具体算法实现。
3. **宪法第 1 层由节点强制**（D24、D30）：总量上限 21,000,000 ATC、排放上限 / 无预挖、PoA→PoS 切换条件，必须在节点客户端（native）中检查，**不能只写在 runtime**。约定存储键（well-known keys）一经发布不得重命名或改格式。
4. **无预挖**（D9）：创世不得分配任何 ATC；除排放规则外不存在任何铸币路径。
5. **金库公式**（D18）：`treasury = max(5% × 计划额, 20/70 × 实际工作排放)`，取较大值，**不相加**。
6. **隐私**（D20、D27）：prompt 与推理输出**永不上链**、网关不记录请求内容；主网付费路径只有匿名凭证。
7. **协议中立**（D6、D24）：协议层不得加入内容审查、地址黑名单、地域封锁。
8. **链不在推理数据路径上**：链下 refine → 工作报告 → 链上 accumulate。
9. **不自研密码学原语**：只封装经过审阅的实现（RustCrypto 等）；新增原语必须有官方测试向量。
10. **不提交秘密**：私钥、助记词、API key、`.env` 一律不入库。

---

## 3. 开发流程：SDD + OpenSpec

**没有经过用户确认的 OpenSpec 变更，不写功能代码。**

```
/opsx:explore（可选，厘清需求）
   → /opsx:propose  生成 proposal / specs / design / tasks
   → 用户审阅确认                         ← 必须等待，不得跳过
   → /opsx:apply    按 tasks.md 逐项实现并勾选
   → 全部任务完成 + CI 通过
   → /opsx:archive  规范合入 openspec/specs/
```

规则：
- **提案**（遵循 `openspec/config.yaml` 的 rules）：必须引用决策编号 Dxx 和技术方案章节；必须有 Non-goals；涉及密码学/共识的变更必须说明对抗量子性与宪法第 1 层的影响；design 必须说明与全量版预留接口的衔接。
- **specs 写行为，不写实现**：不出现库名、函数名；每条 Requirement 至少一个可测试的 Scenario（`####` 四级标题）；用 SHALL / MUST。
- **tasks**：每项 ≤ 约半天工作量，并写明可自动化的验收方式；测试与文档随所在任务组一起交付，不集中到最后。
- **apply**：严格按 tasks.md 顺序；每完成一项立即勾选 `- [x]`；发现 tasks 与 specs/design 矛盾或需要扩大范围时，**停止并提出**，用 `/opsx:update` 修订计划，不要边做边改范围。
- **archive**：归档前确认 specs 已同步；新产生的决策补记到 `docs/decisions.md`（新编号，注明来源），中英文两个版本都要补。
- **OpenSpec 产物使用简体中文。**
- `openspec/changes/` 已被 `.gitignore` 忽略（不入库）；云端容器会被回收，因此**变更完成后尽快归档**。
- **可以不走 OpenSpec 的小改动**：错别字、纯文档措辞、CI 配置的非行为修复、依赖的补丁版本升级。凡改变外部可观察行为或接口的，都必须走 OpenSpec。
- **不要手工编辑** `openspec/specs/`（只能通过 archive 更新），除非修复 archive 留下的 `TBD` Purpose。

---

## 4. 仓库结构与模块边界

```
AGENT.md · CLAUDE.md · README.md · Cargo.toml · rust-toolchain.toml · deny.toml
docs/{decisions.md, design/, research/, rust-guidelines/}
openspec/{config.yaml, specs/, changes/(本地)}
crates/      通用库：ac-crypto、ac-primitives、ac-invariants、ac-toploc、ac-market-proto
node/        ac-node：共识（aura-pq、ac-bft）、节点不变式检查器
runtime/     WASM runtime 组装
pallets/     链上模块（pq-accounts、emission、credits、work、audit …）
circuits/    STARK 电路（β）
services/    gateway、provider、auditor、eth-rpc
clients/     wallet-cli、sdk（Rust 核心）、sdk-bindings（PyO3 / wasm-bindgen）
contracts/   示例 Solidity 合约
tests/       e2e、经济仿真
scripts/     工具脚本
```

- **许可证分区（D47）**：`node/`、`services/`、`clients/wallet-cli/`、`tests/`、`scripts/` 采用 `GPL-3.0-or-later`；其余部分（包括任何新目录）采用 `MIT OR Apache-2.0`，不得依赖 GPL，包括本仓库自己的 GPL crate。每个 crate 在 `Cargo.toml` 中声明所在区的许可证，详见 `LICENSE`。
- **目录按需创建**：不要为未来模块预先建空目录或空 crate。
- **依赖方向**：`crates/*` 不依赖 `node`/`runtime`/`pallets`/`services`；`pallets` 只依赖 `crates` 与 Polkadot SDK；`services`/`clients` 不依赖 `node` 内部实现，只通过 RPC / 公共类型交互。
- **密码学只在 `ac-crypto`**；共享数据类型只在 `ac-primitives`；节点不变式是 `ac-invariants` 中的**纯函数**（便于形式化验证）。

---

## 5. Rust 工程规范

### 5.1 工具链与工作区
- 工具链固定在 `rust-toolchain.toml`，不得在命令中临时切换版本。
- 所有 crate 是根工作区成员；依赖版本统一在根 `Cargo.toml` 的 `[workspace.dependencies]` 声明，成员用 `dep = { workspace = true }`。
- 版本号精确到次版本（如 `"0.1"`），**禁止通配符**（G.CAR.04）；新增依赖前检查：维护状态、许可证（`deny.toml` 白名单）、是否支持 `no_std`、是否重复引入同一库的多个版本（P.SEC.01）。
- 每个 crate 的 `Cargo.toml` 必须有 `description`、`license`、`repository`、`edition`（从 workspace 继承）（G.CAR.02）。

### 5.2 Lint 与格式（CI 强制）
- `cargo fmt --all -- --check` 必须通过（P.FMT.01）。
- `cargo clippy --workspace --all-targets --all-features -- -D warnings` 必须通过。
- 工作区 lints：`unsafe_code = "forbid"`；非测试代码 deny `clippy::unwrap_used`、`clippy::expect_used`、`clippy::panic`、`clippy::indexing_slicing`（链上与密码学 crate）、`clippy::arithmetic_side_effects`（链上与经济相关 crate）。
- 不得用 `#[allow(...)]` 绕过 lint，除非附一行注释说明理由；禁止 crate 级别的 `#![allow(clippy::all)]` 之类的大范围豁免。

### 5.3 错误处理（覆盖上游 P.ERR.02 / G.ERR.02）
- **非测试代码禁止 `unwrap()` / `expect()` / `panic!` / `unreachable!` / `todo!` / `unimplemented!`**；用 `?` 与显式错误类型。
- 库 crate 用自定义错误枚举（`#[non_exhaustive]`，实现 `core::fmt::Display`；`std` feature 下实现 `std::error::Error`）；不在库中使用 `anyhow`。二进制程序（services、CLI）可以在最外层使用 `anyhow`。
- 公开的返回 `Result` 的函数在 rustdoc 中写 `# Errors` 小节（G.CMT.01）。
- 测试代码可以使用 `unwrap` / `expect`。

### 5.4 整数与数值（链上确定性）
- 余额、排放、价格一律用**无符号整数**（余额 `u128`，18 位小数的最小单位）；**runtime、共识、结算代码禁止使用浮点数**。
- 所有可能溢出的运算用 `checked_*` / `saturating_*`，并说明选择理由；禁止依赖 release 模式下的回绕（G.TYP.INT.01）。
- 类型转换用 `From` / `TryFrom`，**禁止用 `as` 做可能截断或改变符号的转换**（G.TYP.01、G.TYP.03、G.TYP.INT.02）。
- 比例、分成用基点（1/10_000）或 `Perbill` / `Permill` 等定点类型，写明舍入方向（向下取整，余数进储备或销毁，不得凭空产生）。
- 数组 / 切片访问用 `get()`，禁止可能越界的索引（G.TYP.ARR.02）。

### 5.5 no_std 与 runtime 兼容
- 会被 runtime 使用的 crate 必须 `#![no_std]`，按需 `extern crate alloc`；`std` 作为 feature 提供。
- CI 中以 `--no-default-features --target wasm32-unknown-unknown` 构建这些 crate。
- runtime 代码中不得出现：浮点、`HashMap`（非确定性迭代顺序，用 `BTreeMap`）、系统时间、随机数（用链上随机数模块）、无界集合（用 `BoundedVec` 等有界类型）。

### 5.6 类型与 API 设计
- 用新类型表达语义，不直接暴露原生类型（P.TYP.01）：如 `AccountId`、`Balance`、`AlgId`。
- 对外公开的 struct / enum 默认加 `#[non_exhaustive]`（G.TYP.SCT.01、G.TYP.ENM.05）；**例外**：线格式 / 链上编码类型的变体集合由 AlgId 等显式数值决定，必须显式标注判别值（G.TYP.ENM.07）。
- 函数参数不超过 5 个，多个 `bool` 参数改用枚举或配置结构体（G.FUD.01、G.FUD.03）。
- 不使用通配符导入 `use foo::*`，测试模块中的 `use super::*` 除外（G.MOD.03）。
- 可见性最小化：默认私有，按需 `pub(crate)`，对外 API 在 `lib.rs` 统一重导出（P.MOD.01、G.MOD.02）。
- Cargo feature 名用肯定式、无多余前后缀（P.NAM.02、G.CAR.03），不滥用 feature（P.CAR.02）。

### 5.7 异步与并发（链下服务）
- 异步运行时统一用 **tokio**；不在 async 上下文中执行阻塞操作（G.ASY.05），CPU 密集任务用 `spawn_blocking`。
- 不跨 `.await` 持有同步锁（G.ASY.02）；优先使用 channel / 消息传递。

### 5.8 unsafe
- 默认 `forbid`。确需例外（FFI、经过审阅的性能关键路径）时：在该 crate 局部改为 `deny` 并对具体块 `allow`；每个 `unsafe` 块前写 `// SAFETY:` 注释说明不变式（P.UNS.SAS.09）；公开 unsafe 函数写 `# Safety` 文档（G.UNS.SAS.01）；必须经用户确认。

---

## 6. 密码学编码规范

1. **只通过 `ac-crypto` 使用密码学**：其他 crate 不得直接依赖 `ml-dsa`、`ml-kem`、`x-wing`、`blake3` 等底层库（`cargo deny` 可配置 bans 强制）。
2. **签名必须带上下文字符串**：格式 `agentcoin/<用途>/v<版本>`（如 `agentcoin/tx/v1`、`agentcoin/bft-vote/v1`、`agentcoin/receipt/v1`）。每个新用途在 `crates/ac-crypto/README.md` 的上下文登记表中登记，**不得复用**已有用途的上下文。
3. **域分离哈希**：上下文格式 `agentcoin <YYYY-MM> <用途> v<版本>`，同样登记，不得复用。
4. **线格式稳定**：AlgId 编号、带标签编码、账户 ID 派生规则一经发布**永不修改**；需要变化时新增编号 / 新版本上下文。相关回归测试不得删除或放宽。
5. **秘密材料**：实现 `Zeroize` / `ZeroizeOnDrop`；`Debug` 输出脱敏；不写日志、不进错误信息、不序列化到非加密存储。
6. **随机数**：只用密码学安全随机源（`rand_core::CryptoRng`）；测试中用固定种子的确定性 RNG 须明确限定在 `#[cfg(test)]` 或测试 feature。
7. **比较秘密**用常数时间比较（`subtle`），禁止 `==`。
8. **测试向量**：每个算法必须通过官方向量（NIST ACVP、IETF 草案附录）；向量文件记录来源 URL、上游 SHA-256 与筛选规则，并可由脚本复现。
9. **验证失败统一返回错误/`false`**，不得 panic，不得回退到其他算法。

---

## 7. Rust 编码规范（docs/rust-guidelines）

本项目采用《Rust 编码规范》（中文版，MIT 许可）作为编码风格与实践基线，本地副本位于 `docs/rust-guidelines/`（上游原文为中文，不翻译）。

### 7.1 如何查阅
- **索引**：`docs/rust-guidelines/INDEX.md`
  - §1 AgentCoin 重点规则（**编码前必读**）
  - §2 AgentCoin 取舍（覆盖 / 不采纳的条款）
  - §3 章节目录、§4 全部 255 条条款及文件链接
- **按关键词或编号检索**：
  ```bash
  grep -n '整数\|溢出' docs/rust-guidelines/rules.tsv
  grep -n 'G.TYP.INT.01' docs/rust-guidelines/rules.tsv
  ```
  找到后打开对应 `src/...md` 阅读正例 / 反例。
- 编号含义：`P.*` 为原则（方向性），`G.*` 为规则（具体、多数可由 clippy 检测）。

### 7.2 使用要求
- 写代码前阅读 INDEX.md §1 的重点规则；涉及某类特性（unsafe、async、宏、no_std、整数、字符串等）时，先检索对应章节。
- Code review（包括自审）时，对照相关条款；在 PR / 提交说明中如有意偏离某条款，写明编号与理由。
- 规范文件是**只读的上游快照**：不得修改 `docs/rust-guidelines/src/`；更新用 `scripts/update-rust-guidelines.sh`，索引用 `scripts/gen-rust-guidelines-index.py` 重新生成，不得手改 `INDEX.md` / `rules.tsv`。

### 7.3 AgentCoin 对上游条款的覆盖
| 条款 | AgentCoin 规定 |
|---|---|
| P.ERR.02、G.ERR.02 | 非测试代码**禁止** `unwrap` / `expect`（比上游更严格），见 §5.3 |
| P.NAM.09 | 不采纳 `G_` 前缀；静态变量用 `SCREAMING_SNAKE_CASE` |
| G.TYP.BOL.07 | 不采纳；使用 `!` 取反 |
| P.CMT.04 | 不要求文件头版权注释（仓库级 LICENSE） |
| G.MTH.LCK.03 / 04 | runtime（no_std）不适用；链下服务可用 `parking_lot` / `crossbeam` / `tokio::sync` |
| G.TYP.SCT.01、G.TYP.ENM.05 | 默认采纳；线格式 / 链上编码枚举例外（见 §5.6） |
| （补充） | 链上与经济代码禁止浮点、禁止 `HashMap`、禁止无界集合（§5.4、§5.5） |

新增覆盖时：同时修改本表（中英文两个版本）与 `scripts/gen-rust-guidelines-index.py` 中的 `OVERRIDES`，并重新生成索引。

---

## 8. Substrate / Runtime 专项规范（M1 起适用）

- **runtime 中不得 panic**：所有 extrinsic 返回 `DispatchResult`；存储读写失败返回错误。
- **每个 extrinsic 必须有 benchmark 与权重**（`frame-benchmarking`），不得使用硬编码的占位权重进入测试网。
- **存储**：只用有界类型；存储结构变更必须附带迁移（`OnRuntimeUpgrade`）与迁移测试；宪法第 1 层使用的约定存储键不得改名、改格式。
- `on_initialize` / `on_finalize` 的工作量必须有上限并计入权重。
- **排放、销毁、罚没**：每条路径都要有“总量守恒”测试（铸造 − 销毁 = 发行量变化）。
- **治理参数**：必须登记护栏（`pallet-guardrails`）边界，不得出现无边界的可治理参数。
- 节点不变式（`ac-invariants`）的任何修改都视为**硬分叉级变更**，必须单独走 OpenSpec 并由用户明确确认。

---

## 9. 测试规范

- **每个 spec Scenario 至少对应一个自动化测试**；测试名或注释中注明对应的 Requirement / Scenario 名称，便于追溯。
- 测试类型：
  - 单元测试：与代码同文件 `#[cfg(test)] mod tests`，或 `tests/` 集成测试（P.MOD.02 建议大型测试移至单独文件）。
  - 向量测试：密码学与编码格式（`tests/vectors/`）。
  - 属性测试：编解码往返、算术守恒等用 `proptest`。
  - 模糊测试：解码器与所有处理外部输入的代码（`cargo fuzz`，M1 起）。
  - 端到端：多节点网络（`tests/e2e/`）；经济仿真（`tests/sim/`）。
- 测试必须确定性：固定种子，不依赖网络（向量获取脚本除外）、不依赖系统时间。
- **不得为了让 CI 通过而删除、跳过（`#[ignore]`）或放宽测试**；测试失败要找根因。

---

## 10. 文档与注释

- **语言约定**：
  - **项目文档双语，英文为主**：每份文档的英文版位于规范路径（如 `docs/decisions.md`），简体中文版为同目录的 `*.zh-CN.md`（如 `docs/decisions.zh-CN.md`）。
  - 英文页首第一行为 `> 🌐 **English** | [简体中文](<name>.zh-CN.md)`；中文页首为 `> 🌐 [English](<name>.md) | **简体中文**`。
  - **修改任一语言版本时，必须在同一次提交中同步修改另一版本**；两版内容（决策、数字、表格、结论）必须一致。英文版为准，出现分歧时以英文版为准并修正中文版。
  - 新建文档时同时创建两个版本。
  - **OpenSpec 产物（`openspec/`）只用简体中文**，不需要英文版。
  - 第三方上游快照（`docs/rust-guidelines/src/` 及生成的 `INDEX.md`、`rules.tsv`）保持上游原语言，不翻译。
  - **代码标识符、rustdoc、代码注释、提交信息用英文**（面向国际开源协作）；crate 的 `README.md` 同样双语（`README.md` + `README.zh-CN.md`）。
  - **与用户沟通使用简体中文**：回复、进度汇报以及流程或变更摘要（如 apply、archive、一轮 CI 之后的总结）一律用简体中文，除非用户在当前对话中另有要求。
- 所有公开项必须有 rustdoc；返回 `Result` 的写 `# Errors`，可能 panic 的写 `# Panics`（本项目原则上不应存在），unsafe 写 `# Safety`（G.CMT.01、G.CMT.02、G.UNS.SAS.01）。
- 注释说明“为什么”，不复述代码（P.CMT.01）；使用 `//` 行注释（P.CMT.03）；`TODO` / `FIXME` 必须附带简短说明（P.CMT.05）。
- 每个 crate 有 `README.md`（及 `README.zh-CN.md`）：用途、feature 说明、最小示例（英文版中的示例作为 doctest 运行）。
- 接口或行为变化时，同步更新相关 README 与 `docs/`。

---

## 11. Git 与提交规范

- 在用户指定的分支上工作；不向其他分支推送，不 force push，不改写已推送历史。
- **提交信息**：英文，Conventional Commits 风格：`feat(ac-crypto): ...`、`fix(...)`、`docs: ...`、`chore: ...`、`test: ...`、`ci: ...`；正文说明动机与影响，引用 OpenSpec 变更名与任务编号（如 `m0-foundation-pq-crypto 4.2`）。
- 按系统提示要求附加 attribution 行；**不得**在提交、PR、代码中写入模型标识。
- 小步提交：一个任务（或一个紧密相关的任务组）一次提交；每次提交都应能通过 fmt / clippy / test。
- 推送前运行完整检查（§13）；网络失败按 2s、4s、8s、16s 退避重试最多 4 次。
- 未经用户明确要求，不创建 Pull Request。
- 不提交：构建产物（`target/`）、OpenSpec 中间产物（`openspec/changes/`）、秘密、个人本地配置（见 `.gitignore`）。

---

## 12. 智能体行为准则

1. **先读后写**：修改文件前阅读它及其相关测试；修改接口前搜索所有调用方。
2. **不扩大范围**：只做当前任务要求的事；发现额外问题时记录并告知用户，不顺手修改。
3. **不猜测关键事实**：库 API、版本、协议细节以实际源码 / 文档 / 编译结果为准；不确定时先验证。
4. **遇到歧义就问**：会改变规范、接口、验收标准或违背决策的问题，必须先问用户；细枝末节可以做合理假设并在产物中记录。
5. **诚实报告**：测试失败、跳过了哪一步、哪些没验证，都要如实说明并附输出；完成并验证的事情直接说明，不含糊其辞。
6. **不绕过门禁**：不关闭 lint、不删测试、不改 CI 放宽条件来“通过”。
7. **保护用户数据与秘密**：不把秘密写入日志、提交或外部服务。
8. **决策留痕**：实现中产生的新决策记入 `docs/decisions.md`（新编号，中英文两个版本），并在对应 OpenSpec 变更中引用。
9. **长任务汇报进度**：阶段性完成时简要说明做了什么、下一步是什么。

---

## 13. 完成定义（Definition of Done）

一个任务 / 变更只有在以下全部满足时才算完成：

- [ ] tasks.md 中对应项已勾选，验收方式已实际执行并通过
- [ ] `cargo fmt --all -- --check` 通过
- [ ] `cargo clippy --workspace --all-targets --all-features -- -D warnings` 通过
- [ ] `cargo test --workspace --all-features` 通过
- [ ] 涉及 runtime 可用的 crate：`cargo build -p <crate> --no-default-features --target wasm32-unknown-unknown` 通过
- [ ] `cargo deny check` 与 `cargo audit` 通过
- [ ] 新增 / 修改的公开 API 有 rustdoc，相关 README / docs 已更新（英文与 `*.zh-CN.md` 两个版本同步）
- [ ] 每个相关 spec Scenario 有对应测试
- [ ] 未违反 §2 红线；有意偏离编码规范之处已注明编号与理由
- [ ] 已提交并推送到指定分支；CI 全绿

---

## 14. 常用命令

```bash
# 质量检查（与 CI 一致）
cargo fmt --all -- --check
#（--all-features 会打开 revive 的基准；SKIP_PALLET_REVIVE_FIXTURES 跳过其测试合约的编译，
#  编译它们需要 RISC-V 工具链与 resolc，见 runtime/README.zh-CN.md）
SKIP_WASM_BUILD=1 SKIP_PALLET_REVIVE_FIXTURES=1 cargo clippy --workspace --all-targets --all-features -- -D warnings
SKIP_PALLET_REVIVE_FIXTURES=1 cargo test --workspace --all-features
cargo build -p ac-crypto --no-default-features --target wasm32-unknown-unknown
cargo deny check
cargo audit

# 供应链、许可证与安全公告
scripts/check-license-boundary.sh [--self-test]     # 许可证分区（D47）与内部开关
scripts/check-release-runtime.sh <wasm>|--self-test # 正式 runtime 不含基准测试 API
scripts/sync-audit-exceptions.py [--write]        # 安全公告例外（编辑 audit-exceptions.toml）
scripts/benchmark-pallet.sh <pallet> <weights.rs> # 重新生成基准权重
scripts/gen-toploc-vectors.sh                     # 由参考实现重新生成 TOPLOC 向量（Python）

# 节点与端到端
cargo build -p ac-node -p ac-wallet -p ac-provider -p ac-gateway -p ac-mock-engine
scripts/run-local-testnet.sh [--check]            # 四节点本地测试网（检查最佳与已最终确定高度）
AC_E2E=1 cargo test -p ac-e2e -- --test-threads 1  # 多节点验收测试
AC_E2E=1 cargo test -p ac-e2e --test market -- --test-threads 1  # 经钱包的市场登记流程
AC_E2E=1 cargo test -p ac-e2e --test settlement -- --test-threads 1  # 收据、报告与领取
AC_E2E=1 cargo test -p ac-e2e --test inference -- --test-threads 1  # OpenAI SDK 经代理、网关、提供者（Python：pip install -r tests/e2e/python/requirements.txt）
scripts/measure-finality.sh [seconds]             # 终局性延迟，4/7/10 节点（release）
scripts/wallet-smoke.sh                           # 针对开发节点运行钱包命令行

# OpenSpec
openspec list                 # 进行中的变更
openspec list --specs         # 已有能力规范
openspec status --change <name>
openspec validate <name> --strict

# Rust 编码规范
grep -n '<关键词或编号>' docs/rust-guidelines/rules.tsv
python3 scripts/gen-rust-guidelines-index.py      # 重建索引
scripts/update-rust-guidelines.sh [<commit>]      # 更新规范快照
```
