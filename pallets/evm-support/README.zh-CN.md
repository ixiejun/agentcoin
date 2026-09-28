> 🌐 [English](README.md) | **简体中文**

# pallet-evm-support

AgentCoin runtime 的 EVM 支持（m4-evm）。`pallet-revive` 负责运行 EVM 合约，本模块让它遵守 AgentCoin 的规则：

- **付费账户**（`EvmPayer`、`SetEvmPayer`）：合约交易（`Revive::call`、`Revive::instantiate_with_code`，或被 `Revive::dispatch_as_fallback_account` 包装的这两种调用）的签名者在执行期间被记为付费账户。该交易扩展不携带任何数据；执行结束后清除付费账户，每个区块结束时再清除一次。模拟执行通过 `Pallet::with_payer` 设置。
- **不增发的货币**（`ReviveCurrency<C, P, B>`）：`pallet-revive` 看到的货币。它把 fungible 接口转交给 `C`（`Balances` 模块），只在 revive 会创造 ATC 的地方例外：`mint_into`（新合约的存在性押金）改为从付费账户转账，`issue` 从付费账户取出，销毁交给 `B`（`Emission` 模块，因此计入 `Emission::TotalBurned`），`set_balance` 永不上调余额，`restore`、`shelve`、`burn_held`、`burn_all_held` 返回错误。任何 EVM 路径都不能铸币（决策 D9；节点的发行量检查会拒绝这样的区块）。
- **revive 自己的账户**：revive 把代码押金冻结在它的模块账户上，上游会在创世时给该账户铸一份存在性押金。本模块改为在创世和 runtime 升级时给该账户加一个 provider 引用，使它以零余额存在。
- **PQ 预编译**（任务组 3）：`pq_verify`、`blake3`、`poseidon2`，以及保留的 `stark_verify` 地址。

## Feature

| Feature | 默认 | 用途 |
|---|---|---|
| `std` | 是 | 原生构建。 |
| `try-runtime` | 否 | 把 `try-runtime` 传递给 FRAME 和 `pallet-revive`。 |

## 示例

示例代码见英文版 [README.md](README.md)（英文版中的示例作为 doctest 运行）：该交易扩展不会给交易增加任何字节。
