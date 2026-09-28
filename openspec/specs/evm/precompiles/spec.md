# evm/precompiles Specification

## Purpose
为 EVM 合约提供后量子密码学能力：在合约中验证 ML-DSA 签名、计算 BLAKE3 与 Poseidon2 哈希，并为全量版的 STARK 验证保留固定地址。

## Requirements

### Requirement: 预编译地址固定
PQ 预编译 SHALL 位于以下固定地址（20 字节中前 16 字节为零，接着两字节编号，末两字节为零），地址一经发布永不更改：
- `pq_verify`：`0x000000000000000000000000000000000a010000`；
- `blake3`：`0x000000000000000000000000000000000a020000`；
- `poseidon2`：`0x000000000000000000000000000000000a030000`；
- `stark_verify`（保留）：`0x000000000000000000000000000000000a100000`。

每个预编译 SHALL 以 Solidity ABI 接口调用（接口定义随 `contracts/` 发布）。调用数据无法按接口解码时，调用 MUST 回滚。

#### Scenario: 地址回归
- **WHEN** 读取四个预编译的地址常量
- **THEN** 与规格给出的地址逐字节一致

#### Scenario: 无法解码的输入
- **WHEN** 合约以不符合接口的调用数据调用 `blake3` 预编译
- **THEN** 调用回滚，调用方可以捕获失败

### Requirement: pq_verify 验证 ML-DSA 签名
`pq_verify` SHALL 接收 AlgId（1 字节）、公钥、消息和签名，按 AlgId 选定的算法验证签名，返回布尔值。验证 MUST 固定使用上下文 `agentcoin/evm-verify/v1`，该上下文 MUST 在上下文登记表中登记且不被其他用途使用；以其他上下文（包括交易签名上下文 `agentcoin/tx/v1`）生成的签名 MUST 验证失败。未知 AlgId、预留但未实现的算法、公钥或签名长度错误、签名无效时 MUST 返回 `false`，MUST NOT 回退到其他算法。计费 SHALL 按算法的基准测试权重加上按消息长度计的部分。

#### Scenario: 有效签名
- **WHEN** 用钱包以 `agentcoin/evm-verify/v1` 对消息 m 签名 ML-DSA-65 签名，合约以 (AlgId, 公钥, m, 签名) 调用 `pq_verify`
- **THEN** 返回 `true`

#### Scenario: 协议签名不能在合约中验证
- **WHEN** 合约用同一公钥和载荷，调用 `pq_verify` 验证一个以 `agentcoin/tx/v1` 生成的签名
- **THEN** 返回 `false`

#### Scenario: 篡改与未知算法
- **WHEN** 消息被改动 1 位，或 AlgId 是未分配或预留未实现的值，或签名长度错误
- **THEN** 返回 `false`，调用本身不回滚

#### Scenario: 覆盖 ML-DSA 三个参数集
- **WHEN** 分别用 ML-DSA-44、65、87 的有效签名调用
- **THEN** 都返回 `true`，且各自消耗的 gas 与该参数集的基准权重一致

### Requirement: blake3 预编译
`blake3` SHALL 对输入字节计算普通模式（非密钥、非派生）的 BLAKE3，输出 32 字节。计费 SHALL 为基础成本加按输入长度线性增长的部分，均来自基准测试。

#### Scenario: 官方向量
- **WHEN** 以 BLAKE3 官方测试向量中的输入调用
- **THEN** 返回与向量一致的 32 字节摘要

#### Scenario: 与 ac-crypto 一致
- **WHEN** 对随机输入分别调用预编译和链下 BLAKE3
- **THEN** 结果相同

### Requirement: poseidon2 预编译
`poseidon2` SHALL 用 `crypto/hashing` 中固定的 Poseidon2 参数对输入计算哈希，输出 32 字节。输入字节到域元素的编码规则 SHALL 在设计中固定并有测试向量；无法按规则编码的输入 MUST 使调用回滚。计费 SHALL 来自基准测试。

#### Scenario: 向量一致
- **WHEN** 以设计给出的测试向量输入调用
- **THEN** 输出与向量一致，且与链下 `crypto/hashing` 的 Poseidon2 结果相同

### Requirement: stark_verify 只保留地址
本里程碑中，对 `stark_verify` 地址的任何调用 SHALL 回滚。该地址 MUST NOT 被其他预编译或合约占用，以便全量版在不改地址的前提下启用。

#### Scenario: 保留地址回滚
- **WHEN** 合约调用 `stark_verify` 地址
- **THEN** 调用回滚

#### Scenario: 保留地址不可部署
- **WHEN** 任何部署试图在该地址上创建合约
- **THEN** 部署失败

### Requirement: 预编译不改变链状态
PQ 预编译 SHALL 是纯函数：不读写存储，不创建账户，不转移或增发余额。

#### Scenario: 首次调用不创建账户
- **WHEN** 首次调用 `pq_verify`、`blake3` 或 `poseidon2`
- **THEN** 预编译地址上不产生账户，总发行量不变

### Requirement: 内置以太坊预编译只用于应用层
EVM 中与以太坊兼容的内置预编译（如 `ecrecover`、`sha256`）SHALL 仅作为应用层工具提供。协议的任何授权、共识或承诺路径 MUST NOT 依赖它们，`ecrecover` 的结果 MUST NOT 被链用于账户授权。

#### Scenario: ecrecover 不能授权
- **WHEN** 某人拿着 secp256k1 私钥和对应的 `ecrecover` 结果，试图以该地址为发起者提交交易
- **THEN** 交易被拒绝，因为没有任何入口接受 secp256k1 签名
