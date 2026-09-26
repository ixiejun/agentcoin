# crypto/hybrid-kem Specification

## Purpose
提供后量子与经典算法组合的混合密钥封装（X-Wing：ML-KEM-768 + X25519），用于端到端加密与会话密钥协商，使数据在“先存储、后解密”的量子攻击下仍然安全，同时不低于经典 X25519 的安全性。

## Requirements

### Requirement: 混合 KEM 的密钥生成、封装与解封装
系统 SHALL 提供 X-Wing KEM：从 32 字节种子确定性生成解封装密钥与封装公钥（1216 字节）；封装操作输出密文（1120 字节）与 32 字节共享秘密；解封装操作用私钥从密文恢复同一个共享秘密。

#### Scenario: 封装后解封装得到相同共享秘密
- **WHEN** 对一个封装公钥执行封装，再用对应私钥对得到的密文执行解封装
- **THEN** 双方得到的 32 字节共享秘密逐字节相同

#### Scenario: 相同种子得到相同密钥
- **WHEN** 用同一个 32 字节种子两次生成 X-Wing 密钥对
- **THEN** 两次得到的封装公钥逐字节相同

### Requirement: 错误密文不泄露信息
对被篡改或不属于该私钥的密文，解封装 SHALL 返回一个伪随机的共享秘密（隐式拒绝），MUST NOT 返回能够区分“密文无效”的错误，并且 MUST 与正常共享秘密不同。

#### Scenario: 篡改密文
- **WHEN** 翻转密文中的一个比特后解封装
- **THEN** 解封装不报错，但得到的共享秘密与封装方的不同

### Requirement: 规范与向量一致性
实现 SHALL 通过所采用 X-Wing 规范版本附带的全部测试向量；其 ML-KEM-768 组件 SHALL 通过 NIST ACVP ML-KEM keyGen 与 encapDecap 向量子集中 ML-KEM-768 的全部用例（每类至少 10 个）。

#### Scenario: X-Wing 规范向量
- **WHEN** 对每个 X-Wing 规范向量，用给定种子和封装随机数执行密钥生成与封装
- **THEN** 公钥、密文、共享秘密都与期望值逐字节相同

#### Scenario: ML-KEM-768 NIST 向量
- **WHEN** 对 ACVP ML-KEM-768 的 keyGen 与 encapDecap 用例执行对应操作
- **THEN** 结果与期望值逐字节相同

### Requirement: 带标签的 KEM 对象
封装公钥与密文 SHALL 使用 `crypto/algorithm-agility` 规定的带标签线格式（KEM 类别 AlgId `0x01`）；共享秘密与私钥 MUST NOT 以带标签格式对外序列化，且在不再使用时 MUST 被清零。

#### Scenario: 公钥带标签
- **WHEN** 对一个 X-Wing 封装公钥进行规范编码
- **THEN** 编码以 `0x01` 开头，总长度为 1 + 1216 字节
