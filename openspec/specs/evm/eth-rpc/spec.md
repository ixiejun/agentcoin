# evm/eth-rpc Specification

## Purpose
以以太坊 JSON-RPC 的形式对外提供链上合约状态、区块、回执与日志，使 Foundry 等工具能够读链和查询交易结果；交易提交只接受已由 ML-DSA 签名的原生交易。

## Requirements

### Requirement: 支持的只读方法
适配器 SHALL 提供以下以太坊 JSON-RPC 方法，参数与返回格式遵循以太坊 JSON-RPC 规范：`web3_clientVersion`、`net_version`、`eth_chainId`、`eth_syncing`、`eth_blockNumber`、`eth_accounts`（恒为空数组）、`eth_gasPrice`、`eth_maxPriorityFeePerGas`、`eth_feeHistory`、`eth_getBalance`、`eth_getTransactionCount`、`eth_getCode`、`eth_getStorageAt`、`eth_call`、`eth_estimateGas`、`eth_getBlockByNumber`、`eth_getBlockByHash`、`eth_getTransactionByHash`、`eth_getTransactionReceipt`、`eth_getLogs`、`eth_sendRawTransaction`。未列出的方法 MUST 返回“方法不存在”错误。

#### Scenario: Foundry 读取
- **WHEN** 对适配器执行 `cast chain-id`、`cast block-number`、`cast balance <地址>`、`cast code <合约>`、`cast call <ERC-20> "balanceOf(address)" <地址>`
- **THEN** 命令成功，返回值与链上状态一致

#### Scenario: 未支持的方法
- **WHEN** 调用 `eth_newFilter` 或 `debug_traceTransaction`
- **THEN** 返回 JSON-RPC 错误码 -32601

### Requirement: 区块标签与终局性
适配器 SHALL 支持区块参数 `latest`、`pending`（按 `latest` 处理）、`earliest`、`safe`、`finalized` 以及具体区块号或区块哈希。`safe` 和 `finalized` MUST 指向 AC-BFT 最新的最终确定区块。区块中的 `hash` 字段 MUST 等于合约在该区块执行 `blockhash` 时得到的值。

#### Scenario: finalized 标签
- **WHEN** 以 `finalized` 调用 `eth_getBlockByNumber`
- **THEN** 返回的区块号等于节点报告的最新最终确定区块号

#### Scenario: 区块哈希一致
- **WHEN** 合约在第 n+1 块读取 `blockhash(n)`，同时通过适配器查询第 n 块
- **THEN** 两者相同

### Requirement: 交易提交只接受 PQ 签名的原生交易
`eth_sendRawTransaction` SHALL 只接受以下字节：一笔已由 ML-DSA 签名的 AgentCoin 原生交易，且其调用是合约部署或合约调用。适配器 MUST 在转发前解码并检查调用类型，通过后提交给节点，返回该交易的 32 字节交易哈希（与节点报告的交易哈希相同）。以下情况 MUST 返回错误，且不向节点提交任何内容：RLP 编码的以太坊交易（任何类型）、其他调用类型的原生交易、无法解码的字节。`eth_sendTransaction`、`eth_sign`、`eth_signTransaction`、`eth_signTypedData*`、`personal_*` MUST 返回错误，错误信息指引使用 `ac-wallet evm` 签名。

#### Scenario: 提交原生合约交易
- **WHEN** 把 `ac-wallet evm send` 生成的已签名交易字节传给 `eth_sendRawTransaction`
- **THEN** 返回交易哈希，交易随后被打包，其回执可按该哈希查询

#### Scenario: 拒绝以太坊签名交易
- **WHEN** 把一笔 secp256k1 签名的 EIP-1559 交易传给 `eth_sendRawTransaction`
- **THEN** 返回错误，信息说明只接受 ML-DSA 签名的原生交易，节点交易池中没有新增交易

