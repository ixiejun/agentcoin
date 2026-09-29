> 🌐 **English** | [简体中文](README.zh-CN.md)

# ac-toploc

TOPLOC proofs for verifiable inference (MVP plan §2, §6; spec `market/toploc`). A provider
keeps, for each chunk of hidden-layer activations, the top-k values by magnitude as a
polynomial over the prime field of 65,497 elements; an auditor who recomputes the activations
compares them with the polynomial and sees whether the declared model and precision were used.
Receipts carry a 32-byte [`commitment`] to the proofs; the proofs themselves stay with the
gateway until the challenge period ends.

This crate is a pure-integer, `no_std` port of the reference implementation by Prime Intellect
(<https://github.com/PrimeIntellect-ai/toploc>, commit `7ab7bcd6a4459ba4400fd41e4636bbeed7438997`,
MIT licence; the notice is kept in `NOTICE`). Proofs are byte-compatible with the reference, and
the test vectors are the reference's own outputs (`scripts/gen-toploc-vectors.sh`).

TOPLOC is a locality-sensitive hash, not a cryptographic primitive: it detects a swapped model
or precision, it does not hide or authenticate anything. Receipts are authenticated by their
ML-DSA signatures.

## Rules

- **Input**: activations as bfloat16 bit patterns ([`Bf16`]); other precisions are refused, as
  the reference only works with bfloat16. Infinite and NaN values are refused (they would fall
  outside the field). Without `skip_prefill` the first activation is the prefill and forms its
  own chunk; the others are concatenated `decode_batching_size` at a time.
- **Proof of a chunk**: the `topk` largest magnitudes; the largest modulus `m ≤ 65,497` that
  keeps their indices distinct; the coefficients of the polynomial through
  `(index mod m, bf16 bits)`, by Newton interpolation mod 65,497.
- **Ties**: equal magnitudes are ordered by lower index first. The reference leaves ties to
  `torch.topk`; only a tie between the k-th and (k+1)-th value changes the proof, and the vectors
  contain none.
- **Encoding**: big-endian `u16` modulus, then each coefficient as a big-endian `u16`.
- **Comparison** ([`compare`]): per chunk, the number of top-k positions whose exponent differs
  from the proof's, and over the others the sum and count of mantissa differences, the sorted
  errors' element `⌊n/2⌋` (`median_upper`, the reference's batched check) and twice
  `statistics.median` (`median_twice`, the reference's list check), all integers. The proof is
  evaluated at `index mod m`, as it was built; the reference's list check evaluates at the raw
  index, which agrees only when `m = 65,497`.
- **Thresholds** for pass or fail are not decided here: the audit (M6) calibrates them per
  hardware and engine.
- **Commitment**: `derive("agentcoin 2026-09 toploc-commit v1", SCALE(decode_batching_size u32,
  topk u32, skip_prefill bool, [proof encodings]))`.

## Features

| Feature | Default | Purpose |
|---|---|---|
| `std` | yes | Standard library and `std::error::Error` for [`Error`]. |

## Example

```rust
use ac_toploc::{Bf16, Params, build_proofs, commitment, compare};

// A prefill of 8 values and two generated tokens of 4 values each (bfloat16 bit patterns).
let prefill: Vec<Bf16> = (0..8u16).map(|i| Bf16(0x3f80 + 5 * i)).collect();
let token: Vec<Bf16> = (0..4u16).map(|i| Bf16(0xbf80 + 3 * i)).collect();
let activations: Vec<&[Bf16]> = vec![&prefill, &token, &token];
let params = Params { decode_batching_size: 2, topk: 4, skip_prefill: false };

let proofs = build_proofs(&activations, &params)?;
assert_eq!(proofs.len(), 2); // the prefill chunk and one decode chunk
assert_eq!(proofs[0].to_bytes().len(), 2 + 2 * 4);

// Recomputing the same activations matches exactly.
for c in compare(&activations, &proofs, &params)? {
    assert_eq!((c.exp_mismatches, c.mant_err_sum), (0, 0));
}

// A different model: every value one exponent step larger.
let other: Vec<Bf16> = prefill.iter().map(|v| Bf16(v.0 + 0x80)).collect();
let changed: Vec<&[Bf16]> = vec![&other, &token, &token];
assert_eq!(compare(&changed, &proofs, &params)?[0].exp_mismatches, 4);

// What a receipt carries.
let commit: [u8; 32] = commitment(&params, &proofs).expect("published context");
# let _ = commit;
# Ok::<(), ac_toploc::Error>(())
```
