#!/usr/bin/env python3
"""Generate TOPLOC test vectors with the reference implementation (spec market/toploc).

Run by scripts/gen-toploc-vectors.sh inside a virtual environment that has PyTorch and the
reference `toploc` package at the pinned commit. Usage:

    gen-toploc-vectors.py <out.json> <source-json>

Activations are not drawn with torch: they come from splitmix64 so that the Rust tests can
regenerate the same inputs from the seed recorded in the vector file (only the reference's
outputs depend on PyTorch). A seed whose chunks have a tie between the k-th and (k+1)-th
magnitude is skipped (the reference leaves such ties to torch.topk); the seed actually used is
recorded.
"""

import json
import sys

import torch
from toploc import build_proofs_bytes, verify_proofs_bytes

MASK64 = (1 << 64) - 1


class SplitMix64:
    """splitmix64; the Rust tests implement the same generator."""

    def __init__(self, seed):
        self.state = seed & MASK64

    def next(self):
        self.state = (self.state + 0x9E3779B97F4A7C15) & MASK64
        z = self.state
        z = ((z ^ (z >> 30)) * 0xBF58476D1CE4E5B9) & MASK64
        z = ((z ^ (z >> 27)) * 0x94D049BB133111EB) & MASK64
        return z ^ (z >> 31)


def activation_bits(rng, n):
    """n finite bf16 patterns: random sign, exponent 0x70..0x85, random mantissa."""
    out = []
    for _ in range(n):
        r = rng.next()
        sign = r & 1
        exp = 0x70 + ((r >> 1) % 22)
        mant = (r >> 8) & 0x7F
        out.append((sign << 15) | (exp << 7) | mant)
    return out


def perturb(bits, rng):
    """Per value: 1/16 exponent + 1, 8/16 mantissa moved by 1-3 (clamped), else unchanged."""
    out = []
    for b in bits:
        r = rng.next()
        kind = r % 16
        exp = (b >> 7) & 0xFF
        mant = b & 0x7F
        if kind == 0:
            exp += 1
        elif kind <= 8:
            delta = 1 + ((r >> 8) % 3)
            mant = mant + delta if (r >> 16) & 1 else mant - delta
            mant = min(max(mant, 0), 0x7F)
        out.append((b & 0x8000) | (exp << 7) | mant)
    return out


def make(seed, lengths, overrides):
    rng = SplitMix64(seed)
    acts = [activation_bits(rng, n) for n in lengths]
    for a, i, bits in overrides:
        acts[a][i] = bits
    return acts


def chunks(acts, batch, skip_prefill):
    """The reference's chunking, on bit lists."""
    out = []
    start = 0
    if not skip_prefill:
        out.append(acts[0])
        start = 1
    for i in range(start, len(acts), batch):
        out.append([v for a in acts[i : i + batch] for v in a])
    return out


def boundary_tie(acts, batch, topk, skip_prefill):
    for chunk in chunks(acts, batch, skip_prefill):
        mags = sorted((v & 0x7FFF for v in chunk), reverse=True)
        if len(mags) > topk and mags[topk - 1] == mags[topk]:
            return True
    return False


def tensor(bits):
    signed = [b - 0x10000 if b >= 0x8000 else b for b in bits]
    return torch.tensor(signed, dtype=torch.int16).view(torch.bfloat16)


def results(res):
    return [
        {
            "exp_mismatches": int(r.exp_mismatches),
            "mant_err_mean": float(r.mant_err_mean),
            "mant_err_median": float(r.mant_err_median),
        }
        for r in res
    ]


def case(name, seed, lengths, batch, topk, skip_prefill, overrides=(), batched=False):
    while True:
        acts = make(seed, lengths, overrides)
        perturbed = perturb([v for a in acts for v in a], SplitMix64(seed ^ 0xA5A5A5A5))
        # Split the flat perturbed list back into activations of the same lengths.
        pert, pos = [], 0
        for n in lengths:
            pert.append(perturbed[pos : pos + n])
            pos += n
        if not (
            boundary_tie(acts, batch, topk, skip_prefill)
            or boundary_tie(pert, batch, topk, skip_prefill)
        ):
            break
        seed += 1
    shapes = [tensor(a) for a in acts]
    if not skip_prefill:
        # The reference takes the prefill as a 2-D tensor; its flattening is the same list.
        shapes[0] = shapes[0].view(1, -1)
    proofs = build_proofs_bytes(shapes, batch, topk, skip_prefill=skip_prefill)
    out = {
        "name": name,
        "seed": seed,
        "lengths": lengths,
        "overrides": [list(o) for o in overrides],
        "decode_batching_size": batch,
        "topk": topk,
        "skip_prefill": skip_prefill,
        "proofs": [p.hex() for p in proofs],
        "same": results(verify_proofs_bytes(shapes, proofs, batch, topk, skip_prefill=skip_prefill)),
        "perturbed": results(
            verify_proofs_bytes([tensor(a) for a in pert], proofs, batch, topk, skip_prefill=skip_prefill)
        ),
    }
    if batched:
        # The reference's batched C++ check (a 2-D tensor with skip_prefill): its median is the
        # sorted errors' element n/2, and it reduces indices by the proof's modulus as the proof
        # construction does (the list check above evaluates at unreduced indices, which agrees
        # with the proofs only when the modulus is 65,497).
        assert skip_prefill and len(set(lengths)) == 1
        for key, rows in (("same_batched", acts), ("perturbed_batched", pert)):
            stacked = torch.stack([tensor(a) for a in rows])
            out[key] = results(verify_proofs_bytes(stacked, proofs, batch, topk, skip_prefill=True))
    return out


def main():
    out_path, source_json = sys.argv[1], sys.argv[2]
    source = json.loads(source_json)
    source["torch"] = torch.__version__.split("+")[0]
    cases = [
        case("prefill-and-decode-batches", 1, [6 * 32] + [32] * 11, 4, 8, False),
        case("skip-prefill", 2, [24] * 9, 3, 6, True, batched=True),
        case("short-last-batch", 3, [5 * 16] + [16] * 7, 3, 4, False),
        case("single-decode-chunks", 4, [48] + [48] * 5, 1, 16, False),
        case("even-topk-medians", 5, [40] * 8, 2, 10, True, batched=True),
        # 70,000 values with the two largest at indices 10 and 65,507, which coincide mod 65,497:
        # the modulus search has to go below 65,497.
        case(
            "modulus-below-prime",
            6,
            [70_000],
            1,
            16,
            True,
            overrides=[(0, 10, 0x4480), (0, 65_507, 0x4500)],
            batched=True,
        ),
    ]
    doc = {
        "source": source,
        "generator": (
            "splitmix64(seed) per activation value: sign = r & 1, exponent = 0x70 + (r >> 1) % 22, "
            "mantissa = (r >> 8) & 0x7f; overrides [activation, index, bits] applied after; "
            "perturbation from splitmix64(seed ^ 0xa5a5a5a5) over the flattened activations: "
            "r % 16 == 0 exponent + 1, 1..=8 mantissa +/- (1 + (r >> 8) % 3) (+ if (r >> 16) & 1) "
            "clamped to 0..=127, else unchanged"
        ),
        "cases": cases,
    }
    with open(out_path, "w", encoding="utf-8") as f:
        json.dump(doc, f, indent=1)
        f.write("\n")


if __name__ == "__main__":
    main()
