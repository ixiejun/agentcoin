#!/usr/bin/env python3
"""The vLLM TOPLOC plugin against a real (CPU) vLLM (m5-engine-toploc 7.2, CI job vllm-plugin).

Runs the pinned model in-process with the plugin enabled and a fake provider socket, and a
reference hook on the model's final norm (the activations the TOPLOC reference reads). Checks:

1. every segment the plugin sends equals the first k of that step's activations in the proof
   order (larger magnitude, lower index), with a prompt long enough for a chunked prefill;
2. the proofs built from the candidates (crates/ac-toploc example `toploc_from_candidates`) are
   byte-identical to the proofs of the whole activations;
3. on chunks without a tie at the k-th place they are byte-identical to the reference
   implementation's `build_proofs_bytes` (the reference leaves ties to `torch.topk`);
4. a float16 model or prefix caching makes the engine fail to start;
5. nothing of the prompt reaches the logs, and the plugin writes no file.

Usage: scripts/check-vllm-plugin.py   (after scripts/setup-vllm-cpu.sh; env AC_VLLM_MODEL,
AC_VLLM_REVISION)
"""

import json
import logging
import os
import socket
import struct
import subprocess
import sys
import tempfile
import threading
import time
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
MODEL = os.environ["AC_VLLM_MODEL"]
REVISION = os.environ["AC_VLLM_REVISION"]
TOPK, BATCH = 128, 32
MARKER = "quokka-marker-7b1e"
PROMPT = (
    f"Please repeat {MARKER} and then describe, in plain words, how a lighthouse keeper "
    "spends a long winter night: the lamp, the logbook, the weather, the ships that pass, "
    "the silence, the radio, the tea, the stairs, the storms and the first light of dawn. "
) * 3

failures: list[str] = []


def check(ok: bool, what: str) -> None:
    print(("ok   " if ok else "FAIL ") + what, flush=True)
    if not ok:
        failures.append(what)


class FakeProvider:
    """Answers Hello with Welcome(1, 128) and records Segment and Finish frames."""

    def __init__(self, path: str) -> None:
        self.segments: list[tuple[str, int, int, list[tuple[int, int]]]] = []
        self.finished: list[str] = []
        self.hidden = None
        self.server = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self.server.bind(path)
        self.server.listen()
        threading.Thread(target=self._serve, daemon=True).start()

    def _serve(self) -> None:
        while True:
            conn, _ = self.server.accept()
            threading.Thread(target=self._handle, args=(conn,), daemon=True).start()

    def _handle(self, conn: socket.socket) -> None:
        buf = b""
        greeted = False
        while True:
            data = conn.recv(1 << 20)
            if not data:
                return
            buf += data
            while len(buf) >= 4:
                (n,) = struct.unpack(">I", buf[:4])
                if len(buf) < 4 + n:
                    break
                p, buf = buf[4 : 4 + n], buf[4 + n :]
                if not greeted:
                    assert p[:5] == b"\x10ACTL" and p[5] == 1, p[:6]
                    (self.hidden,) = struct.unpack(">I", p[6:10])
                    conn.sendall(struct.pack(">IBBH", 4, 0x11, 1, TOPK))
                    greeted = True
                    continue
                (idlen,) = struct.unpack(">H", p[1:3])
                request = p[3 : 3 + idlen].decode()
                rest = p[3 + idlen :]
                if p[0] == 0x01:
                    phase, length, count = struct.unpack(">BIH", rest[:7])
                    pairs = [struct.unpack(">IH", rest[7 + 6 * i : 13 + 6 * i]) for i in range(count)]
                    self.segments.append((request, phase, length, pairs))
                elif p[0] == 0x02:
                    self.finished.append(request)


def proof_order(values) -> list[int]:
    """Indices of the first TOPK values: larger magnitude first, lower index among equals."""
    import numpy as np

    mag = np.abs(values.astype(np.float32))
    order = np.lexsort((np.arange(len(mag)), -mag))
    return order[: min(TOPK, len(mag))].tolist()


def bits(t) -> list[int]:
    import torch

    return [b & 0xFFFF for b in t.contiguous().view(torch.int16).tolist()]


def rust_proofs(segments: list[dict]) -> list[str]:
    """Proofs from `toploc_from_candidates` (segments with candidates or whole values)."""
    body = json.dumps(
        {
            "params": {"decode_batching_size": BATCH, "topk": TOPK, "skip_prefill": False},
            "segments": segments,
        }
    )
    out = subprocess.run(
        ["cargo", "run", "-q", "-p", "ac-toploc", "--example", "toploc_from_candidates"],
        input=body.encode(),
        capture_output=True,
        cwd=REPO,
        check=True,
    )
    return json.loads(out.stdout)["proofs"]


def refused(kwargs: str, needle: str) -> None:
    """A second engine with a bad configuration must fail to start, naming the reason."""
    code = (
        "from vllm import LLM\n"
        f"LLM(model={MODEL!r}, revision={REVISION!r}, max_model_len=512, enforce_eager=True, {kwargs})\n"
    )
    with tempfile.TemporaryDirectory() as d:
        env = dict(os.environ, AGENTCOIN_TOPLOC_SOCKET=os.path.join(d, "t.sock"))
        r = subprocess.run([sys.executable, "-c", code], capture_output=True, text=True, env=env, timeout=900)
    check(r.returncode != 0 and needle in (r.stdout + r.stderr), f"{kwargs} refused ({needle})")


