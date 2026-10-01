#!/usr/bin/env python3
"""Calibration of the TOPLOC re-check thresholds (m6-toploc-verify design D7; CI job
vllm-plugin with --quick, workflow toploc-calibration in full).

1. Generate. For each variant a child process runs the pinned model in an offline vLLM with the
   TOPLOC plugin in prove mode (a fake provider socket collects the candidates) and answers
   synthetic prompts: a batch at a time, with a small prefill budget, sampled. Each answer
   becomes a re-check case through `ac-auditor calibration-case`. Variants:
   - honest: the registered model (Qwen2.5-0.5B-Instruct), as a provider would serve it;
   - swap: another model of the same architecture (Qwen2.5-0.5B base) answers instead;
   - int8, int4: the registered model with its linear weights quantized (int8 per output
     channel, int4 in groups of 128) and dequantized back to bfloat16, as a provider serving a
     quantized model would compute;
   - prompt: the provider puts a system message before the user's messages and reports the
     user's prompt token count (hiding the change).
2. Re-check. `vllm serve` with the plugin in verify mode, one request at a time with vLLM's
   default batching (a different computation path than generation), and
   `ac-auditor recheck --cases`.
3. Report `calibration.json` (every sample's outcome and chunk metrics) and a summary of the
   metric distributions. With --quick, fail unless every honest sample passes and every cheating
   sample fails, and unless the prompts' marker stays out of the logs.

Usage: scripts/calibrate-toploc.py [--quick] [--honest N] [--cheat N] [--out DIR]
       (after scripts/setup-vllm-cpu.sh and `cargo build -p ac-auditor`; env AC_VLLM_MODEL,
       AC_VLLM_REVISION, AC_CHEAT_MODEL, AC_CHEAT_REVISION)
"""

from __future__ import annotations

import argparse
import json
import os
import random
import secrets
import socket
import struct
import subprocess
import sys
import tempfile
import threading
import time
import urllib.request
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
AUDITOR = REPO / "target" / "debug" / "ac-auditor"
TOPK = 128
MEMORY = 0.3  # share of RAM vLLM's CPU backend reserves; runners have about 16 GB
MODEL_ID = "0x" + "71" * 32  # stands for the registered model; the re-check takes --quant bf16
ENGINE_NAME = "calibration-model"
MARKER = "narwhal-marker-5e8a"
VARIANTS = ("honest", "swap", "int8", "int4", "prompt")
CHEATS = VARIANTS[1:]

TOPICS = [
    "lighthouses", "volcanoes", "bread baking", "the stock market", "honeybees", "glaciers",
    "medieval castles", "electric cars", "chess openings", "coral reefs", "jazz music",
    "the moon landing", "rainforests", "compilers", "origami", "tides", "solar panels",
    "the printing press", "wolves", "hot air balloons", "public libraries", "tea ceremonies",
    "earthquakes", "bicycles", "the immune system", "deserts", "sailing", "vaccines",
]
TASKS = [
    "Explain {t} to a curious teenager.",
    "Write a short story that involves {t}.",
    "List five surprising facts about {t}.",
    "Give a step-by-step guide related to {t}.",
    "Compare {t} with something from everyday life.",
    "Write a poem about {t}.",
    "Summarize the history of {t} in a few sentences.",
    "What are common misconceptions about {t}?",
]
CONTEXT = [
    "Keep the answer friendly and clear.",
    "Use simple words where you can.",
    "Mention one example from a different country.",
    "Avoid jargon unless you explain it.",
    "End with a question for the reader.",
    "Assume the reader has no background knowledge.",
]


def prompt_set(variant: str, count: int, seed: int) -> list[tuple[list[dict], int]]:
    """`count` (messages, max_tokens) pairs; deterministic per variant and seed."""
    rng = random.Random(f"{seed}-{variant}")
    out = []
    for i in range(count):
        topic = rng.choice(TOPICS)
        task = rng.choice(TASKS).format(t=topic)
        # 20 to about 400 prompt tokens: a few context sentences, sometimes a long preamble.
        extra = " ".join(rng.choice(CONTEXT) for _ in range(rng.randint(0, 3)))
        preamble = ""
        if rng.random() < 0.3:
            preamble = " ".join(
                f"Background {k}: {rng.choice(TOPICS)} relates to {rng.choice(TOPICS)}."
                for k in range(rng.randint(5, 40))
            ) + " "
        content = f"{preamble}{task} {extra} (ref {MARKER}-{i})".strip()
        messages = [{"role": "user", "content": content}]
        if rng.random() < 0.3:
            messages.insert(0, {"role": "system", "content": "You are a helpful assistant."})
        out.append((messages, rng.randint(16, 128)))
    return out


