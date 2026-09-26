> 🌐 [English](README.md) | **简体中文**

# ac-crypto

AgentCoin 的抗量子密码库。所有公钥、签名和密文都带有**算法编号（AlgId）**，因此新增或替换算法只需要分配一个新编号，不会改变任何已有数据的含义。这是仓库中唯一允许直接调用具体密码学实现的 crate（AGENT.md §6）。

- 签名：ML-DSA-44 / 65 / 87（FIPS 204，pure 接口，必须提供上下文字符串）
- 混合 KEM：X-Wing = ML-KEM-768 + X25519（draft-connolly-cfrg-xwing-kem-06）
- 哈希：BLAKE3-256、SHA3-256、域分离 BLAKE3（`derive_key`）
- 由公钥派生的 32 字节账户 ID
- `no_std`；验证、哈希和账户 ID 既不需要 `std` 也不需要随机源（可用于 WASM runtime）

## 示例

示例代码见英文版 [README.md](README.md)（英文版中的示例作为 doctest 运行）。流程：从种子生成 ML-DSA-44 密钥 → 在上下文 `agentcoin/tx/v1` 下签名 → 同一上下文验证通过、换成 `agentcoin/bft-vote/v1` 验证失败 → 规范编码以 1 字节 AlgId `0x01` 开头、总长 1 + 2420 字节 → 由公钥派生账户 ID。

## AlgId 编号表

每个类别（签名、KEM）各自拥有独立的 1 字节编号空间。编号永不复用，含义永不改变；`0x00` 不分配，`0xFF` 在每个类别中都保留为扩展标记。按算法家族分段：ML-DSA `0x01–0x0F`，SLH-DSA `0x10–0x1F`，FN-DSA `0x20–0x2F`，XMSS `0x30–0x3F`。

| 类别 | AlgId | 算法 | 状态 | 公钥原始长度 | 签名 / 密文原始长度 |
|---|---|---|---|---|---|
| 签名 | `0x01` | ML-DSA-44 | 已实现 | 1312 | 2420 |
| 签名 | `0x02` | ML-DSA-65 | 已实现 | 1952 | 3309 |
| 签名 | `0x03` | ML-DSA-87 | 已实现 | 2592 | 4627 |
| 签名 | `0x10` | SLH-DSA-SHA2-128s | 预留 | — | — |
| 签名 | `0x20` | FN-DSA-512 | 预留 | — | — |
| 签名 | `0x30` | XMSS（lean） | 预留 | — | — |
| KEM | `0x01` | X-Wing（ML-KEM-768 + X25519，draft-06） | 已实现 | 1216 | 1120 |
| KEM | `0x02` | ML-KEM-1024 | 预留 | — | — |
| 各类别 | `0xFF` | 扩展标记 | 保留 | — | — |

## 线格式

`PqPublicKey`、`PqSignature`、`KemPublicKey`、`KemCiphertext` 都是枚举：变体序号就是 AlgId，载荷是该算法的定长字节。它们的规范编码为 `AlgId（1 字节）‖ 原始字节`，这同时就是 SCALE 编码、链上存储编码，也正是 `TypeInfo` 元数据所描述的格式（决策 D34）——全局只有一种字节形式。未知 AlgId、预留 AlgId、长度错误和多余字节都会返回错误。

## 功能开关

| 功能 | 启用内容 | 使用方 |
|---|---|---|
| *（无）* | 类型、解码、验证、哈希、账户 ID（`no_std`，无需随机源） | WASM runtime |
| `std` | 为 `Error` 实现 `std::error::Error` | 原生程序 |
| `rand` | 由随机源生成密钥、hedged 签名、KEM 封装 | 节点、钱包 |
| `kem` | X-Wing 混合 KEM | 节点 P2P、网关、客户端 |
| `deterministic` | 确定性签名、指定随机数的封装 | 仅限测试与工具 |
| `scale` | 带标签类型的 SCALE `Encode` / `Decode` / `MaxEncodedLen` / `TypeInfo` | pallet |

## 上下文登记表

签名上下文（`agentcoin/<用途>/v<版本>`，不超过 255 字节）与哈希上下文（`agentcoin <YYYY-MM> <用途> v<版本>`）必须先在此登记后才能使用，且永不复用于其他用途。

| 上下文 | 类别 | 用途 | 状态 |
|---|---|---|---|
| `agentcoin 2026-09 account-id v1` | 哈希 | 账户 ID 派生 | **使用中（共识关键）** |
| `agentcoin 2026-09 test-rng v1` | 哈希 | 测试中的确定性随机源 | 仅限测试 |
| `agentcoin/tx/v1` | 签名 | 交易签名 | 为 M1 预留 |
| `agentcoin/bft-vote/v1` | 签名 | 最终性投票 | 为 M2 预留 |
| `agentcoin/receipt/v1` | 签名 | 推理回执 | 为 M5 预留 |

## 新增一个算法

1. 通过 OpenSpec 提出（AGENT.md §3），并引用它服务的决策。
2. 在 `src/alg.rs` 中分配新的 AlgId（或把预留编号改为已实现），在 `src/tagged.rs` 中以 `codec(index = AlgId)` 增加变体，并在 `tests/alg_table.rs` 与 `tests/scale_codec.rs` 的固化表中追加——不得修改已有行。
3. 在独立模块中实现后端，只有该模块依赖新的库；在 `sig/mod.rs` 或 `kem/mod.rs` 中按 AlgId 分派。
4. 通过 `scripts/fetch-test-vectors.sh` 加入官方测试向量，并记录在 `tests/vectors/SOURCES.md`。
5. 更新本 README（中英文两个版本）；如需新的上下文，同时更新上下文登记表。

## 测试向量

`tests/vectors/` 存放筛选后的 NIST ACVP（ML-DSA、ML-KEM-768）和 X-Wing 规范向量，可通过 `scripts/fetch-test-vectors.sh` 复现，详见 `tests/vectors/SOURCES.md`。
