> 🌐 [English](README.md) | **简体中文**

# ac-crypto

AgentCoin 的抗量子密码库。所有公钥、签名和密文都带有**算法编号（AlgId）**，因此新增或替换算法只需要分配一个新编号，不会改变任何已有数据的含义。这是仓库中唯一允许直接调用具体密码学实现的 crate（AGENT.md §6）。

- 签名：ML-DSA-44 / 65 / 87（FIPS 204，pure 接口，必须提供上下文字符串）
- 混合 KEM：X-Wing = ML-KEM-768 + X25519（draft-connolly-cfrg-xwing-kem-06）
- 哈希：BLAKE3-256、SHA3-256、域分离 BLAKE3（`derive_key`）
- 由公钥派生的 32 字节账户 ID
- 钱包支持：钱包熵的 24 词 BIP-39 编码、确定性的钱包密钥与开发密钥派生、口令加密的私钥文件（Argon2id + XChaCha20-Poly1305）
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
| `getrandom` | `OsRng`：由操作系统播种的密码学安全随机源，从不 panic | 节点、钱包 |
| `mnemonic` | 钱包熵的 24 词 BIP-39（英文）编码（`no_std`） | 钱包 |
| `keystore` | 口令加密的私钥文件（同时启用 `std`、`rand` 与 `getrandom`） | 节点、钱包 |
| `poseidon2` | Goldilocks 域上的 Poseidon2-256（`no_std`），见下文 | EVM 预编译（runtime） |

密钥种子派生（`wallet_key_seed`、`dev_seed`）始终可用：种子为 `derive_key(上下文, 输入)`，上下文见下表。开发种子是**公开的**，只能用于开发链和本地链。

## 上下文登记表

签名上下文（`agentcoin/<用途>/v<版本>`，不超过 255 字节）与哈希上下文（`agentcoin <YYYY-MM> <用途> v<版本>`）必须先在此登记后才能使用，且永不复用于其他用途。

| 上下文 | 类别 | 用途 | 状态 |
|---|---|---|---|
| `agentcoin 2026-09 account-id v1` | 哈希 | 账户 ID 派生 | **使用中（共识关键）** |
| `agentcoin 2026-09 test-rng v1` | 哈希 | 测试中的确定性随机源 | 仅限测试 |
| `agentcoin 2026-09 tx-payload v1` | 哈希 | 32 字节交易签名载荷 | 自 M1 起使用（共识关键） |
| `agentcoin 2026-09 wallet-key v1` | 哈希 | 钱包密钥种子：`AlgId ‖ u32_le(序号) ‖ 熵` | 自 M1 起使用 |
| `agentcoin 2026-09 dev-seed v1` | 哈希 | 由名称派生的公开开发种子 | 自 M1 起使用（仅限开发链） |
| `agentcoin 2026-09 keystore-aad v1` | 哈希 | 加密私钥文件的附加认证数据 | 自 M1 起使用 |
| `agentcoin 2026-09 os-rng v1` | 哈希 | `OsRng`（由操作系统播种）的输出流 | 自 M1 起使用 |
| `agentcoin 2026-09 bft-message v1` | 哈希 | AC-BFT 消息的 32 字节签名载荷 | 自 M2 起使用（共识关键） |
| `agentcoin 2026-09 randomness-secret v1` | 哈希 | 验证人每个纪元的随机数秘密值：`种子 ‖ 创世哈希 ‖ u64_le(纪元)` | 自 M2 起使用 |
| `agentcoin 2026-09 randomness-commit v1` | 哈希 | 随机数秘密值的承诺 | 自 M2 起使用（共识关键） |
| `agentcoin 2026-09 randomness v1` | 哈希 | 纪元随机数：`u64_le(纪元) ‖ 按账户 ID 排序的揭示值` | 自 M2 起使用（共识关键） |
| `agentcoin 2026-09 randomness-subject v1` | 哈希 | 由纪元随机数按主题派生的值 | 自 M2 起使用 |
| `agentcoin 2026-09 model-id v1` | 哈希 | 模型 ID：权重清单的 SCALE 编码 | 自 M5 起使用 |
| `agentcoin 2026-09 voucher-payload v1` | 哈希 | 透明额度凭证的 32 字节签名载荷 | 自 M5 起使用 |
| `agentcoin/tx/v1` | 签名 | 交易签名 | 自 M1 起使用（共识关键） |
| `agentcoin/aura-seal/v1` | 签名 | Aura-PQ 区块封印 | 自 M1 起使用（共识关键） |
| `agentcoin/key-rotation/v1` | 签名 | 轮换新密钥的持有证明 | 自 M1 起使用（共识关键） |
| `agentcoin/bft-vote/v1` | 签名 | AC-BFT 的全部消息（提议、投票、超时） | 自 M2 起使用（共识关键） |
| `agentcoin/validator-pop/v1` | 签名 | 质押注册的验证人公钥的持有证明 | 自 M3 起使用（共识关键） |
| `agentcoin/evm-verify/v1` | 签名 | 合约通过 `pq_verify` 预编译验证的消息 | 自 M4 起使用 |
| `agentcoin/voucher/v1` | 签名 | 透明额度凭证（按通道累计） | 自 M5 起使用 |
| `agentcoin/receipt/v1` | 签名 | 推理回执 | 为 M5 预留 |

