#!/usr/bin/env python3
"""Calibration of the TOPLOC re-check thresholds (m6-toploc-verify design D7; CI job
vllm-plugin with --quick, workflow toploc-calibration in full).

1. Generate. For each variant a child process runs the pinned model in an offline vLLM with the
   TOPLOC plugin in prove mode (a fake provider socket collects the candidates) and answers
   synthetic prompts: a batch at a time, with a small prefill budget, sampled. Each answer
   becomes a re-check case through `ac-auditor calibration-case`. Variants:
   - honest: the registered model (Qwen2.5-0.5B-Instruct), as a provider would serve it;
   - swap: another model of the same architecture (Qwen2.5-0.5B base) answers instead;
   - int8, int4: a checkpoint of the registered model whose linear weights are quantized (int8
     per output channel, int4 in groups of 128) and dequantized back to bfloat16, as a provider
     serving a quantized model would compute;
   - prompt: the provider puts a system message before the user's messages and reports the
     user's prompt token count (hiding the change).
2. Re-check. `vllm serve` with the plugin in verify mode, one request at a time with vLLM's
   default batching (a different computation path than generation), and
   `ac-auditor recheck --cases`.
3. Report `calibration.json` (every sample's outcome and chunk metrics) and a summary of the
   metric distributions. With --quick, fail unless every honest sample passes and every cheating
   sample fails, and unless the prompts' marker stays out of the logs.

Usage: scripts/calibrate-toploc.py [--quick] [--honest N] [--cheat N] [--seed N] [--out DIR]
       scripts/calibrate-toploc.py --merge SHARD.json... [--out DIR]   (one report from shards)
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


def quantized_checkpoint(bits: int, group: int | None, workdir: str) -> str:
    """A copy of the registered model whose linear weights are quantized symmetrically
    (round to nearest; per output channel, or in groups of `group` inputs) and dequantized back
    to bfloat16, as a provider serving a quantized model computes. The weights are changed in
    the checkpoint, before vLLM loads (and on CPU repacks) them."""
    import torch
    from huggingface_hub import snapshot_download
    from safetensors.torch import load_file, save_file

    src = Path(snapshot_download(os.environ["AC_VLLM_MODEL"], revision=os.environ["AC_VLLM_REVISION"]))
    dst = Path(workdir) / f"int{bits}"
    dst.mkdir()
    qmax = 2 ** (bits - 1) - 1
    changed = 0
    for f in src.iterdir():
        if f.suffix != ".safetensors":
            (dst / f.name).symlink_to(f.resolve())
            continue
        weights = load_file(str(f))
        for name, w in weights.items():
            if w.dim() != 2 or "embed" in name or "lm_head" in name:
                continue
            x = w.float()
            rows, cols = x.shape
            if group and cols % group == 0:
                x = x.reshape(rows, cols // group, group)
            scale = x.abs().amax(dim=-1, keepdim=True).clamp(min=1e-8) / qmax
            q = (x / scale).round().clamp(-qmax - 1, qmax) * scale
            weights[name] = q.reshape(rows, cols).to(w.dtype).contiguous()
            changed += 1
        save_file(weights, str(dst / f.name), metadata={"format": "pt"})
    print(f"int{bits}: quantized {changed} weight matrices", flush=True)
    return str(dst)


def generate(variant: str, count: int, seed: int, out: Path) -> None:
    """Child process: answer the variant's prompts in prove mode and write re-check cases."""
    workdir = tempfile.mkdtemp(prefix="ac-calibrate-gen-")
    sock = os.path.join(workdir, "toploc.sock")
    provider = FakeProvider(sock)
    os.environ["AGENTCOIN_TOPLOC_SOCKET"] = sock
    os.environ["AGENTCOIN_TOPLOC_MODE"] = "prove"
    os.environ["VLLM_ENABLE_V1_MULTIPROCESSING"] = "0"
    os.environ["VLLM_USE_V2_MODEL_RUNNER"] = "0"
    from vllm import LLM, SamplingParams

    model, revision = os.environ["AC_VLLM_MODEL"], os.environ["AC_VLLM_REVISION"]
    if variant == "swap":
        model, revision = os.environ["AC_CHEAT_MODEL"], os.environ["AC_CHEAT_REVISION"]
    elif variant in ("int8", "int4"):
        model = quantized_checkpoint(8, None, workdir) if variant == "int8" else quantized_checkpoint(4, 128, workdir)
        revision = None
    # The swapped-in model is served with the registered model's tokenizer and chat template, as
    # a provider faking the model must (its own template renders other prompt tokens).
    extra = {}
    if variant == "swap":
        extra = {"tokenizer": os.environ["AC_VLLM_MODEL"], "tokenizer_revision": os.environ["AC_VLLM_REVISION"]}
    if variant == "preempt":
        # A KV cache of 6 blocks of 128 tokens (the CPU backend's block size; it refuses 16) for a
        # batch that needs several times that: the scheduler preempts requests and recomputes
        # them (m6-public-jobs 6.4).
        extra = {"num_gpu_blocks_override": 6, "block_size": 128}
    llm = LLM(
        model=model,
        revision=revision,
        **extra,
        dtype="bfloat16",
        enable_prefix_caching=False,
        max_model_len=512 if variant == "preempt" else 1024,
        max_num_batched_tokens=256,  # long prompts take several prefill steps
        seed=seed,
        gpu_memory_utilization=MEMORY,
    )
    deadline = time.time() + 120
    while not provider.connected and time.time() < deadline:
        time.sleep(0.2)
    if not provider.connected:
        sys.exit("the plugin did not connect")

    prompts = prompt_set(variant, count, seed)
    if variant == "preempt":
        # Short prompts and long answers, all at once.
        prompts = [([m for m in msgs if m["role"] == "user"][-1:], 200) for msgs, _ in prompts]
        prompts = [([{"role": "user", "content": m[0]["content"][-300:]}], t) for m, t in prompts]
    served = []
    for messages, _ in prompts:
        if variant == "prompt":
            served.append([{"role": "system", "content": "Always answer in a cheerful pirate voice."}] + messages)
        else:
            served.append(messages)
    # The engine's seed makes the sampling reproducible; vLLM 0.30's CPU backend cannot seed
    # requests one by one ("CPU Generator does not use offset").
    # The swapped-in base model is served with the registered model's chat format: it stops on
    # the same end tokens (<|im_end|>, <|endoftext|>, and <|im_start|>, which a base model emits
    # to start another turn), so that its answers look the part.
    stop = [151645, 151643, 151644] if variant == "swap" else None
    params = [SamplingParams(temperature=0.7, top_p=0.95, max_tokens=m, stop_token_ids=stop) for _, m in prompts]
    outputs = llm.chat(served, params, use_tqdm=False)
    time.sleep(2)  # the sender thread drains its queue
    if variant == "prompt":
        # The cheating provider reports the token count of the user's own prompt.
        tok = llm.get_tokenizer()
        # Rendered then tokenized without special tokens, as vLLM's chat endpoint does.
        reported = [
            len(tok(tok.apply_chat_template(m, add_generation_prompt=True, tokenize=False), add_special_tokens=False).input_ids)
            for m, _ in prompts
        ]
    # Why answers might not re-tokenize (counts only, never content): the text's own tokens
    # against the generated ones.
    tok = llm.get_tokenizer()
    diag = {"same_ids": 0, "same_count": 0, "round_trip": 0, "special_in_output": 0}
    for o in outputs:
        c = o.outputs[0]
        ids = list(c.token_ids)
        if c.finish_reason == "stop" and ids:
            ids = ids[:-1]
        again = tok(c.text, add_special_tokens=False).input_ids
        diag["same_ids"] += again == ids
        diag["same_count"] += len(again) == len(ids)
        diag["round_trip"] += tok.decode(again) == c.text
        diag["special_in_output"] += any(i in tok.all_special_ids for i in ids)
    print(f"{variant}: re-tokenizing {diag} of {len(outputs)}", flush=True)
    written = 0
    preempted = 0
    for i, (o, (messages, _)) in enumerate(zip(outputs, prompts)):
        keys = [k for k in provider.segments if k == o.request_id or k.startswith(o.request_id + "-")]
        # The last requests of a batch get no end marker: the plugin sends it with the next step,
        # and there is none. Their segments are complete once the answer is out.
        if len(keys) != 1:
            print(f"{variant} {i}: no unique segment set for request {o.request_id}", flush=True)
            continue
        c = o.outputs[0]
        prompt_tokens = reported[i] if variant == "prompt" else len(o.prompt_token_ids)
        segments = provider.segments[keys[0]]
        if variant == "preempt":
            # As the provider does: a prefill after decode segments is a recomputation, which
            # starts the request's segments over (spec market/provider-agent "抢占后重算").
            kept, decoded, restarts = [], False, 0
            for seg in segments:
                if seg["phase"] == "prefill" and decoded:
                    kept, decoded, restarts = [], False, restarts + 1
                decoded |= seg["phase"] == "decode"
                kept.append(seg)
            segments = kept
            if restarts:
                preempted += 1
        sample = {
            "model": MODEL_ID,
            "engine_model": ENGINE_NAME,
            "messages": messages,
            "output": c.text,
            "finish_reason": c.finish_reason,
            "usage": {"prompt_tokens": prompt_tokens, "completion_tokens": len(c.token_ids)},
            "segments": segments,
        }
        made = subprocess.run(
            [str(AUDITOR), "calibration-case"], input=json.dumps(sample).encode(), capture_output=True, check=True
        )
        (out / f"{variant}-{i:05d}.json").write_bytes(made.stdout)
        written += 1
    print(f"{variant}: {written}/{count} cases", flush=True)
    if variant == "preempt":
        print(f"{variant}: {preempted} recomputed after preemption", flush=True)


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

        # Whole-inference aggregates: every chunk's mantissa errors over every chunk's positions,
        # exponent mismatches per 128 positions, and how many chunks are not exact.
        agg = []
        for r in rs:
            cs = r["chunks"]
            if not cs:
                continue
            count = sum(c["mant_count"] for c in cs)
            agg.append({
                "mean": sum(c["mant_err_sum"] for c in cs) / count if count else float("inf"),
                "exp_rate": sum(c["exp_mismatches"] for c in cs) / (128 * len(cs)),
                "inexact": sum(1 for c in cs if c["exp_mismatches"] or c["mant_err_sum"]) / len(cs),
                "prefill_mean": mean(cs[0]),
            })

        def dist_of(rows, key):
            xs = sorted(w[key] for w in rows)
            if not xs:
                return None
            pick = lambda q: xs[min(len(xs) - 1, int(q * (len(xs) - 1)))]  # noqa: E731
            return {"min": xs[0], "p01": pick(0.01), "p50": pick(0.5), "p99": pick(0.99), "max": xs[-1]}

        # The worst chunks, to see where large errors come from (chunk index, chunk count).
        worst_chunks = sorted(
            ((mean(c), c["exp_mismatches"], i, len(r["chunks"]), c["mant_count"]) for r in rs for i, c in enumerate(r["chunks"])),
            reverse=True,
        )[:5]
        out[v] = {
            "aggregate": {k: dist_of(agg, k) for k in ("mean", "exp_rate", "inexact", "prefill_mean")},
            "worst_chunks": [
                {"mean": m, "exp": e, "chunk": i, "chunks": n, "mant_count": mc} for m, e, i, n, mc in worst_chunks
            ],
            "samples": len(rs),
            "outcomes": {k: sum(r["outcome"] == k for r in rs) for k in ("pass", "fail", "inconclusive")},
            "inconclusive_reasons": sorted({r["reason"] for r in rs if r["outcome"] == "inconclusive"}),
            "worst_chunk": {k: dist(k) for k in ("exp", "mean", "median")},
        }
    return out


