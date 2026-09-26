> 🌐 [English](README.md) | **简体中文**

# ac-consensus-aura-pq

AgentCoin 抗量子出块机制 Aura-PQ 的节点部分（方案 §4.1）。

SDK 自带的 `sc-consensus-aura` 通过 keystore 签名，而 keystore 只支持经典密钥类型（sr25519、Ed25519、ECDSA、BLS）。Aura-PQ 复用 `sc-consensus-slots` 的通用时隙机制，把 ML-DSA-65 授权密钥直接保存在内存中。

- **出块**（`start_aura_pq`）：时隙 `s` 属于第 `s mod N` 个授权节点（时隙 1 秒）。出块人加入声明时隙的预运行摘要（引擎 ID `acpq`），并用 hedged ML-DSA-65 签名（上下文 `agentcoin/aura-seal/v1`）对去掉封印后的区块头哈希封印。
- **导入**（`import_queue`）：拒绝以下区块：封印缺失或无效、时隙摘要缺失或重复、时隙比本地时钟超前一个以上、时隙不大于父区块、出块人不是该时隙的合法授权节点。交易内部的固有数据照常校验。
- **双签**：同一出块人在同一时隙签出的两个区块头会连同两个 SCALE 编码的区块头一起记入日志（`equivocation::check`）。罚没随 M2 的 AC-BFT 引入。
- M1 没有终局性组件，分叉选择采用最长链。

## 示例

示例代码见英文版 [README.md](README.md)（英文版中的示例作为 doctest 运行）：用 ML-DSA-65 密钥为时隙 10 的区块头封印，再用授权节点列表校验该区块头。
