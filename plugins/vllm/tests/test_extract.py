"""Candidate extraction on CPU tensors (runs where PyTorch is installed: the vllm-plugin CI job)."""

import torch

from agentcoin_vllm import client, extract


def bf16_bits(t: torch.Tensor) -> list[int]:
    return [b & 0xFFFF for b in t.contiguous().view(torch.int16).tolist()]


def reference_topk(values: torch.Tensor, k: int) -> set[tuple[int, int]]:
    flat = values.reshape(-1)
    idx = flat.abs().topk(min(k, flat.numel())).indices
    return set(zip(idx.tolist(), bf16_bits(flat[idx])))


def test_spans_split_a_mixed_batch():
    # r1 decodes (prompt of 5 already computed plus 2 tokens), r2 prefills its first 4
    # prompt tokens of 10, r3 is not scheduled, r4 finishes its prompt (8 of 12 done).
    s = extract.spans(["r1", "r2", "r3", "r4"], [1, 4, 0, 4], [7, 0, 3, 8], [5, 10, 3, 12])
    assert s == [
        extract.Span("r1", client.DECODE, 0, 1),
        extract.Span("r2", client.PREFILL, 1, 4),
        extract.Span("r4", client.PREFILL, 5, 4),
    ]


# Scenario "候选与激活值一致" on a synthetic step.
def test_candidates_are_each_spans_top_k():
    torch.manual_seed(7)
    hidden = torch.randn(9, 16, dtype=torch.bfloat16)
    step = extract.spans(["a", "b", "c"], [1, 5, 3], [4, 0, 2], [4, 20, 2])
    meta, idx, bits = extract.candidates(hidden, step, k=6)
    got = list(extract.frames(meta, idx, bits))
    assert [(r, p, n) for r, p, n, _ in got] == [
        ("a", client.DECODE, 16),
        ("b", client.PREFILL, 80),
        ("c", client.DECODE, 48),
    ]
    rows = {"a": hidden[0:1], "b": hidden[1:6], "c": hidden[6:9]}
    for request, _, _, pairs in got:
        assert len(pairs) == 6
        assert set(pairs) == reference_topk(rows[request], 6)


def test_a_segment_smaller_than_k_sends_everything():
    hidden = torch.randn(1, 4, dtype=torch.bfloat16)
    step = extract.spans(["a"], [1], [3], [3])
    ((_, _, n, pairs),) = extract.frames(*extract.candidates(hidden, step, k=128))
    assert n == 4 and len(pairs) == 4
    assert sorted(i for i, _ in pairs) == [0, 1, 2, 3]


def test_other_dtypes_are_refused():
    hidden = torch.randn(2, 4, dtype=torch.float16)
    try:
        extract.candidates(hidden, extract.spans(["a"], [2], [0], [2]), k=2)
    except TypeError as e:
        assert "bfloat16" in str(e)
    else:
        raise AssertionError("float16 accepted")


def test_an_empty_step():
    meta, idx, bits = extract.candidates(torch.zeros(0, 4, dtype=torch.bfloat16), [], k=4)
    assert meta == [] and idx.numel() == 0 and bits.numel() == 0