## Poseidon2

`poseidon2::hash`（功能 `poseidon2`）是 256 位的 Poseidon2 哈希，供合约（位于 `0x…0a030000` 的 `poseidon2` 预编译）以及将来的 STARK 工作使用。它包装 Plonky3（`p3-goldilocks` 0.8，`MIT OR Apache-2.0`）：置换和海绵结构都来自 Plonky3，不自行实现任何部分。

- **实例**：Plonky3 默认的 Goldilocks 实例（p = 2^64 − 2^32 + 1），宽度 12，S-box x^7，外部轮 8、内部轮 22，采用 Plonky3 固定的轮常数和内部对角矩阵。它不是 Poseidon2 作者参考实现的实例，因此用 Plonky3 公布的已知答案向量校验，而不是作者向量（用户决定，m4-evm）。
- **海绵**：速率 8、容量 4，初始状态全零；每 8 个元素一组覆盖速率部分后置换；输出前 4 个元素，每个按小端 8 字节拼接（32 字节，128 位安全）。
- **编码（单射）**：先是输入的字节长度（最多 2^32 − 1，否则返回 `Error::InputTooLong`），随后是输入加一个 `0x01` 字节、再补零到 7 字节的倍数，按小端每 7 字节一个元素，最后补零元素到 8 的倍数。
- **上下文**：该函数是不带域分离上下文的原始哈希，这符合合约的使用习惯；协议内部使用 Poseidon2 时仍须加上已登记的上下文前缀（见上表）。

示例代码见英文版 [README.md](README.md)（作为 doctest 运行）：空输入的哈希与回归向量一致，末尾多一个零字节会改变哈希。

## 加密私钥文件（格式 v1）

JSON 文档，字段包括：`version`（1）、`kind`（`signing-seed` 或 `wallet-entropy`）、`alg` 与 `public_key`（规范编码的十六进制，仅签名种子有）、`kdf`（`argon2id`，含 `m_kib`、`t`、`p` 和 16 字节 `salt`）、`cipher`（`xchacha20poly1305`，含 24 字节 `nonce` 和 `ciphertext`）。附加认证数据为对全部元数据字段计算的 `derive_key("agentcoin 2026-09 keystore-aad v1", …)`，因此篡改任一字段都会导致解密失败。KDF 参数低于 64 MiB / 3 轮 / 1 路并行的文件会被拒绝。

## 新增一个算法

1. 通过 OpenSpec 提出（AGENT.md §3），并引用它服务的决策。
2. 在 `src/alg.rs` 中分配新的 AlgId（或把预留编号改为已实现），在 `src/tagged.rs` 中以 `codec(index = AlgId)` 增加变体，并在 `tests/alg_table.rs` 与 `tests/scale_codec.rs` 的固化表中追加——不得修改已有行。
3. 在独立模块中实现后端，只有该模块依赖新的库；在 `sig/mod.rs` 或 `kem/mod.rs` 中按 AlgId 分派。
4. 通过 `scripts/fetch-test-vectors.sh` 加入官方测试向量，并记录在 `tests/vectors/SOURCES.md`。
5. 更新本 README（中英文两个版本）；如需新的上下文，同时更新上下文登记表。

## 测试向量

`tests/vectors/` 存放筛选后的 NIST ACVP（ML-DSA、ML-KEM-768）、X-Wing、Argon2id（RFC 9106）、XChaCha20-Poly1305（draft-irtf-cfrg-xchacha-03）、BIP-39 向量和 Plonky3 的 Poseidon2 置换向量（可通过 `scripts/fetch-test-vectors.sh` 复现），以及本仓库生成的回归向量（含 Poseidon2 哈希），详见 `tests/vectors/SOURCES.md`。
