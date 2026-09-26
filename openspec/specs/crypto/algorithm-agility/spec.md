# crypto/algorithm-agility Specification

## Purpose
定义 AgentCoin 所有密码学对象（公钥、签名、KEM 公钥与密文）的算法标识（AlgId）与带标签的线格式，使任何算法都能通过新增编号来引入或替换，而不改变已有数据的含义；并保证规范编码、链上存储编码与链上类型元数据描述的是同一种字节格式。

## Requirements

### Requirement: 稳定的算法编号表
系统 SHALL 在每个类别（签名、KEM）内为每个算法分配一个 1 字节编号（AlgId，取值 0x01–0xFE）；各类别的编号空间相互独立。编号一经发布 MUST NOT 被复用或改变含义；`0x00` MUST NOT 被分配；`0xFF` 在每个类别中都保留为将来的扩展标记，MUST NOT 分配给具体算法。首版编号表如下：

| 类别 | AlgId | 算法 | 状态 |
|---|---|---|---|
| 签名 | `0x01` | ML-DSA-44 | 已实现 |
| 签名 | `0x02` | ML-DSA-65 | 已实现 |
| 签名 | `0x03` | ML-DSA-87 | 已实现 |
| 签名 | `0x10` | SLH-DSA-SHA2-128s | 预留 |
| 签名 | `0x20` | FN-DSA-512 | 预留 |
| 签名 | `0x30` | XMSS（lean 变体） | 预留 |
| KEM | `0x01` | X-Wing（ML-KEM-768 + X25519） | 已实现 |
| KEM | `0x02` | ML-KEM-1024 | 预留 |
| 各类别 | `0xFF` | 扩展标记 | 保留 |

#### Scenario: 已实现算法的编号往返
- **WHEN** 把任一算法转换为 AlgId，再从该 AlgId 转换回来
- **THEN** 得到同一个算法，且数值与上表一致

#### Scenario: 编号表被固化
- **WHEN** 运行编号表回归测试
- **THEN** 测试断言上表中的每个 AlgId 数值都与其算法一一对应，任何改动都会导致测试失败

#### Scenario: 保留编号不可用
- **WHEN** 查询编号 `0x00` 或 `0xFF`
- **THEN** 返回“未知算法”错误

### Requirement: 带标签的线格式
所有公钥、签名、KEM 公钥和 KEM 密文的规范编码（canonical encoding）SHALL 为：1 字节 AlgId，后接该算法规定的固定长度原始字节，且不含其他任何字节。各算法的原始字节长度 MUST 如下：

| 算法 | 公钥 | 签名 / 密文 |
|---|---|---|
| ML-DSA-44 | 1312 | 2420 |
| ML-DSA-65 | 1952 | 3309 |
| ML-DSA-87 | 2592 | 4627 |
| X-Wing | 1216 | 1120 |

#### Scenario: 编码再解码得到相同对象
- **WHEN** 对一个合法的 ML-DSA-65 公钥进行规范编码，再解码
- **THEN** 解码得到的公钥与原公钥逐字节相等，编码以 `0x02` 开头，长度为 1 + 1952 字节

#### Scenario: 长度不符被拒绝
- **WHEN** 解码一个 AlgId 为 ML-DSA-44、但原始字节只有 2419 字节或多出 1 字节的签名
- **THEN** 解码返回“长度错误”，而不是截断或填充

### Requirement: 未知与预留算法的安全处理
系统在遇到未知 AlgId 或“预留但未实现”的 AlgId 时 SHALL 返回明确的错误，MUST NOT panic、MUST NOT 回退到其他算法，并且在验证场景中 MUST 视为验证失败。

#### Scenario: 未知 AlgId
- **WHEN** 解码一个以 `0xEE`（未分配）开头的字节串
- **THEN** 返回“未知算法”错误

#### Scenario: 预留算法
- **WHEN** 请求用 AlgId `0x10`（SLH-DSA，预留）生成密钥或解码对象
- **THEN** 返回“算法未实现”错误

### Requirement: 跨算法混用被拒绝
签名验证 SHALL 要求公钥与签名的 AlgId 相同；二者不一致时 MUST 判定为验证失败，而不是尝试按其中任一算法验证。

#### Scenario: ML-DSA-44 公钥配 ML-DSA-65 签名
- **WHEN** 用 ML-DSA-44 公钥去验证一个 ML-DSA-65 签名
- **THEN** 返回“算法不匹配”错误，验证失败

### Requirement: 单一字节形式与准确的链上类型元数据
在启用相应功能开关时，系统 SHALL 为所有带标签类型提供 SCALE 编解码、最大编码长度与类型元数据（TypeInfo）。SCALE 编码 MUST 与规范编码逐字节一致；类型元数据 MUST 把每个带标签类型描述为以 AlgId 作为变体序号、以固定长度字节数组作为载荷的枚举，从而使按元数据解码的结果与规范解码完全一致。

#### Scenario: SCALE 编码与规范编码一致
- **WHEN** 对同一个签名分别进行 SCALE 编码和规范编码
- **THEN** 两者逐字节相同

#### Scenario: 类型元数据与字节格式一致
- **WHEN** 读取 `PqSignature` 的类型元数据
- **THEN** 它是一个枚举，每个已实现算法对应一个变体，变体序号等于该算法的 AlgId，载荷是长度等于该算法签名长度的字节数组
