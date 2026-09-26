> 🌐 [English](README.md) | **简体中文**

# pallet-aura-pq

AgentCoin 抗量子出块机制 Aura-PQ 的 runtime 部分（方案 §3.5、§4.1）。

- **授权节点集合**：按时隙分配顺序排列的 ML-DSA-65 公钥；时隙 `s` 属于第 `s mod N` 个授权节点。M1 中该集合来自创世配置（PoA）。创世构建拒绝重复公钥、非 ML-DSA-65 公钥以及超过 `MaxAuthorities` 的集合；空的默认创世不写入任何授权节点，节点会拒绝运行这样的链。
- **时隙记录**：`on_initialize` 记录区块 Aura-PQ 预运行摘要中声明的时隙。区块有效性（时隙顺序、出块人、封印）由节点的导入校验器强制执行；runtime 在区块执行期间从不 panic，只记录不一致。
- **`AuthoritySetWriter`**：为 M3 验证人集合状态机（PoA → PoS）预留的写入入口。
- `AuraPqApi` runtime API、摘要辅助函数和封印上下文 `agentcoin/aura-seal/v1` 位于 `ac_primitives::aura_pq`，与节点共用。

## Feature

| Feature | 默认 | 用途 |
|---|---|---|
| `std` | 是 | 原生构建与测试。WASM runtime 构建时关闭。 |
| `try-runtime` | 否 | SDK 的 try-runtime 支持。 |

## 示例

示例代码见英文版 [README.md](README.md)（英文版中的示例作为 doctest 运行）：生成 3 个 ML-DSA-65 授权公钥，校验集合合法，并按 `s mod N` 规则确定时隙 4 的出块人。
