"""Per-request top-k candidates of one forward step (spec market/engine-plugin
"每段激活的 top-k 候选")."""

from __future__ import annotations

from dataclasses import dataclass

import torch

from .client import DECODE, PREFILL


@dataclass(frozen=True)
class Span:
    """One request's part of a step: rows `[start, start + tokens)` of the hidden states."""

    request: str
    phase: int
    start: int
    tokens: int


def spans(req_ids, scheduled, computed, prompt_lens) -> list[Span]:
    """Splits a step's rows by request, in batch order.

    `scheduled[i]` tokens of request `req_ids[i]` were computed in this step; the step is a
    prefill if the request had computed fewer tokens than its prompt before it (a prefill split
    over several steps gives several prefill spans; a preempted and recomputed request shows up
    as a prefill again, which the provider refuses to prove).
    """
    out = []
    start = 0
    for req, n, done, prompt in zip(req_ids, scheduled, computed, prompt_lens):
        n = int(n)
        if n > 0:
            phase = PREFILL if int(done) < int(prompt) else DECODE
            out.append(Span(req, phase, start, n))
        start += n
    return out


@torch.no_grad()
def candidates(hidden: torch.Tensor, step: list[Span], k: int):
    """Top-k by magnitude of each span's flattened activations, on the device.

    Returns `(meta, indices, bits)`: `meta` holds `(request, phase, values, count)` per span in
    order; `indices` (int64) and `bits` (the bfloat16 values reinterpreted as int16) hold every
    span's candidates, concatenated in the same order.
    """
    if hidden.dtype != torch.bfloat16:
        raise TypeError("TOPLOC needs bfloat16 activations")
    meta, idx, bits = [], [], []
    hidden_size = hidden.shape[-1]
    for s in step:
        flat = hidden[s.start : s.start + s.tokens].reshape(-1)
        count = min(k, flat.numel())
        top = flat.abs().topk(count, sorted=False).indices
        meta.append((s.request, s.phase, s.tokens * hidden_size, count))
        idx.append(top)
        bits.append(flat[top].view(torch.int16))
    if not meta:
        empty = torch.empty(0, dtype=torch.int64)
        return meta, empty, empty.to(torch.int16)
    return meta, torch.cat(idx), torch.cat(bits)


def frames(meta, indices, bits):
    """Encodes host-side candidates as `(request, phase, values, pairs)` per span."""
    idx = indices.tolist()
    raw = [b & 0xFFFF for b in bits.tolist()]
    at = 0
    for request, phase, values, count in meta:
        yield request, phase, values, list(zip(idx[at : at + count], raw[at : at + count]))
        at += count
