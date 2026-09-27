# governance/poa-admin Specification

## Purpose
规定 PoA 阶段的管理权限：由创世设定的一组 ML-DSA 账户按门限共同决议，决议以 Root 身份执行，覆盖 runtime 升级、国库支出和成员变更；链上治理上线后移交。

## Requirements

### Requirement: 创世成员与门限
管理权限 SHALL 由创世设定的成员账户与门限 t 组成（1 ≤ t ≤ 成员数）。正式链创世 MUST 设置至少一名成员；成员账户与其他账户一样只用 ML-DSA 签名授权。

#### Scenario: 缺少成员的正式链创世
- **WHEN** 以没有管理成员的正式链预设构建创世
- **THEN** 创世构建失败

### Requirement: 提案、批准与执行
任一成员 SHALL 能对一个调用发起提案；其他成员可批准；批准数达到门限后，该调用 MUST 以 Root 身份执行且只执行一次，执行结果通过事件公布。非成员 MUST 不能发起或批准提案。提案 MUST 由其调用内容的 BLAKE3 哈希标识。

#### Scenario: 达到门限后执行
- **WHEN** 门限为 2 的本地链上，alice 提议一笔国库支出、bob 批准
- **THEN** 支出以 Root 身份执行一次，发出执行事件

#### Scenario: 未达门限不执行
- **WHEN** 只有提议者本人批准
- **THEN** 调用不被执行，状态不变

#### Scenario: 非成员被拒绝
- **WHEN** 一个非成员账户发起提案
- **THEN** 交易失败

### Requirement: runtime 升级
runtime 升级 SHALL 只能通过管理权限的决议执行。升级后的 runtime 仍受节点不变量约束。

#### Scenario: 多签升级
- **WHEN** 管理权限通过决议把 runtime 升级为新版本
- **THEN** 新版本在随后的区块生效，runtime 版本号随之变化

### Requirement: 成员变更
成员集合与门限 SHALL 只能通过管理权限自身的决议变更；变更后的门限 MUST 仍满足 1 ≤ t ≤ 成员数。

#### Scenario: 替换成员
- **WHEN** 管理权限决议把成员从 alice、bob、charlie 改为 alice、bob、dave，门限保持 2
- **THEN** 之后 charlie 不能再提议或批准，dave 可以