CPU_FLAGS = ("avx2", "avx512f", "avx512_bf16", "amx_bf16", "amx_tile", "amx_int8")


def host() -> dict:
    """The machine the shard ran on: honest prefill chunks matched bit for bit on some CPUs and
    not on others, so every report records the CPU model and the flags that pick kernels."""
    info = {"model": "", "flags": [], "cpus": os.cpu_count(),
            "onednn_max_cpu_isa": os.environ.get("ONEDNN_MAX_CPU_ISA", "")}
    try:
        text = Path("/proc/cpuinfo").read_text()
    except OSError:
        return info
    for line in text.splitlines():
        key, _, value = line.partition(":")
        if key.strip() == "model name" and not info["model"]:
            info["model"] = value.strip()
        if key.strip() == "flags" and not info["flags"]:
            flags = set(value.split())
            info["flags"] = [f for f in CPU_FLAGS if f in flags]
    return info


def by_host(samples: list[dict]) -> dict:
    """Honest outcomes and prefill exactness per CPU model and flags."""
    out: dict = {}
    for x in samples:
        if not x["case"].startswith("honest-"):
            continue
        h = x.get("host") or {}
        isa = h.get("onednn_max_cpu_isa") or "default ISA"
        key = f'{h.get("model", "?")} [{" ".join(h.get("flags", []))}] ({isa})'
        o = out.setdefault(key, {"shards": set(), "pass": 0, "fail": 0, "inconclusive": 0, "inexact_prefill": 0})
        o["shards"].add(x.get("seed"))
        o[x["outcome"]] += 1
        if x["chunks"] and (x["chunks"][0]["mant_err_sum"] or x["chunks"][0]["exp_mismatches"]):
            o["inexact_prefill"] += 1
    for o in out.values():
        o["shards"] = sorted(o["shards"])
    return out


