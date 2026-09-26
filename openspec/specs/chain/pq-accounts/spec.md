# chain/pq-accounts Specification

## Purpose
规定账户 ID 与后量子公钥的绑定、公钥登记表和带持有证明的密钥轮换，使账户在更换密钥或迁移算法时地址保持不变。

## Requirements

### Requirement: 账户 ID 与首个公钥的绑定
账户 ID SHALL 按 `crypto/hashing` 中的账户 ID 派生规则由账户的**首个**公钥计算得到；账户 ID 在账户存续期间 MUST 保持不变，与之后轮换到的公钥和算法无关。任何人 SHALL 能在离线状态下由公钥算出地址并向其转账，而无需该账户先上链登记。

#### Scenario: 向未登记账户转账
- **WHEN** 向一个仅由公钥离线派生、从未上链的账户 ID 转入不低于存在性押金的金额
- **THEN** 转账成功，该账户余额增加，其公钥仍处于未登记状态

### Requirement: 公钥登记表
链 SHALL 为每个已登记账户保存其当前公钥（带 AlgId）与轮换计数，并 SHALL 通过公开的存储查询与 runtime API 提供按账户 ID 查询当前公钥与算法的能力。登记表中的每个公钥 MUST 属于已实现的签名算法。

#### Scenario: 查询当前公钥
- **WHEN** 查询一个已完成首笔交易的账户的当前公钥
- **THEN** 返回首笔交易携带的公钥及其算法，轮换计数为 0

#### Scenario: 查询未登记账户
- **WHEN** 查询一个从未发过交易的账户的当前公钥
- **THEN** 返回“未登记”

### Requirement: 密钥轮换
已登记账户 SHALL 能提交 `rotate_key` 交易，把当前公钥替换为新公钥（可为相同或不同的已实现签名算法）。该交易 MUST 由当前公钥按 `chain/pq-transaction-auth` 授权，并 MUST 携带新公钥对“轮换声明”的持有证明签名，上下文为 `agentcoin/key-rotation/v1`；轮换声明 MUST 包含创世哈希、账户 ID、当前轮换计数与新公钥。成功后账户 ID、余额与 nonce MUST 不变，轮换计数加一。

#### Scenario: 轮换后账户 ID 不变
- **WHEN** 账户用 ML-DSA-44 当前密钥提交携带有效持有证明的 `rotate_key`，新公钥为 ML-DSA-65
- **THEN** 交易成功；该账户的账户 ID 与余额不变，当前公钥变为新 ML-DSA-65 公钥，轮换计数为 1

#### Scenario: 轮换后旧密钥失效
- **WHEN** 轮换成功后，用旧私钥签名一笔新交易并提交
- **THEN** 交易以“签名无效”被拒绝

#### Scenario: 轮换后新密钥可用
- **WHEN** 轮换成功后，用新私钥签名一笔转账并提交
- **THEN** 转账执行成功

#### Scenario: 缺少或伪造持有证明
- **WHEN** 提交的 `rotate_key` 中持有证明不是由新私钥对正确轮换声明签出的（例如由旧私钥签名，或声明中的轮换计数错误）
- **THEN** 交易执行失败，当前公钥与轮换计数不变

#### Scenario: 持有证明不能跨账户重放
- **WHEN** 把账户 A 的轮换持有证明用于账户 B 的 `rotate_key`
- **THEN** 交易执行失败

### Requirement: 公钥不可被他人占用
一个公钥 SHALL 至多作为一个账户的当前公钥；`rotate_key` 的新公钥若已是任何账户的当前公钥，或其派生出的账户 ID 已是另一个存在的账户，交易 MUST 失败。

#### Scenario: 轮换到他人的公钥
- **WHEN** 账户 A 试图把当前公钥轮换为账户 B 的当前公钥
- **THEN** 交易执行失败

### Requirement: 预留算法的迁移路径
当某个预留算法（SLH-DSA、FN-DSA、XMSS）将来被实现并通过 runtime 升级启用后，已有账户 SHALL 仅通过 `rotate_key` 即可迁移到该算法，不需要修改账户 ID、存储格式或既有交易格式。在其被启用之前，轮换到预留算法 MUST 失败。

#### Scenario: 轮换到未实现的算法
- **WHEN** 提交新公钥 AlgId 为 FN-DSA-512 的 `rotate_key`
- **THEN** 交易失败，当前公钥不变

### Requirement: 轮换操作计量
`rotate_key` 的权重 SHALL 由基准测试得出，并覆盖持有证明验证的最坏情况（最大的已实现签名算法）。

#### Scenario: 最坏情况权重
- **WHEN** 对 `rotate_key` 运行基准测试
- **THEN** 生成的权重不低于用 ML-DSA-87 新公钥完成轮换的实测成本