def main() -> int:
    refused("dtype='float16', enable_prefix_caching=False", "bfloat16")
    refused("dtype='bfloat16', enable_prefix_caching=True", "--no-enable-prefix-caching")

    records: list[str] = []

    class Keep(logging.Handler):
        def emit(self, record: logging.LogRecord) -> None:
            records.append(self.format(record))

    logging.getLogger().addHandler(Keep())
    logging.getLogger().setLevel(logging.DEBUG)

    workdir = tempfile.mkdtemp(prefix="ac-vllm-check-")
    sock = os.path.join(workdir, "toploc.sock")
    provider = FakeProvider(sock)
    os.environ["AGENTCOIN_TOPLOC_SOCKET"] = sock
    os.environ["VLLM_ENABLE_V1_MULTIPROCESSING"] = "0"  # the reference hook runs in-process

    import torch
    from vllm import LLM, SamplingParams

    llm = LLM(
        model=MODEL,
        revision=REVISION,
        dtype="bfloat16",
        enable_prefix_caching=False,
        enforce_eager=True,  # module hooks run in eager mode only (the plugin does not need it)
        max_model_len=1024,
        enable_chunked_prefill=True,
        max_num_batched_tokens=64,  # the prompt's prefill takes several steps
        seed=0,
    )
    captured: list = []

    def attach(model):
        def hook(_module, _inputs, output):
            out = output[0] if isinstance(output, tuple) else output
            captured.append(out.detach().to("cpu").clone())

        return model.model.norm.register_forward_hook(hook)

    llm.apply_model(attach)
    deadline = time.time() + 120
    while provider.hidden is None and time.time() < deadline:
        time.sleep(0.2)
    check(provider.hidden is not None, "the plugin connected to the provider socket")
    captured.clear()
    provider.segments.clear()
    out = llm.generate([PROMPT], SamplingParams(temperature=0, max_tokens=70))[0]
    time.sleep(2)  # the sender thread drains its queue
    prompt_tokens = len(out.prompt_token_ids)
    output_tokens = len(out.outputs[0].token_ids)
    hidden = provider.hidden
    segs = provider.segments
    print(f"prompt {prompt_tokens} tokens, output {output_tokens}, hidden {hidden}, "
          f"{len(segs)} segments, {len(captured)} forward passes")

    prefill = [s for s in segs if s[1] == 0]
    decode = [s for s in segs if s[1] == 1]
    check(len(prefill) >= 2, f"the prefill took several steps ({len(prefill)})")
    check(sum(s[2] for s in prefill) == prompt_tokens * hidden, "prefill segments cover the prompt")
    check(len(decode) == output_tokens - 1, f"one decode segment per output token but the last ({len(decode)})")
    check(len(captured) == len(segs), "one segment per forward pass")

    # 1. Candidates against the reference hook's activations, step by step.
    rows = []
    for (request, phase, length, pairs), act in zip(segs, captured):
        tokens = length // hidden
        flat = act[:tokens].reshape(-1)
        rows.append(flat)
        want = proof_order(flat.float().numpy())
        all_bits = bits(flat)
        expected = set(zip(want, [all_bits[i] for i in want]))
        if set(pairs) != expected:
            check(False, f"candidates of a {'prefill' if phase == 0 else 'decode'} step")
            break
    else:
        check(True, "every segment is the first k of its activations in the proof order")

    # 2. Proofs from the candidates = proofs of the whole activations.
    by_candidates = rust_proofs(
        [{"phase": "prefill" if p == 0 else "decode", "len": n, "candidates": c} for _, p, n, c in segs]
    )
    whole = rust_proofs(
        [{"phase": "prefill" if s[1] == 0 else "decode", "values": bits(r)} for s, r in zip(segs, rows)]
    )
    check(by_candidates == whole, f"proofs from candidates equal the whole activations' ({len(whole)} chunks)")

    # 3. The reference, on chunks without a tie at the k-th place.
    from toploc import build_proofs_bytes

    n_pre = len(prefill)
    activations = [torch.cat(rows[:n_pre])] + rows[n_pre:]
    reference = [p.hex() for p in build_proofs_bytes(activations, decode_batching_size=BATCH, topk=TOPK, skip_prefill=False)]
    chunks = [activations[0]] + [torch.cat(activations[1 + i : 1 + i + BATCH]) for i in range(0, len(activations) - 1, BATCH)]
    compared = tied = 0
    for i, chunk in enumerate(chunks):
        mag = chunk.float().abs().sort(descending=True).values
        if mag.numel() > TOPK and mag[TOPK - 1] == mag[TOPK]:
            tied += 1
            continue
        compared += 1
        check(reference[i] == whole[i], f"chunk {i} equals the reference implementation")
    print(f"reference comparison: {compared} chunk(s) compared, {tied} with a tie at the k-th place skipped")
    check(len(reference) == len(whole), "the reference has as many chunks")

    # 4 and 5. Privacy.
    check(not any(MARKER in r for r in records), "no prompt content in the logs")
    check(os.listdir(workdir) == ["toploc.sock"], "the plugin wrote no file")

    print("FAILED:" if failures else "all checks passed", *failures, sep="\n  ")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
