#!/usr/bin/env python3
"""Probe the vLLM interfaces the public job worker relies on (m6-public-jobs 6.1, CI job
vllm-plugin).

Evaluation units (`ac-worker`, design D10) score each choice of a multiple-choice item by the log
likelihood of its continuation:

1. `/tokenize` of the context alone is a prefix of `/tokenize` of context + continuation (checked
   for a few items; a mismatch is reported, the worker then treats the item as an execution
   error);
2. `/v1/completions` with a list of token IDs, `max_tokens: 1` and `prompt_logprobs: 0` returns
   `choices[0].prompt_logprobs`: one entry per prompt token (the first `null`), each mapping the
   prompt's token ID (as a string) to an object with a `logprob`.

Embedding units need:

3. `vllm serve` of the pinned embedding model (pooling runner) answering `/v1/embeddings` with
   `data[i].embedding`, one vector per input text, all of one dimension.

Interface failures fail the script; the observed shapes are printed for the design.

Usage: scripts/probe-public-jobs-vllm.py   (after scripts/setup-vllm-cpu.sh; env AC_VLLM_MODEL,
AC_VLLM_REVISION, AC_EMBED_MODEL, AC_EMBED_REVISION)
"""

import json
import os
import secrets
import subprocess
import sys
import tempfile
import time
import urllib.request
from pathlib import Path

MODEL = os.environ["AC_VLLM_MODEL"]
REVISION = os.environ["AC_VLLM_REVISION"]
EMBED = os.environ["AC_EMBED_MODEL"]
EMBED_REVISION = os.environ["AC_EMBED_REVISION"]

ITEMS = [
    ("The capital of France is", [" Paris.", " Berlin.", " Madrid."]),
    ("Water boils at sea level at", [" 100 degrees Celsius.", " 50 degrees Celsius."]),
    ("2 + 2 =", [" 4", " 5", " 22"]),
    ("A cat is a kind of", [" animal.", " vegetable.", " mineral.", " planet."]),
]
TEXTS = [
    "A lighthouse guides ships at night.",
    "Hash functions map data of any size to a fixed size.",
    "Pancakes need flour, milk and eggs.",
]

failures: list[str] = []


def check(ok: bool, what: str) -> None:
    print(("ok   " if ok else "FAIL ") + what, flush=True)
    if not ok:
        failures.append(what)


def post(base: str, path: str, body: dict) -> dict:
    req = urllib.request.Request(
        base + path, data=json.dumps(body).encode(), headers={"Content-Type": "application/json"}
    )
    with urllib.request.urlopen(req, timeout=300) as r:
        return json.load(r)


def serve(args: list[str], workdir: str, name: str):
    """Starts `vllm serve` with `args`; returns (process, base URL) once it answers."""
    port = 18000 + secrets.randbelow(1000)
    base = f"http://127.0.0.1:{port}"
    log = open(Path(workdir) / f"{name}.log", "w")
    env = dict(os.environ, VLLM_USE_V2_MODEL_RUNNER="0")
    env.pop("AGENTCOIN_TOPLOC_SOCKET", None)
    proc = subprocess.Popen(
        ["vllm", "serve", *args, "--port", str(port)], env=env, stdout=log, stderr=subprocess.STDOUT
    )
    deadline = time.time() + 900
    while time.time() < deadline:
        try:
            urllib.request.urlopen(base + "/v1/models", timeout=5)
            return proc, base
        except OSError:
            if proc.poll() is not None:
                print(Path(workdir, f"{name}.log").read_text()[-4000:])
                return None, base
            time.sleep(2)
    proc.terminate()
    return None, base


def probe_eval(workdir: str) -> None:
    proc, base = serve(
        [MODEL, "--revision", REVISION, "--served-model-name", "eval-model", "--dtype", "bfloat16",
         "--no-enable-prefix-caching", "--max-model-len", "1024", "--gpu-memory-utilization", "0.3"],
        workdir, "eval",
    )
    check(proc is not None, "vllm serve (generation model) started")
    if proc is None:
        return
    try:
        prefix_ok = 0
        total = 0
        for context, choices in ITEMS:
            ctx = post(base, "/tokenize", {"model": "eval-model", "prompt": context})["tokens"]
            for cont in choices:
                total += 1
                full = post(base, "/tokenize", {"model": "eval-model", "prompt": context + cont})["tokens"]
                is_prefix = full[: len(ctx)] == ctx and len(full) > len(ctx)
                prefix_ok += is_prefix
                comp = post(base, "/v1/completions", {
                    "model": "eval-model", "prompt": full, "max_tokens": 1, "temperature": 0,
                    "prompt_logprobs": 0,
                })
                plp = comp["choices"][0].get("prompt_logprobs")
                check(isinstance(plp, list) and len(plp) == len(full),
                      f"prompt_logprobs has one entry per prompt token ({len(full)})")
                if not isinstance(plp, list) or len(plp) != len(full):
                    print("     choice:", json.dumps(comp["choices"][0])[:800])
                    continue
                check(plp[0] is None, "the first prompt token has no log probability")
                total_lp = 0.0
                shape_ok = True
                for pos in range(len(ctx), len(full)):
                    entry = plp[pos]
                    key = str(full[pos])
                    if not isinstance(entry, dict) or key not in entry or "logprob" not in entry[key]:
                        shape_ok = False
                        print("     entry:", json.dumps(entry)[:400])
                        break
                    total_lp += entry[key]["logprob"]
                check(shape_ok, f"each entry maps the prompt token ID to its logprob ({cont!r})")
                print(f"     {context!r} + {cont!r}: {len(full) - len(ctx)} tokens, "
                      f"log likelihood {total_lp:.4f}")
        print(f"tokenization prefix: {prefix_ok}/{total} continuations extend the context's tokens")
    finally:
        proc.terminate()
        proc.wait(timeout=60)


def probe_embed(workdir: str) -> None:
    common = [EMBED, "--revision", EMBED_REVISION, "--served-model-name", "embed-model",
              "--dtype", "bfloat16", "--gpu-memory-utilization", "0.3"]
    proc, base = serve([*common, "--runner", "pooling"], workdir, "embed")
    if proc is None:
        print("     --runner pooling refused; trying --task embed")
        proc, base = serve([*common, "--task", "embed"], workdir, "embed-task")
    check(proc is not None, "vllm serve (embedding model) started")
    if proc is None:
        return
    try:
        r = post(base, "/v1/embeddings", {"model": "embed-model", "input": TEXTS})
        vecs = [d["embedding"] for d in r["data"]]
        dims = {len(v) for v in vecs}
        check(len(vecs) == len(TEXTS), f"one embedding per text ({len(vecs)})")
        check(len(dims) == 1 and next(iter(dims)) > 0, f"embeddings share one dimension {sorted(dims)}")
        again = post(base, "/v1/embeddings", {"model": "embed-model", "input": TEXTS[:1]})["data"][0]["embedding"]
        diff = max(abs(a - b) for a, b in zip(again, vecs[0]))
        print(f"     dimension {sorted(dims)}, largest difference single vs batched: {diff:.3e}")
    finally:
        proc.terminate()
        proc.wait(timeout=60)


def main() -> int:
    workdir = tempfile.mkdtemp(prefix="ac-vllm-public-probe-")
    probe_eval(workdir)
    probe_embed(workdir)
    print("FAILED:" if failures else "all checks passed", *failures, sep="\n  ")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