class FakeProvider:
    """Answers the plugin's Hello (v2, prove mode) and keeps every request's segments."""

    def __init__(self, path: str) -> None:
        self.segments: dict[str, list] = {}
        self.finished: set[str] = set()
        self.server = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self.server.bind(path)
        self.server.listen()
        self.connected = False
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
                    assert p[:5] == b"\x10ACTL" and p[5] == 2 and p[10] == 0, p[:11]
                    conn.sendall(struct.pack(">IBBH", 4, 0x11, 2, TOPK))
                    greeted = self.connected = True
                    continue
                (idlen,) = struct.unpack(">H", p[1:3])
                request = p[3 : 3 + idlen].decode()
                rest = p[3 + idlen :]
                if p[0] == 0x01:
                    phase, length, count = struct.unpack(">BIH", rest[:7])
                    pairs = [list(struct.unpack(">IH", rest[7 + 6 * i : 13 + 6 * i])) for i in range(count)]
                    self.segments.setdefault(request, []).append(
                        {"phase": "prefill" if phase == 0 else "decode", "len": length, "candidates": pairs}
                    )
                elif p[0] == 0x02:
                    self.finished.add(request)


def quantize(bits: int, group: int | None):
    """A function for `LLM.apply_model`: symmetric round-to-nearest quantization of every 2-D
    linear weight (not the embeddings), dequantized back in place."""

    def apply(model):
        import torch

        qmax = 2 ** (bits - 1) - 1
        n = 0
        with torch.no_grad():
            for name, p in model.named_parameters():
                if p.dim() != 2 or "embed" in name or "lm_head" in name:
                    continue
                w = p.data.float()
                rows, cols = w.shape
                if group and cols % group == 0:
                    w = w.reshape(rows, cols // group, group)
                scale = w.abs().amax(dim=-1, keepdim=True).clamp(min=1e-8) / qmax
                q = (w / scale).round().clamp(-qmax - 1, qmax) * scale
                p.data.copy_(q.reshape(rows, cols).to(p.dtype))
                n += 1
        return n

    return apply


def generate(variant: str, count: int, seed: int, out: Path) -> None:
    """Child process: answer the variant's prompts in prove mode and write re-check cases."""
    workdir = tempfile.mkdtemp(prefix="ac-calibrate-gen-")
    sock = os.path.join(workdir, "toploc.sock")
    provider = FakeProvider(sock)
    os.environ["AGENTCOIN_TOPLOC_SOCKET"] = sock
    os.environ["AGENTCOIN_TOPLOC_MODE"] = "prove"
    os.environ["VLLM_ENABLE_V1_MULTIPROCESSING"] = "0"  # apply_model runs in this process
    os.environ["VLLM_USE_V2_MODEL_RUNNER"] = "0"
    from vllm import LLM, SamplingParams

    if variant == "swap":
        model, revision = os.environ["AC_CHEAT_MODEL"], os.environ["AC_CHEAT_REVISION"]
    else:
        model, revision = os.environ["AC_VLLM_MODEL"], os.environ["AC_VLLM_REVISION"]
    llm = LLM(
        model=model,
        revision=revision,
        dtype="bfloat16",
        enable_prefix_caching=False,
        max_model_len=1024,
        max_num_batched_tokens=256,  # long prompts take several prefill steps
        seed=seed,
        gpu_memory_utilization=MEMORY,
    )
    if variant in ("int8", "int4"):
        changed = llm.apply_model(quantize(8, None) if variant == "int8" else quantize(4, 128))
        print(f"{variant}: quantized {changed} weight matrices", flush=True)
    deadline = time.time() + 120
    while not provider.connected and time.time() < deadline:
        time.sleep(0.2)
    if not provider.connected:
        sys.exit("the plugin did not connect")

    prompts = prompt_set(variant, count, seed)
    served = []
    for messages, _ in prompts:
        if variant == "prompt":
            served.append([{"role": "system", "content": "Always answer in a cheerful pirate voice."}] + messages)
        else:
            served.append(messages)
    # The engine's seed makes the sampling reproducible; vLLM 0.30's CPU backend cannot seed
    # requests one by one ("CPU Generator does not use offset").
    params = [SamplingParams(temperature=0.7, top_p=0.95, max_tokens=m) for _, m in prompts]
    outputs = llm.chat(served, params, use_tqdm=False)
    time.sleep(2)  # the sender thread drains its queue
    if variant == "prompt":
        # The cheating provider reports the token count of the user's own prompt.
        tok = llm.get_tokenizer()
        reported = [len(tok.apply_chat_template(m, add_generation_prompt=True, tokenize=True)) for m, _ in prompts]
    written = 0
    for i, (o, (messages, _)) in enumerate(zip(outputs, prompts)):
        keys = [k for k in provider.segments if k == o.request_id or k.startswith(o.request_id + "-")]
        keys = [k for k in keys if k in provider.finished]
        if len(keys) != 1:
            print(f"{variant} {i}: no unique segment set for request {o.request_id}", flush=True)
            continue
        c = o.outputs[0]
        prompt_tokens = reported[i] if variant == "prompt" else len(o.prompt_token_ids)
        sample = {
            "model": MODEL_ID,
            "engine_model": ENGINE_NAME,
            "messages": messages,
            "output": c.text,
            "finish_reason": c.finish_reason,
            "usage": {"prompt_tokens": prompt_tokens, "completion_tokens": len(c.token_ids)},
            "segments": provider.segments[keys[0]],
        }
        made = subprocess.run(
            [str(AUDITOR), "calibration-case"], input=json.dumps(sample).encode(), capture_output=True, check=True
        )
        (out / f"{variant}-{i:05d}.json").write_bytes(made.stdout)
        written += 1
    print(f"{variant}: {written}/{count} cases", flush=True)


def recheck(cases: Path, logs: Path) -> list[dict]:
    """Starts a verify-mode `vllm serve` and re-checks every case."""
    workdir = tempfile.mkdtemp(prefix="ac-calibrate-check-")
    sock = os.path.join(workdir, "recheck.sock")
    port = 18000 + secrets.randbelow(1000)
    env = dict(
        os.environ,
        AGENTCOIN_TOPLOC_SOCKET=sock,
        AGENTCOIN_TOPLOC_MODE="verify",
        VLLM_USE_V2_MODEL_RUNNER="0",
    )
    log = open(logs / "vllm-verify.log", "w")
    engine = subprocess.Popen(
        [
            "vllm", "serve", os.environ["AC_VLLM_MODEL"], "--revision", os.environ["AC_VLLM_REVISION"],
            "--served-model-name", ENGINE_NAME, "--dtype", "bfloat16", "--no-enable-prefix-caching",
            "--max-model-len", "1024", "--gpu-memory-utilization", str(MEMORY), "--port", str(port),
        ],
        env=env, stdout=log, stderr=subprocess.STDOUT,
    )
    try:
        deadline = time.time() + 900
        while True:
            try:
                urllib.request.urlopen(f"http://127.0.0.1:{port}/v1/models", timeout=5)
                break
            except OSError:
                if engine.poll() is not None or time.time() > deadline:
                    sys.exit("the re-check engine did not start; see vllm-verify.log")
                time.sleep(2)
        with open(logs / "auditor.log", "w") as err:
            run = subprocess.run(
                [
                    str(AUDITOR), "recheck", "--cases", str(cases), "--engine", f"http://127.0.0.1:{port}",
                    "--socket", sock, "--quant", "bf16", "--log-level", "debug",
                ],
                stdout=subprocess.PIPE, stderr=err, check=True,
            )
        return [json.loads(line) for line in run.stdout.decode().splitlines() if line.strip()]
    finally:
        engine.terminate()
        engine.wait(timeout=60)
        log.close()


def mean(c: dict) -> float:
    return c["mant_err_sum"] / c["mant_count"] if c["mant_count"] else float("inf")


def summary(results: list[dict]) -> dict:
    """Per variant: outcomes, and the distribution of each sample's worst chunk."""
    out = {}
    for v in VARIANTS:
        rs = [r for r in results if r["case"].startswith(v + "-")]
        if not rs:
            continue
        worst = [
            {
                "exp": max(c["exp_mismatches"] for c in r["chunks"]),
                "mean": max(mean(c) for c in r["chunks"]),
                "median": max((c["median"] if c["median"] is not None else 255) for c in r["chunks"]),
            }
            for r in rs
            if r["chunks"]
        ]

        def dist(key):
            xs = sorted(w[key] for w in worst)
            if not xs:
                return None
            pick = lambda q: xs[min(len(xs) - 1, int(q * (len(xs) - 1)))]  # noqa: E731
            return {"min": xs[0], "p01": pick(0.01), "p50": pick(0.5), "p99": pick(0.99), "max": xs[-1]}

        out[v] = {
            "samples": len(rs),
            "outcomes": {k: sum(r["outcome"] == k for r in rs) for k in ("pass", "fail", "inconclusive")},
            "inconclusive_reasons": sorted({r["reason"] for r in rs if r["outcome"] == "inconclusive"}),
            "worst_chunk": {k: dist(k) for k in ("exp", "mean", "median")},
        }
    return out


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--quick", action="store_true", help="CI regression: 48 honest, 8 per cheat, assert")
    ap.add_argument("--honest", type=int)
    ap.add_argument("--cheat", type=int)
    ap.add_argument("--seed", type=int, default=1)
    ap.add_argument("--out", type=Path, default=Path("target/toploc-calibration"))
    ap.add_argument("--generate", choices=VARIANTS, help=argparse.SUPPRESS)
    ap.add_argument("--count", type=int, help=argparse.SUPPRESS)
    args = ap.parse_args()
    if args.generate:
        generate(args.generate, args.count, args.seed, args.out)
        return 0

    honest = args.honest or (48 if args.quick else 3000)
    cheat = args.cheat or (8 if args.quick else 100)
    out = args.out.resolve()
    cases = out / "cases"
    cases.mkdir(parents=True, exist_ok=True)
    for f in cases.glob("*.json"):
        f.unlink()
    logs = out / "logs"
    logs.mkdir(exist_ok=True)
    for v in VARIANTS:
        n = honest if v == "honest" else cheat
        with open(logs / f"generate-{v}.log", "w") as log:
            run = subprocess.run(
                [sys.executable, __file__, "--generate", v, "--count", str(n), "--seed", str(args.seed), "--out", str(cases)],
                stdout=log, stderr=subprocess.STDOUT,
            )
        lines = (logs / f"generate-{v}.log").read_text(errors="replace").strip().splitlines()
        if run.returncode != 0:
            # The log holds vLLM's messages, never the prompts (the marker check below covers it).
            print(f"generating {v} failed; the end of its log:", *lines[-60:], sep="\n", flush=True)
            return 1
        print(lines[-1] if lines else f"{v}: no output", flush=True)

    results = recheck(cases, logs)
    from_auditor = {r["case"] for r in results}
    for f in sorted(cases.glob("*.json")):
        assert f.name in from_auditor, f.name
    s = summary(results)
    thresholds = {r["thresholds_version"] for r in results}
    report = {"seed": args.seed, "honest": honest, "cheat": cheat, "thresholds_versions": sorted(thresholds),
              "summary": s, "samples": results}
    (out / "calibration.json").write_text(json.dumps(report, indent=1))
    print(json.dumps(s, indent=1))

    failures = []
    for f in logs.iterdir():
        if MARKER in f.read_text(errors="replace"):
            failures.append(f"{f.name} contains request content")
    if args.quick:
        h = s.get("honest", {}).get("outcomes", {})
        if h.get("pass") != honest:
            failures.append(f"honest samples: {h}")
        for v in CHEATS:
            o = s.get(v, {}).get("outcomes", {})
            if o.get("fail") != cheat:
                failures.append(f"{v} samples: {o} of {cheat}")
    print("FAILED:" if failures else "calibration done", *failures, sep="\n  ")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
