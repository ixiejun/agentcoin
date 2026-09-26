# crypto/pq-signature Specification

## Purpose
提供符合 FIPS 204 的 ML-DSA-44/65/87 后量子签名能力（确定性密钥生成、签名、验证与上下文域分离），作为 AgentCoin 账户授权与共识签名的唯一签名算法族（MVP 阶段）。

## Requirements

### Requirement: 从种子确定性生成密钥
系统 SHALL 支持从 32 字节种子（FIPS 204 中的 ξ）为 ML-DSA-44/65/87 生成密钥对；相同算法与相同种子 MUST 生成完全相同的密钥对。系统也 SHALL 支持使用调用方提供的密码学安全随机源生成种子。

#### Scenario: 相同种子得到相同密钥
- **WHEN** 用同一个 32 字节种子两次生成 ML-DSA-65 密钥对
- **THEN** 两次得到的公钥逐字节相同

#### Scenario: 与 NIST keyGen 向量一致
- **WHEN** 对 NIST ACVP ML-DSA keyGen 向量子集中的每个用例，用其种子生成密钥
- **THEN** 生成的公钥与私钥编码都与向量的期望值逐字节相同

### Requirement: 带上下文的签名与验证
签名与验证 SHALL 采用 FIPS 204 的 pure ML-DSA 接口，并要求调用方提供上下文字符串（0–255 字节）。用某个上下文生成的签名 MUST 只能在同一上下文下验证通过。上下文超过 255 字节时 MUST 返回错误。

#### Scenario: 同一上下文验证通过
- **WHEN** 用上下文 `agentcoin/tx/v1` 对消息签名，再用同一上下文和对应公钥验证
- **THEN** 验证通过

#### Scenario: 不同上下文验证失败
- **WHEN** 用上下文 `agentcoin/tx/v1` 生成的签名，在上下文 `agentcoin/vote/v1` 下验证
- **THEN** 验证失败

#### Scenario: 上下文过长
- **WHEN** 以 256 字节的上下文请求签名或验证
- **THEN** 返回“上下文过长”错误

#### Scenario: 篡改消息或签名
- **WHEN** 对一个合法签名翻转消息或签名中的任意一个比特后验证
- **THEN** 验证失败

### Requirement: 签名模式
系统 SHALL 默认使用 hedged（加入新鲜随机数）签名；并 SHALL 提供确定性签名模式，供测试与可复现场景使用。两种模式产生的签名 MUST 都能用同一公钥验证通过。

#### Scenario: hedged 签名每次不同
- **WHEN** 用 hedged 模式对同一消息签名两次
- **THEN** 两个签名不同，且都验证通过

#### Scenario: 确定性签名可复现
- **WHEN** 用确定性模式对同一消息、同一上下文签名两次
- **THEN** 两个签名逐字节相同

### Requirement: NIST 签名与验证向量一致性
实现 SHALL 通过 NIST ACVP ML-DSA sigGen（确定性用例、外部 / pure 接口）与 sigVer 向量子集的全部用例；每个参数集（44/65/87）都 MUST 至少覆盖 10 个用例，并且 sigVer 子集 MUST 同时包含期望通过与期望失败的用例。

#### Scenario: sigGen 向量
- **WHEN** 对 sigGen 子集的每个用例，用给定私钥、消息、上下文进行确定性签名
- **THEN** 得到的签名与期望值逐字节相同

#### Scenario: sigVer 向量
- **WHEN** 对 sigVer 子集的每个用例执行验证
- **THEN** 验证结果与向量标注的通过 / 失败完全一致

### Requirement: 验证能力可在 no_std 环境使用
公钥解码与签名验证 SHALL 在无标准库、无随机源、无操作系统的环境（例如 WASM runtime）中可用；密钥生成与 hedged 签名 MAY 需要额外启用随机源。

#### Scenario: WASM 构建
- **WHEN** 以关闭默认功能的方式为 `wasm32-unknown-unknown` 目标构建本能力
- **THEN** 构建成功，且验证接口可用

### Requirement: 私钥材料的保护
私钥与种子 SHALL 在不再使用时被清零；其调试输出 MUST NOT 暴露任何私钥字节。

#### Scenario: 调试输出脱敏
- **WHEN** 以调试格式打印一个私钥对象
- **THEN** 输出中不包含私钥或种子的任何字节
