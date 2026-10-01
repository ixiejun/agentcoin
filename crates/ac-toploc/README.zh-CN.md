> 🌐 [English](README.md) | **简体中文**

# ac-toploc

可验证推理的 TOPLOC 证明（MVP 技术方案 §2、§6；规格 `market/toploc`）。提供者对隐藏层激活值的每一块，把按绝对值最大的 top-k 个值保存为 65,497 元素素数域上的一个多项式；审计员复算激活值后与多项式比对，就能看出是否使用了声明的模型和精度。收据只携带证明的 32 字节[`commitment`]（承诺），证明本身由网关保存到挑战期结束。

本 crate 是 Prime Intellect 参考实现（<https://github.com/PrimeIntellect-ai/toploc>，提交 `7ab7bcd6a4459ba4400fd41e4636bbeed7438997`，MIT 许可证，版权声明保留在 `NOTICE` 中）的纯整数、`no_std` 移植。证明与参考实现逐字节兼容，测试向量就是参考实现自己的输出（`scripts/gen-toploc-vectors.sh`）。

TOPLOC 是局部敏感哈希，不是密码学原语：它用来发现被替换的模型或精度，不承担任何保密或认证职能。收据由其 ML-DSA 签名认证。

## 规则

- **输入**：激活值为 bfloat16 位模式（[`Bf16`]），参考实现只支持 bfloat16，其他精度一律拒绝。无穷大和 NaN 也拒绝（它们会落在域外）。不跳过预填充时，第一个激活是预填充，单独成一块；其余激活每 `decode_batching_size` 个拼成一块。
- **一块的证明**：绝对值最大的 `topk` 个值；使这些下标两两不同余的最大模数 `m ≤ 65,497`；过点 `(下标 mod m, bf16 位模式)` 的多项式系数，由模 65,497 的牛顿插值求得。
- **并列**：绝对值相同的按下标小者优先。参考实现把并列交给 `torch.topk`；只有第 k 与第 k+1 名并列时证明才会不同，向量中没有这种情况。
- **编码**：大端 `u16` 模数，后接每个系数的大端 `u16`。
- **比对**（[`compare`]）：每块给出指数与证明不同的 top-k 位置数；其余位置的尾数差之和与项数；排序后第 `⌊n/2⌋` 项（`median_upper`，参考实现批量校验的中位数）以及 `statistics.median` 的两倍（`median_twice`，参考实现逐列表校验的中位数），全部为整数。证明按构造时的方式在 `下标 mod m` 处求值；参考实现的逐列表校验在原始下标处求值，只在 `m = 65,497` 时一致。
- **阈值**：通过与否的阈值不在这里决定，由审计（M6）按硬件和引擎校准。
- **承诺**：`derive("agentcoin 2026-09 toploc-commit v1", SCALE(decode_batching_size u32, topk u32, skip_prefill bool, [各块证明编码]))`。

## 由引擎候选构造证明

推理引擎插件不发送完整的激活值。对每个前向步骤，它为每个请求发送一个*段*：阶段（预填充或解码）、元素个数，以及按绝对值最大的 `min(k, len)` 个值和它们的下标（[`top_k_candidates`]）。[`build_proofs_from_candidates`] 把预填充的各段拼成预填充激活，每个解码段作为一个激活，按 [`build_proofs`] 的规则分块（段内下标加上块内在它之前各段的元素数），每块从候选中取 top-k。在同一顺序（幅值降序、下标升序）下，块的 top-k 一定落在各段 top-k 的并集里，所以只要插件按这一顺序选取每段的候选（vLLM 插件正是如此），结果就与由全量激活值调用 [`build_proofs`] 逐字节相同，并列时也一样。参考实现把并列交给 `torch.topk`，因此只在第 k 名处没有并列的块上与它逐字节一致；真实的 bfloat16 激活值经常出现这种并列。候选数量不是 `min(k, len)`、下标越出段或重复、值不是有限数、或预填充段出现在解码段之后时，构造返回错误。

`examples/toploc_from_candidates.rs` 以 JSON 读入这样的段，输出证明与承诺（CI 用它把 vLLM 插件与参考实现比对）。

