"""Candidate extraction on CPU tensors (runs where PyTorch is installed: the vllm-plugin CI job)."""

import torch

from agentcoin_vllm import client, extract


def bf16_bits(t: torch.Tensor) -> list[int]:
    return [b & 0xFFFF for b in t.contiguous().view(torch.int16).tolist()]


def reference_topk(values: torch.Tensor, k: int) -> set[tuple[int, int]]:
    """The first k of (larger magnitude, lower index), as ac-toploc orders them."""
    flat = values.reshape(-1)
    order = sorted(range(flat.numel()), key=lambda i: (-abs(float(flat[i])), i))[:k]
    return set(zip(order, bf16_bits(flat[order])))


def test_spans_split_a_mixed_batch():
    # r1 decodes (prompt of 5 already computed plus 2 tokens), r2 prefills its first 4
    # prompt tokens of 10, r3 is not scheduled, r4 finishes its prompt (8 of 12 done).
    s = extract.spans(["r1", "r2", "r3", "r4"], [1, 4, 0, 4], [7, 0, 3, 8], [5, 10, 3, 12])
    assert s == [
        extract.Span("r1", client.DECODE, 0, 1),
        extract.Span("r2", client.PREFILL, 1, 4),
        extract.Span("r4", client.PREFILL, 5, 4),
    ]


# Spec "抢占后重算": a request with a prompt of 20 that generated 5 tokens is preempted and
# recomputed in one step: one prefill span for the prompt, one decode span per generated row.
def test_a_recomputed_request_splits_at_the_prompt():
    s = extract.spans(["a", "b"], [1, 25], [9, 0], [8, 20])
    assert s == [
        extract.Span("a", client.DECODE, 0, 1),
        extract.Span("b", client.PREFILL, 1, 20),
    ] + [extract.Span("b", client.DECODE, 21 + i, 1) for i in range(5)]


# A recomputation split over steps: the prompt's rest, then generated rows each on their own,
# and a later chunk that holds only generated rows.
def test_a_chunked_recomputation_splits_by_row():
    first = extract.spans(["b"], [12], [12], [20])
    assert first == [extract.Span("b", client.PREFILL, 0, 8)] + [
        extract.Span("b", client.DECODE, 8 + i, 1) for i in range(4)
    ]
    later = extract.spans(["b"], [3], [24], [20])
    assert later == [extract.Span("b", client.DECODE, i, 1) for i in range(3)]


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


# m5-engine-toploc 3.5, scenario "第 k 名处的并列": ties at the k-th place go to the lower index.
def test_ties_at_the_kth_place_take_the_lower_index():
    # Magnitudes 3, 2 (x4, one negative), 1: the top 3 are index 1 (3.0) and the two lowest
    # indices of the 2.0s.
    flat = torch.tensor([2.0, 3.0, -2.0, 1.0, 2.0, 2.0], dtype=torch.bfloat16)
    assert extract.top_k_indices(flat, 3).tolist() == [1, 0, 2]
    # Many ties: a coarse grid of values in random order.
    torch.manual_seed(3)
    flat = (torch.randint(-6, 7, (4096,)).to(torch.bfloat16) / 2).contiguous()
    got = extract.top_k_indices(flat, 128)
    assert set(zip(got.tolist(), bf16_bits(flat[got]))) == reference_topk(flat, 128)
    step = extract.spans(["a"], [16], [0], [16])
    ((_, _, _, pairs),) = extract.frames(*extract.candidates(flat.reshape(16, 256), step, k=128))
    assert set(pairs) == reference_topk(flat, 128)


# Scenarios "每个 token 一段" and "分步预填充" (verify mode): one segment per row of the
# prefill spans, in row order, each the row's first k in the proof order, ties included.
def test_rows_are_each_rows_top_k():
    torch.manual_seed(11)
    hidden = (torch.randint(-5, 6, (10, 64)).to(torch.bfloat16) / 4).contiguous()
    hidden[3] = torch.randn(64, dtype=torch.bfloat16)  # one row without ties
    # A decode row of "a", then 6 prefill rows of "b" (a later step of a chunked prefill),
    # then 3 prefill rows of "c".
    step = extract.spans(["a", "b", "c"], [1, 6, 3], [9, 4, 0], [9, 20, 3])
    prefill = [s for s in step if s.phase == client.PREFILL]
    got = list(extract.frames(*extract.rows(hidden, prefill, k=16)))
    assert [(r, p, n, len(pairs)) for r, p, n, pairs in got] == [("b", client.PREFILL, 64, 16)] * 6 + [
        ("c", client.PREFILL, 64, 16)
    ] * 3
    for (_, _, _, pairs), row in zip(got, hidden[1:10]):
        assert set(pairs) == reference_topk(row, 16)


def test_rows_narrower_than_k_send_everything():
    hidden = torch.randn(2, 4, dtype=torch.bfloat16)
    step = extract.spans(["a"], [2], [0], [2])
    got = list(extract.frames(*extract.rows(hidden, step, k=128)))
    assert [(n, sorted(i for i, _ in pairs)) for _, _, n, pairs in got] == [(4, [0, 1, 2, 3])] * 2
    meta, idx, bits = extract.rows(hidden, [], k=4)
    assert meta == [] and idx.numel() == 0