#### Scenario: 拒绝非合约调用
- **WHEN** 提交一笔签名有效的原生转账交易
- **THEN** 返回错误，交易不被转发

#### Scenario: 拒绝节点签名
- **WHEN** 调用 `eth_sendTransaction`
- **THEN** 返回错误，信息中给出 `ac-wallet evm` 的用法

### Requirement: 交易与回执
适配器 SHALL 对其索引范围内的合约交易返回交易对象和回执：交易哈希、区块号与区块哈希、发起者 EVM 地址、`to`（部署时为空）、`contractAddress`（部署时为新合约地址）、`status`（成功为 1，失败或回滚为 0）、日志、`gasUsed` 与 `effectiveGasPrice`。`gasUsed × effectiveGasPrice` 与该交易实际支付的手续费之差 MUST 小于一个 gas 单位的价格。索引范围 SHALL 至少覆盖适配器启动前最近的可配置数量（默认 10,000）的区块以及启动后的全部区块；范围外或不存在的交易 MUST 返回 `null`。

#### Scenario: 部署回执
- **WHEN** 部署交易被打包后查询其回执
- **THEN** `status` 为 1，`contractAddress` 等于链上新合约的地址，`to` 为空

#### Scenario: 回滚回执
- **WHEN** 一笔合约调用在执行中回滚，随后查询其回执
- **THEN** `status` 为 0，日志为空，手续费仍被扣除且与 `gasUsed × effectiveGasPrice` 一致

#### Scenario: 未知交易
- **WHEN** 查询一个不存在的交易哈希
- **THEN** 返回 `null`

### Requirement: 日志查询
`eth_getLogs` SHALL 按区块范围（或单个区块哈希）、合约地址（单个或列表）和主题（含 `null` 通配与每位置多选）过滤合约事件，返回的日志字段（地址、主题、数据、区块号、区块哈希、交易哈希、交易序号、日志序号）MUST 与合约实际发出的事件一致，顺序与链上执行顺序相同。单次查询的区块范围超过可配置上限（默认 1,000 块）时 MUST 返回错误，而不是返回截断的结果。

#### Scenario: 按主题过滤
- **WHEN** 对一个区块范围查询 ERC-20 的 `Transfer` 主题，并指定接收方主题
- **THEN** 只返回该接收方的 `Transfer` 事件，数据与链上一致

#### Scenario: 超出范围上限
- **WHEN** 查询跨度 1,001 个区块的日志
- **THEN** 返回错误，说明范围上限

### Requirement: 模拟调用与估算
`eth_call` 与 `eth_estimateGas` SHALL 使用链上只读查询（`evm/contracts`）在指定区块状态上执行，调用者可以是任意地址。回滚时 `eth_call` MUST 返回 JSON-RPC 错误码 3，并附回滚数据。`eth_estimateGas` 的结果 MUST 足以让同样参数的交易在同一状态下执行成功。

#### Scenario: eth_call 回滚
- **WHEN** 以 `eth_call` 调用一个必然 `revert("no")` 的函数
- **THEN** 返回错误码 3，错误数据是 `Error(string)` 编码的 "no"

#### Scenario: 估算足够
- **WHEN** 按 `eth_estimateGas` 的结果设定上限提交同样的调用
- **THEN** 交易执行成功

### Requirement: 适配器的边界
适配器 SHALL 只通过节点的公开 RPC 与链交互，不持有任何私钥，不代任何人签名。它 MUST NOT 记录请求参数或返回内容（只允许记录方法名、耗时和错误类别）。

#### Scenario: 无私钥
- **WHEN** 在没有任何钱包文件的环境下启动适配器
- **THEN** 适配器正常提供全部只读方法，且 `eth_accounts` 返回空数组

#### Scenario: 日志不含请求内容
- **WHEN** 以调试日志级别运行适配器并执行一次 `eth_call`
- **THEN** 日志中不出现调用数据或返回数据