```rust
use ac_toploc::{Bf16, Params, Phase, Segment, build_proofs, build_proofs_from_candidates, top_k_candidates};

let prefill: Vec<Bf16> = (0..12u16).map(|i| Bf16(0x3f80 + 7 * i)).collect();
let token: Vec<Bf16> = (0..4u16).map(|i| Bf16(0xbf80 + 3 * i)).collect();
let params = Params { decode_batching_size: 2, topk: 4, skip_prefill: false };

// 预填充分两步计算，之后是两个解码步骤。
let (a, b) = prefill.split_at(8);
let segment = |phase, v: &[Bf16]| Segment {
    phase,
    len: v.len() as u32,
    candidates: top_k_candidates(v, 4),
};
let segments = [
    segment(Phase::Prefill, a),
    segment(Phase::Prefill, b),
    segment(Phase::Decode, &token),
    segment(Phase::Decode, &token),
];
let whole: Vec<&[Bf16]> = vec![&prefill, &token, &token];
assert_eq!(build_proofs_from_candidates(&segments, &params)?, build_proofs(&whole, &params)?);
```

## 由候选比对

复核方同样不需要完整的激活值。[`compare_from_candidates`] 以候选段的形式接收复算的激活值，按上面的规则分块，再把每块的 top-k 与证明比对；只要每段候选都是该段真实的 top-k，结果就与对完整激活值调用 [`compare`] 相同。审计员的引擎对 prompt 与输出做一次预填充，每个 token 行发送一段；prompt 的各行作为预填充段，其后每行作为一个解码步骤，这样就重建了原推理的分块。

```rust
use ac_toploc::{Bf16, Params, Phase, Segment, build_proofs, compare, compare_from_candidates, top_k_candidates};

let hidden = 4;
let prompt: Vec<Bf16> = (0..12u16).map(|i| Bf16(0x3f80 + 7 * i)).collect(); // 3 行
let token: Vec<Bf16> = (0..4u16).map(|i| Bf16(0xbf80 + 3 * i)).collect();
let params = Params { decode_batching_size: 2, topk: 4, skip_prefill: false };
let proofs = build_proofs(&[&prompt, &token, &token], &params)?;

// 复核方的行：三个 prompt 行，然后是经过前向计算的两个输出 token。
let row = |phase, v: &[Bf16]| Segment { phase, len: v.len() as u32, candidates: top_k_candidates(v, 4) };
let mut rows: Vec<Segment> = prompt.chunks(hidden).map(|r| row(Phase::Prefill, r)).collect();
rows.push(row(Phase::Decode, &token));
rows.push(row(Phase::Decode, &token));
assert_eq!(
    compare_from_candidates(&rows, &proofs, &params)?,
    compare(&[&prompt, &token, &token], &proofs, &params)?,
);
# Ok::<(), ac_toploc::Error>(())
```

## 功能开关

| 开关 | 默认 | 用途 |
|---|---|---|
| `std` | 是 | 标准库，以及 [`Error`] 的 `std::error::Error` 实现。 |

## 示例

```rust
use ac_toploc::{Bf16, Params, build_proofs, commitment, compare};

// 8 个值的预填充，以及两个各 4 个值的生成 token（bfloat16 位模式）。
let prefill: Vec<Bf16> = (0..8u16).map(|i| Bf16(0x3f80 + 5 * i)).collect();
let token: Vec<Bf16> = (0..4u16).map(|i| Bf16(0xbf80 + 3 * i)).collect();
let activations: Vec<&[Bf16]> = vec![&prefill, &token, &token];
let params = Params { decode_batching_size: 2, topk: 4, skip_prefill: false };

let proofs = build_proofs(&activations, &params)?;
assert_eq!(proofs.len(), 2); // 预填充一块、解码一块
assert_eq!(proofs[0].to_bytes().len(), 2 + 2 * 4);

// 复算得到相同的激活值：完全一致。
for c in compare(&activations, &proofs, &params)? {
    assert_eq!((c.exp_mismatches, c.mant_err_sum), (0, 0));
}

// 换了模型：每个值都大一个指数级。
let other: Vec<Bf16> = prefill.iter().map(|v| Bf16(v.0 + 0x80)).collect();
let changed: Vec<&[Bf16]> = vec![&other, &token, &token];
assert_eq!(compare(&changed, &proofs, &params)?[0].exp_mismatches, 4);

// 收据中携带的承诺。
let commit: [u8; 32] = commitment(&params, &proofs).expect("published context");
# let _ = commit;
# Ok::<(), ac_toploc::Error>(())
```
