# crypto/hashing Specification

## Purpose
定义 AgentCoin 统一使用的 256 位哈希函数、域分离规则和 32 字节账户 ID 派生规则；账户 ID 一经主网使用即成为不可更改的共识格式。

## Requirements

### Requirement: 256 位哈希
系统 SHALL 提供 BLAKE3（32 字节输出）作为通用哈希，并提供 SHA3-256 用于互操作；所有输出 MUST 为 32 字节。

#### Scenario: 官方向量
- **WHEN** 对 BLAKE3 官方测试向量与 SHA3-256 的 NIST 示例向量计算哈希
- **THEN** 结果与期望值逐字节相同

### Requirement: 域分离哈希
系统 SHALL 提供域分离哈希：调用方提供一个全局唯一的上下文字符串（格式为 `agentcoin <YYYY-MM> <用途> v<版本>`），使用 BLAKE3 的 derive_key 模式计算。不同上下文对相同输入 MUST 产生不同的输出。

#### Scenario: 不同域输出不同
- **WHEN** 以两个不同的上下文字符串对同一输入计算域分离哈希
- **THEN** 两个输出不同

#### Scenario: 上下文格式校验
- **WHEN** 以不符合约定格式的上下文字符串（例如空字符串）调用域分离哈希
- **THEN** 返回“上下文格式错误”

### Requirement: 账户 ID 派生
账户 ID SHALL 为 32 字节，计算方式为：以上下文 `agentcoin 2026-09 account-id v1`、对“1 字节 AlgId ‖ 公钥原始字节”做 BLAKE3 derive_key。该规则 MUST 被固定为回归测试；相同公钥 MUST 始终得到相同账户 ID，不同算法的公钥即使原始字节相同也 MUST 得到不同账户 ID。

#### Scenario: 固定向量
- **WHEN** 用仓库内固定的 ML-DSA-44 与 ML-DSA-65 示例公钥派生账户 ID
- **THEN** 结果与仓库中记录的期望账户 ID 逐字节相同

#### Scenario: 算法参与派生
- **WHEN** 对同一段原始字节分别以 AlgId `0x01` 和 `0x02` 作为前缀派生账户 ID
- **THEN** 两个账户 ID 不同

### Requirement: Poseidon2 哈希
系统 SHALL 提供 Poseidon2 哈希，参数（素数域、状态宽度、S-box 次数、轮数、轮常数与矩阵的生成方式）在设计中固定，并满足以下条件：
- 输出 256 位（32 字节，按设计固定的规则由域元素编码）；
- 按 Poseidon2 论文的安全分析，达到至少 128 位安全；
- 实现来自经过审阅、公开维护的开源实现，只做包装，不自行实现置换；
- 实现 MUST 通过所选实现上游公布的该参数实例的置换已知答案向量。所选实例（Plonky3 的 Goldilocks 实例）不是 Poseidon2 作者参考实现的实例，没有作者向量；这一偏离经用户决定（2026-09-28），记为决策并在设计中写明来源。

字节输入到域元素的编码 SHALL 是单射的，并在设计中固定。长度不同的输入 MUST 不会被编码成相同的域元素序列。

#### Scenario: 上游置换向量
- **WHEN** 对所选参数实例运行上游公布的置换已知答案向量中的输入
- **THEN** 输出与向量逐元素一致

#### Scenario: 哈希回归向量
- **WHEN** 对设计中列出的字节输入（含空输入、31 字节、32 字节、1,000 字节）计算 Poseidon2 哈希
- **THEN** 输出与仓库中固定的回归向量逐字节一致

#### Scenario: 编码单射
- **WHEN** 对仅在末尾多一个零字节的两个输入计算哈希
- **THEN** 两个输出不同

#### Scenario: 无标准库可用
- **WHEN** 为 `wasm32-unknown-unknown` 以无标准库方式构建哈希功能
- **THEN** 构建成功，结果与标准库构建一致
