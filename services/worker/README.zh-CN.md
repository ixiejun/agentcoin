> 🌐 [English](README.md) | **简体中文**

# ac-worker

AgentCoin 的公共任务工作者与发布方工具（MVP 方案 §5.6；OpenSpec 变更 `m6-public-jobs`；规格 `market/public-worker`）。工作者用本机推理引擎执行链（`pallet-public-jobs`）分配给它的单元：按清单哈希核对数据，按该类型的固定规则执行，提交摘要的承诺，在承诺期限之后揭示，把完整结果上传给发布方并领取报酬。日志只记录任务、单元、计数与耗时，从不记录数据与结果。

## 命令

| 命令 | 使用者 | 作用 |
|---|---|---|
| `ac-worker run --config worker.json` | 工作者 | 工作者服务 |
| `ac-worker collect --config collect.json` | 发布方 | 结果收集服务；公开金丝雀 |
| `ac-worker canary --job N --kind K --manifest m.json --units 0,7,… --out c.json [--engine URL --model NAME]` | 发布方 | 生成任务的金丝雀并打印根 |
| `ac-worker exec --kind K --shard s.jsonl [--engine URL --model NAME --job N --out r]` | 任何人 | 执行一个分片，打印摘要与结果哈希 |
| `ac-worker compare --kind K --a HEX --b HEX` | 任何人 | 按比对规则 v1 比较两份摘要 |

### 工作者服务

```json
{
  "node": "http://127.0.0.1:9944",
  "wallet": "worker.json", "password_file": "worker.pass",
  "data_dir": "worker-data",
  "engines": [{"model": "0x…", "engine": "http://127.0.0.1:8000", "engine_model": "qwen"}],
  "poll_ms": 1000
}
```

账户须先登记（`ac-wallet public worker register --model 0x…`）；服务启动时把链上登记的模型更新为配置中的模型。每个周期它每轮声明一次就绪，在后台执行新分配的单元（每个引擎同一时间只执行一个），在期限前提交承诺、期限后揭示，为每个自己处于多数方的通过单元上传结果（失败则重试，直到链清理该单元），领取已结算的纪元并取出到期的报酬。交易发出后不等待上链（nonce 计入仍在交易池中的交易），效果从链上读回。数据与哈希不符、或引擎出错的单元不提交承诺：宁可记一次失误，也不提交未经计算的摘要。已执行的单元（摘要、结果、盐）保存在 `data_dir/units`，直到链清理该单元，因此重启后的工作者仍能揭示与上传。

### 结果收集服务

```json
{"node": "http://127.0.0.1:9944", "listen": "0.0.0.0:8600", "dir": "results",
 "wallet": "publisher.json", "password_file": "publisher.pass",
 "canaries": ["job-3-canaries.json"]}
```

工作者以 `PUT /<job>/<unit>` 上传，请求头 `X-AgentCoin-Worker` 为其地址。只有当单元已通过、上传者属于多数方、请求体的 BLAKE3 等于其揭示的结果哈希、且尚未保存时，结果才保存到 `dir/<job>/<unit>`；否则回答 403 或 409，不作说明。配置了金丝雀文件（及用于签名的钱包）时，收集服务在每个金丝雀单元通过后公开其金丝雀。

## 执行规则（版本 1）

- **评测。**每道题有一个上下文与 2–16 个续写。续写的得分是其各 token 在上下文之后的对数概率之和（`/v1/completions`，prompt 为 token ID，`max_tokens: 1`，`prompt_logprobs: 0`；不采样），以千分之一奈特为单位四舍五入。答案取得分最高者，相等时取序号较小者；上下文的分词必须是“上下文 + 续写”分词的前缀，否则该单元不提交。摘要是各题答案；完整结果包含每个得分。
- **嵌入。**每段文本的向量（`/v1/embeddings`）做 L2 归一化；其 32 位指纹的第 j 位表示它与第 j 个方向的点积是否为正。第 j 个方向的第 d 维取 +1 或 −1，取决于 `derive("agentcoin 2026-10 public-direction v1", job ‖ j ‖ block)` 的第 d / 256 个 32 字节块中第 d mod 256 位（每字节先取最低位）是否置位。摘要是各段指纹；完整结果是归一化后的向量。
- **数据清洗**（无需引擎；逐字节可复现）：Unicode NFC（`unicode-normalization` 0.1.25，Unicode 17.0），删除除换行与制表符外的控制字符，行内连续空白合并、行首尾去空白，删除少于 32 个字符的文档，删除精确重复，删除近似重复（小写字符 5-gram 的 MinHash，128 个排列、固定种子，16 带 × 8 行，103 个最小值相等即确认，即估计 Jaccard 相似度 0.8）；重复者保留先出现的。完整结果是保留文档的 JSON Lines `{"index", "text"}`，摘要是其 BLAKE3。上述任何一点或 Unicode 表的变化都是新的规则版本：`tests/vectors/public_clean_v1.json` 固定了结果字节。

## 发布任务

1. **数据。**把工作切分为单元，为每个分片（JSON Lines：评测 `{"context": …, "choices": […]}`，文本 `{"text": …}`）与清单 `{"units": [{"url": …, "blake3": 十六进制, "items": n}, …]}`（每单元一项，按单元顺序）提供下载地址。
2. **金丝雀。**随机挑选金丝雀单元并对选择保密：`ac-worker canary` 用你自己的引擎计算其预期摘要（任务编号用任务将得到的编号，`ac-wallet public jobs` 显示最新的任务）并打印根。串通的多数只会在金丝雀单元上被抓到，比例决定抓到的速度：金丝雀比例为 `c` 时，伪造 `n` 个单元的多数逃脱的概率约为 `(1 − c)^n`；默认取 5%–10% 较为合理。每个单元通过前，金丝雀文件须保密。
3. **收集服务。**在结果地址上运行 `ac-worker collect`，并带上金丝雀文件。
4. **发布。**`ac-wallet public publish --kind … --manifest m.json --manifest-url … --results-url … --units N --price 0.05 [--model 0x…] [--canary-root 0x…]` 核对清单与价格并打印调用；由管理权限以动议提出。

## 示例

数据清洗可复现，其摘要就是结果的哈希：

```rust
use ac_worker::exec::clean_unit;

let docs = vec![
    "A document long enough to be kept by the cleaning rules.".to_string(),
    "  A document long enough to be kept by the cleaning rules.  ".to_string(),
    "too short".to_string(),
];
let out = clean_unit(&docs).unwrap();
assert_eq!(out.summary, out.result_hash.to_vec());
assert_eq!(out, clean_unit(&docs).unwrap());
assert_eq!(String::from_utf8(out.result).unwrap().lines().count(), 1);
```

## 功能开关

无。