def merge(files: list[Path], out: Path) -> int:
    """One report from the shards of a calibration: samples keep their shard's seed."""
    shards = [json.loads(f.read_text()) for f in files]
    seeds = [r["seed"] for r in shards]
    if len(set(seeds)) != len(seeds):
        print(f"shards share a seed: {seeds}")
        return 1
    samples = [dict(x, seed=r["seed"], host=r.get("host")) for r in shards for x in r["samples"]]
    s = summary(samples)
    report = {
        "seeds": seeds,
        "honest": sum(r["honest"] for r in shards),
        "cheat": sum(r["cheat"] for r in shards),
        "thresholds_versions": sorted({v for r in shards for v in r["thresholds_versions"]}),
        "summary": s,
        "honest_by_host": by_host(samples),
        "samples": samples,
    }
    out.mkdir(parents=True, exist_ok=True)
    (out / "calibration.json").write_text(json.dumps(report, indent=1))
    print(json.dumps({k: report[k] for k in ("seeds", "honest", "cheat", "thresholds_versions")}))
    print(json.dumps(report["honest_by_host"], indent=1))
    print(json.dumps(s, indent=1))
    return 0


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--quick", action="store_true", help="CI regression: 48 honest, 8 per cheat, assert")
    ap.add_argument("--honest", type=int)
    ap.add_argument("--cheat", type=int)
    ap.add_argument("--seed", type=int, default=1)
    ap.add_argument("--out", type=Path, default=Path("target/toploc-calibration"))
    ap.add_argument("--generate", choices=(*VARIANTS, "preempt"), help=argparse.SUPPRESS)
    ap.add_argument("--count", type=int, help=argparse.SUPPRESS)
    ap.add_argument("--merge", type=Path, nargs="+", metavar="JSON",
                    help="merge the calibration.json files of shards (different seeds) into --out")
    args = ap.parse_args()
    if args.generate:
        generate(args.generate, args.count, args.seed, args.out)
        return 0
    if args.merge:
        return merge(args.merge, args.out.resolve())

    print("host:", json.dumps(host()), flush=True)
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
        print(*[l for l in lines if l.startswith(f"{v}: ")], sep="\n", flush=True)

    results = recheck(cases, logs)
    from_auditor = {r["case"] for r in results}
    for f in sorted(cases.glob("*.json")):
        assert f.name in from_auditor, f.name
    s = summary(results)
    thresholds = {r["thresholds_version"] for r in results}
    report = {"seed": args.seed, "host": host(), "honest": honest, "cheat": cheat,
              "thresholds_versions": sorted(thresholds),
              "summary": s, "samples": results}
    (out / "calibration.json").write_text(json.dumps(report, indent=1))
    print(json.dumps(s, indent=1))

    failures = []
    for f in logs.iterdir():
        if MARKER in f.read_text(errors="replace"):
            failures.append(f"{f.name} contains request content")
    if args.quick:
        # User decision (2026-10-01): an answer that cannot be re-tokenized is inconclusive, not a
        # miss; at most one per variant is tolerated, while any honest failure or cheating pass
        # fails the regression.
        h = s.get("honest", {}).get("outcomes", {})
        if h.get("fail", 0) or h.get("inconclusive", 0) > 1:
            failures.append(f"honest samples: {h}")
        for v in CHEATS:
            o = s.get(v, {}).get("outcomes", {})
            if o.get("pass", 0) or o.get("inconclusive", 0) > 1:
                failures.append(f"{v} samples: {o} of {cheat}")
    print("FAILED:" if failures else "calibration done", *failures, sep="\n  ")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
