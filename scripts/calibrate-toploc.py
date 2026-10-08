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

Generation and re-check can run on different machines (m6-toploc-gpu-calibration design D1):
--generate-only writes a case bundle (`cases/`, `prover.json` with the prover's hardware
fingerprint, `MANIFEST.sha256`), --recheck-only re-checks a bundle and records the auditor's
fingerprint, and --merge counts every sample in its "prover → auditor" cell, replays the
thresholds on the chunk metrics and states the conclusion per side of the audit length band (keep
or none). Without either option both steps run on this machine, as before.

Usage: scripts/calibrate-toploc.py [--quick] [--honest N] [--cheat N] [--seed N] [--out DIR]
       scripts/calibrate-toploc.py --generate-only [--honest N] [--cheat N] [--seed N] --out BUNDLE
       scripts/calibrate-toploc.py --recheck-only BUNDLE [--shard K/N] --out DIR
       scripts/calibrate-toploc.py --compare A.json B.json   (same cases, same outcomes)
       scripts/calibrate-toploc.py --merge SHARD.json... [--thresholds prefill=E,M,D decode=E,M,D]
                                   [--summary-out FILE] [--out DIR]
       (after scripts/setup-vllm-cpu.sh or scripts/gpu-calibration.sh setup, and
       `cargo build -p ac-auditor`; env AC_VLLM_MODEL, AC_VLLM_REVISION, AC_CHEAT_MODEL,
       AC_CHEAT_REVISION; CALIBRATION_PROVER_ISA sets the generating engines' ONEDNN_MAX_CPU_ISA)
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
MEMORY = 0.3  # share of RAM vLLM's CPU backend reserves (runners have about 16 GB), or of a GPU's memory
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


def prompt_set(variant: str, count: int, seed: int, min_words: int = 0,
               preamble: bool = True) -> list[tuple[list[dict], int]]:
    """`count` (messages, max_tokens) pairs; deterministic per variant and seed. With `min_words`,
    every user message is lengthened with background sentences to at least that many words (from
    a separate random stream, so that the other prompts stay those of the seed). Without
    `preamble`, no prompt gets the long preamble (the generation fits them to target lengths)."""
    rng = random.Random(f"{seed}-{variant}")
    out = []
    for i in range(count):
        topic = rng.choice(TOPICS)
        task = rng.choice(TASKS).format(t=topic)
        # 20 to about 400 prompt tokens: a few context sentences, sometimes a long preamble.
        extra = " ".join(rng.choice(CONTEXT) for _ in range(rng.randint(0, 3)))
        preamble_text = ""
        if rng.random() < 0.3 and preamble:
            preamble_text = " ".join(
                f"Background {k}: {rng.choice(TOPICS)} relates to {rng.choice(TOPICS)}."
                for k in range(rng.randint(5, 40))
            ) + " "
        content = f"{preamble_text}{task} {extra} (ref {MARKER}-{i})".strip()
        if min_words:
            pad = random.Random(f"{seed}-{variant}-pad-{i}")
            while len(content.split()) < min_words:
                content = f"Background: {pad.choice(TOPICS)} relates to {pad.choice(TOPICS)}. {content}"
        messages = [{"role": "user", "content": content}]
        if rng.random() < 0.3:
            messages.insert(0, {"role": "system", "content": "You are a helpful assistant."})
        out.append((messages, rng.randint(16, 128)))
    return out


# Prompt lengths of the calibration (m6-toploc-gpu-calibration design D10): about 80% of the
# prompts in the audit length band, 10% shorter (from 20 tokens) and 10% longer (to 600), so that
# both sides of the band are calibrated. A background sentence is at most about 15 tokens: targets
# keep that much clear of the band's ends, so that a prompt fitted to one stays on its side.
SHORTEST, LONGEST, SENTENCE = 20, 600, 16


def auditor_thresholds() -> dict:
    """The thresholds `ac-auditor` judges by (`ac-auditor thresholds`), as `THRESHOLDS` holds
    them: one source for the band, the crate's constant."""
    made = subprocess.run([str(AUDITOR), "thresholds"], capture_output=True, check=True)
    t = json.loads(made.stdout)
    return {k: (tuple(v) if isinstance(v, list) else v) for k, v in t.items()}


def stale_auditor() -> str | None:
    """Why the `ac-auditor` binary does not judge by this checkout's thresholds, or None: a binary
    built before a `git pull` judges by older ones (calibration-results/gpu-check-v3-2026-10-09)."""
    try:
        t = auditor_thresholds()
    except (OSError, subprocess.CalledProcessError, ValueError) as e:
        return f"{AUDITOR} cannot print its thresholds ({e}): rebuild it (cargo build -p ac-auditor)"
    if t != CURRENT:
        return f"{AUDITOR} judges by {t}, this checkout by {CURRENT}: rebuild it (cargo build -p ac-auditor)"
    return None


def prompt_targets(variant: str, count: int, seed: int, band: tuple[int, int]) -> list[int]:
    """A target prompt token count per prompt, deterministic per variant and seed."""
    rng = random.Random(f"{seed}-{variant}-target")
    lo, hi = band
    out = []
    for _ in range(count):
        r = rng.random()
        if r < 0.8:
            out.append(rng.randint(lo, max(lo, hi - SENTENCE)))
        elif r < 0.9:
            out.append(rng.randint(SHORTEST, max(SHORTEST, lo - SENTENCE)))
        else:
            out.append(rng.randint(hi + 1, LONGEST))
    return out


def fit_prompt(messages: list[dict], target: int, count, rng: random.Random) -> list[dict]:
    """The messages with background sentences put before the last user message's text until
    `count(messages)` (prompt tokens under the chat template) reaches `target`; a prompt already
    that long is kept. The question and the marker stay at the end."""
    out = [dict(m) for m in messages]
    last = max(i for i, m in enumerate(out) if m["role"] == "user")
    question = out[last]["content"]
    background: list[str] = []
    while count(out) < target:
        background.append(f"Background: {rng.choice(TOPICS)} relates to {rng.choice(TOPICS)}.")
        out[last]["content"] = " ".join(background) + " " + question
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


def generate(variant: str, count: int, seed: int, out: Path, min_words: int = 0,
             band: tuple[int, int] | None = None) -> None:
    """Child process: answer the variant's prompts in prove mode and write re-check cases. With
    `band` (and no `min_words`), prompts are fitted to target lengths around it (design D10)."""
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

    targeted = band is not None and not min_words and variant != "preempt"
    prompts = prompt_set(variant, count, seed, min_words, preamble=not targeted)
    if targeted:
        tok = llm.get_tokenizer()

        def count_tokens(m: list[dict]) -> int:
            # Rendered then tokenized without special tokens, as vLLM's chat endpoint does.
            text = tok.apply_chat_template(m, add_generation_prompt=True, tokenize=False)
            return len(tok(text, add_special_tokens=False).input_ids)

        targets = prompt_targets(variant, count, seed, band)
        prompts = [
            (fit_prompt(m, t, count_tokens, random.Random(f"{seed}-{variant}-fit-{i}")), mt)
            for i, ((m, mt), t) in enumerate(zip(prompts, targets))
        ]
        inside = sum(band[0] <= count_tokens(m) <= band[1] for m, _ in prompts)
        print(f"{variant}: {inside}/{count} prompts in the band {band[0]}-{band[1]}", flush=True)
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
    lengths = {}
    unproven: dict[str, int] = {}
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
        if variant == "prompt":
            # The cheat reports the user's prompt tokens; its own code fits the segments to what
            # its engine computed.
            sample["engine_prompt_tokens"] = len(o.prompt_token_ids)
        made = subprocess.run(
            [str(AUDITOR), "calibration-case"], input=json.dumps(sample).encode(), capture_output=True
        )
        if made.returncode != 0:
            # Segments that do not fit the usage: the provider would give no proof
            # (m6-toploc-async-stop design D1). The reason holds counts only.
            why = (made.stderr.decode(errors="replace").strip().splitlines() or ["?"])[-1]
            unproven[why] = unproven.get(why, 0) + 1
            continue
        (out / f"{variant}-{i:05d}.json").write_bytes(made.stdout)
        lengths[f"{variant}-{i:05d}.json"] = prompt_tokens
        written += 1
    # The prompt token count of every case (numbers only), for the statistics by prompt length.
    (out.parent / f"prompt-tokens-{variant}.json").write_text(json.dumps(lengths))
    (out.parent / f"unproven-{variant}.json").write_text(json.dumps(unproven))
    print(f"{variant}: {written}/{count} cases", flush=True)
    if unproven:
        print(f"{variant}: {sum(unproven.values())} without proof at the prover: {unproven}", flush=True)
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


def gpu() -> dict | None:
    """The GPU the engine runs on, if any (m6-toploc-gpu-calibration design D2)."""
    try:
        import torch
    except ImportError:
        return None
    if not torch.cuda.is_available():
        return None
    props = torch.cuda.get_device_properties(0)
    driver = ""
    try:
        driver = subprocess.run(
            ["nvidia-smi", "--query-gpu=driver_version", "--format=csv,noheader"],
            capture_output=True, text=True, timeout=30,
        ).stdout.strip().splitlines()[0]
    except (OSError, subprocess.SubprocessError, IndexError):
        pass
    return {
        "name": torch.cuda.get_device_name(0),
        "capability": list(torch.cuda.get_device_capability(0)),
        "memory_gib": round(props.total_memory / 2**30, 1),
        "count": torch.cuda.device_count(),
        "driver": driver,
        "cuda": torch.version.cuda,
        "cudnn": torch.backends.cudnn.version(),
    }


def version_of(package: str) -> str:
    from importlib import metadata

    try:
        return metadata.version(package)
    except metadata.PackageNotFoundError:
        return ""


def fingerprint() -> dict:
    """The hardware and software a prover or an auditor ran on: the CPU (model, kernel flags,
    oneDNN ISA), the GPU if any (model, compute capability, driver, CUDA, cuDNN), the engine,
    PyTorch and plugin versions, and the models' revisions. Taken in the process that runs the
    engine (or, for `vllm serve`, in the one that starts it with the same environment)."""
    return {
        "cpu": host(),
        "gpu": gpu(),
        "torch": version_of("torch"),
        "vllm": version_of("vllm"),
        "plugin": version_of("agentcoin-vllm"),
        "model": f'{os.environ.get("AC_VLLM_MODEL", "")}@{os.environ.get("AC_VLLM_REVISION", "")}',
        "cheat_model": f'{os.environ.get("AC_CHEAT_MODEL", "")}@{os.environ.get("AC_CHEAT_REVISION", "")}',
    }


def amx_in_use(cpu: dict) -> bool:
    """Whether oneDNN may pick AMX kernels: the CPU has AMX and `ONEDNN_MAX_CPU_ISA` does not
    keep oneDNN below it (as the plugin's own check reads it)."""
    isa = cpu.get("onednn_max_cpu_isa") or ""
    limited = bool(isa) and isa != "ALL" and "AMX" not in isa
    return "amx_bf16" in (cpu.get("flags") or []) and not limited


def cell_key(fp: dict | None) -> str:
    """A prover's or an auditor's side of a cell: the GPU model, or the CPU model with AMX on or
    off. The full fingerprint stays in the report."""
    fp = fp or {}
    if fp.get("gpu"):
        return fp["gpu"]["name"]
    cpu = fp.get("cpu") or {}
    return f'CPU {cpu.get("model") or "?"} AMX {"on" if amx_in_use(cpu) else "off"}'


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


def variant_of(case: str) -> str:
    return case.split("-", 1)[0]


def side(x: dict, band: tuple[int, int] | None = None) -> str:
    """Which side of the audit length band (the current version's unless given) a sample's prompt
    is on: "inside", "outside", or "unknown" for reports without prompt token counts."""
    band = band or CURRENT["band"]
    n = x.get("prompt_tokens")
    if n is None:
        return "unknown"
    return "inside" if band[0] <= n <= band[1] else "outside"


def cell_counts(samples: list[dict]) -> dict:
    """Honest outcomes and inexact prefill chunks, and per cheating variant its samples and how
    many passed (missed) or were inconclusive."""
    c = {"honest": {"samples": 0, "pass": 0, "fail": 0, "inconclusive": 0, "inexact_prefill": 0}, "cheats": {}}
    for x in samples:
        v = variant_of(x["case"])
        if v == "honest":
            h = c["honest"]
            h["samples"] += 1
            h[x["outcome"]] += 1
            if x["chunks"] and (x["chunks"][0]["mant_err_sum"] or x["chunks"][0]["exp_mismatches"]):
                h["inexact_prefill"] += 1
        elif v in CHEATS:
            o = c["cheats"].setdefault(v, {"samples": 0, "missed": 0, "inconclusive": 0})
            o["samples"] += 1
            o["missed"] += x["outcome"] == "pass"
            o["inconclusive"] += x["outcome"] == "inconclusive"
    return c


def cells(samples: list[dict]) -> dict:
    """Per "prover → auditor" cell: the counts of `cell_counts`, in all and per side of the audit
    length band (spec "报告按区间内外分开")."""
    groups: dict = {}
    for x in samples:
        groups.setdefault(f'{x["prover"]} → {x["auditor"]}', []).append(x)
    out = {}
    for key, xs in sorted(groups.items()):
        by_side: dict = {}
        for x in xs:
            by_side.setdefault(side(x), []).append(x)
        out[key] = {"prover": xs[0]["prover"], "auditor": xs[0]["auditor"], **cell_counts(xs),
                    "by_band": {k: cell_counts(v) for k, v in sorted(by_side.items())}}
    return out


# Thresholds as (exponent mismatches, mean mantissa error in hundredths, median mantissa error)
# per chunk kind, judged as `ac_market_proto::toploc::judge` does (the replay must reproduce the
# auditor's outcomes; scripts/tests checks the current version against AUDIT_THRESHOLDS). Version
# 3 bounds the prefill chunk by `prefill` when the prompt's tokens are in `band` (both ends
# included) and by `prefill_outside` otherwise (m6-toploc-gpu-calibration design D8); version 2
# had one prefill set, kept to replay older reports.
THRESHOLDS_V2 = {"version": 2, "band": None, "prefill": (2, 50, 1), "prefill_outside": (2, 50, 1),
                 "decode": (20, 800, 8)}
THRESHOLDS_V3 = {"version": 3, "band": (150, 300), "prefill": (6, 85, 1), "prefill_outside": (15, 500, 4),
                 "decode": (20, 800, 8)}
THRESHOLDS = {2: THRESHOLDS_V2, 3: THRESHOLDS_V3}
CURRENT = THRESHOLDS_V3
METRICS = {"exp": "exponent mismatches", "noexp": "no matching exponent",
           "mean": "mean mantissa error", "median": "median mantissa error"}


def judge_chunk(c: dict, bounds: tuple[int, int, int]) -> str | None:
    """The bound a chunk exceeds, or None."""
    exp, mean_centi, median = bounds
    if c["exp_mismatches"] > exp:
        return "exp"
    if c["mant_count"] == 0:
        return "noexp"
    if c["mant_err_sum"] * 100 > mean_centi * c["mant_count"]:
        return "mean"
    if c["median"] is None or c["median"] > median:
        return "median"
    return None


def judged(x: dict) -> bool:
    """Whether a sample's outcome came from the thresholds (a pass, or a fail on a chunk);
    other outcomes (no proof, commitment mismatch, inconclusive) do not depend on them."""
    return bool(x["chunks"]) and (x["outcome"] == "pass" or x["reason"].startswith("chunk "))


def in_band(x: dict, t: dict) -> bool:
    """Whether a sample's prompt is in the audit length band of `t` (always, without a band). A
    sample without its prompt token count cannot be placed: it is an error under a banded version."""
    if t.get("band") is None:
        return True
    n = x.get("prompt_tokens")
    if n is None:
        raise ValueError(f"{x['case']}: no prompt token count to judge it under version {t['version']}")
    return t["band"][0] <= n <= t["band"][1]


def replay(x: dict, t: dict) -> tuple[str, str]:
    """A sample's (outcome, reason) under thresholds `t`."""
    if not judged(x):
        return x["outcome"], x["reason"]
    prefill = t["prefill"] if in_band(x, t) else t["prefill_outside"]
    for i, c in enumerate(x["chunks"]):
        m = judge_chunk(c, prefill if i == 0 else t["decode"])
        if m:
            return "fail", f"chunk {i}: {METRICS[m]}"
    return "pass", ""


def parse_thresholds(specs: list[str]) -> dict:
    """`band=MIN,MAX prefill=E,M,D prefill_outside=E,M,D decode=E,M,D` (M in hundredths), the
    unnamed ones from the current version."""
    t = dict(CURRENT, version=None)
    for s in specs:
        kind, _, values = s.partition("=")
        parts = [int(v) for v in values.split(",")]
        size = 2 if kind == "band" else 3
        if kind not in ("band", "prefill", "prefill_outside", "decode") or len(parts) != size or min(parts) < 0:
            raise ValueError(f"bad thresholds {s!r}: use band=MIN,MAX or prefill|prefill_outside|decode=E,M,D")
        t[kind] = tuple(parts)
    return t


def minimal_thresholds(samples: list[dict], band: tuple[int, int] | None = None) -> dict | None:
    """The smallest thresholds (with the current band unless given) under which every honest
    sample judged by thresholds passes: per chunk kind the largest exponent mismatches, mean
    (hundredths, rounded up) and median of any honest chunk, the prefill chunk per side of the
    band (a sample without its prompt token count counts on both sides). None when an honest
    chunk has no matching exponent (no bound passes it)."""
    band = band or CURRENT["band"]
    worst = {"prefill": [0, 0, 0], "prefill_outside": [0, 0, 0], "decode": [0, 0, 0]}
    for x in samples:
        if variant_of(x["case"]) != "honest" or not judged(x):
            continue
        where = side(x, band)
        for i, c in enumerate(x["chunks"]):
            if c["mant_count"] == 0 or c["median"] is None:
                return None
            if i > 0:
                kinds = ["decode"]
            else:
                kinds = {"inside": ["prefill"], "outside": ["prefill_outside"]}.get(where, ["prefill", "prefill_outside"])
            for k in kinds:
                w = worst[k]
                w[0] = max(w[0], c["exp_mismatches"])
                w[1] = max(w[1], -(-c["mant_err_sum"] * 100 // c["mant_count"]))
                w[2] = max(w[2], c["median"])
    return {"version": None, "band": tuple(band), **{k: tuple(v) for k, v in worst.items()}}


LENGTH_BUCKETS = (0, 64, 128, 256)


def by_prompt_length(samples: list[dict]) -> dict:
    """Per cell and prompt length bucket (prompt tokens; samples without a count are left out):
    each variant's prefill chunk mean mantissa error (min, median, max) and exponent mismatches
    (max). On GPUs the honest prefill error depends on the prompt's length
    (calibration-results/gpu-quick-2026-10-07)."""
    groups: dict = {}
    for x in samples:
        n = x.get("prompt_tokens")
        if n is None or not x["chunks"]:
            continue
        low = max(b for b in LENGTH_BUCKETS if n >= b)
        bucket = f">={low}" if low == LENGTH_BUCKETS[-1] else f"{low}-{LENGTH_BUCKETS[LENGTH_BUCKETS.index(low) + 1] - 1}"
        c = x["chunks"][0]
        g = groups.setdefault(f'{x["prover"]} → {x["auditor"]}', {}).setdefault(bucket, {}).setdefault(
            variant_of(x["case"]), {"means": [], "exp": []})
        g["means"].append(mean(c))
        g["exp"].append(c["exp_mismatches"])
    out: dict = {}
    for cell, buckets in sorted(groups.items()):
        for bucket, variants in buckets.items():
            for v, g in variants.items():
                xs = sorted(g["means"])
                out.setdefault(cell, {}).setdefault(bucket, {})[v] = {
                    "samples": len(xs), "prefill_mean_min": round(xs[0], 3),
                    "prefill_mean_median": round(xs[len(xs) // 2], 3), "prefill_mean_max": round(xs[-1], 3),
                    "prefill_exp_max": max(g["exp"]),
                }
    return out


# Each cell needs this many honest samples inside and outside the band (spec "GPU 跨硬件校准").
MINIMUM_HONEST = {"inside": 3000, "outside": 500}


def under(samples: list[dict], t: dict) -> dict:
    """Per cell and side of `t`'s band (all inside without a band): honest samples and fails,
    and per cheating variant its samples and passes, when every sample is judged by `t`. A
    sample without its prompt token count (a report from before version 3) is "unknown" and
    judged by the bounds and rules of the band's inside, the strict ones."""
    out: dict = {}
    for x in samples:
        if t.get("band") is None:
            where, judge_by = "inside", t
        elif x.get("prompt_tokens") is None:
            where, judge_by = "unknown", dict(t, band=None)
        else:
            where, judge_by = ("inside" if in_band(x, t) else "outside"), t
        o = out.setdefault(f'{x["prover"]} → {x["auditor"]}', {}).setdefault(
            where, {"honest": 0, "honest_fail": 0, "cheats": {}, "missed": {}})
        outcome, _ = replay(x, judge_by)
        v = variant_of(x["case"])
        if v == "honest":
            o["honest"] += 1
            o["honest_fail"] += outcome == "fail"
        elif v in CHEATS:
            o["cheats"][v] = o["cheats"].get(v, 0) + 1
            o["missed"][v] = o["missed"].get(v, 0) + (outcome == "pass")
    return dict(sorted(out.items()))


def broken(r: dict) -> list[str]:
    """Where `under`'s result breaks the conclusion rules: inside the band no honest fail and no
    cheat missed; outside it no honest fail and no cheat but int8 missed (int8 is not required to
    fail there; its passes are only reported)."""
    out = []
    for cell, sides in r.items():
        for where, o in sides.items():
            if o["honest_fail"]:
                out.append(f'{cell} {where}: {o["honest_fail"]} honest fail')
            for v, n in o["missed"].items():
                if n and not (where == "outside" and v == "int8"):
                    out.append(f"{cell} {where}: {n} {v} missed")
    return out


def short_of_samples(r: dict) -> list[str]:
    """Cells with fewer honest samples on a side of the band than the calibration needs."""
    return [f'{cell} {where}: {sides.get(where, {}).get("honest", 0)} honest of {need}'
            for cell, sides in r.items() for where, need in MINIMUM_HONEST.items()
            if sides.get(where, {}).get("honest", 0) < need]


def conclusion(samples: list[dict], minimum: bool = True) -> dict:
    """The conclusion rules of spec engineering/ci-quality-gates "GPU 跨硬件校准", cell by cell:
    keep version 3's provisional values if they hold in every cell; else version 3 with every
    bound widened to the honest maximum (the band kept) if that still catches every cheat it must;
    else no thresholds, for the user to decide (design D12: version 3's values can change until
    the change is archived). With `minimum`, a cell short of honest samples on a side of the band
    leaves the conclusion open."""
    current = under(samples, CURRENT)
    low = minimal_thresholds(samples)
    found = {"minimal_thresholds": low}
    if low is not None:
        found["minimal_holds"] = not broken(under(samples, low))
    short = short_of_samples(current) if minimum else []
    if not broken(current):
        decision = {"decision": "keep", "thresholds": CURRENT, "cells": current}
    elif low is None:
        return {"decision": "no thresholds", "reason": "an honest chunk has no matching exponent",
                "broken": broken(current), "cells": current, **found}
    else:
        wide = {"version": CURRENT["version"], "band": CURRENT["band"],
                **{k: tuple(max(a, b) for a, b in zip(CURRENT[k], low[k]))
                   for k in ("prefill", "prefill_outside", "decode")}}
        widened = under(samples, wide)
        if broken(widened):
            return {"decision": "no thresholds", "thresholds": wide, "broken": broken(widened),
                    "cells": widened, "current": current, **found}
        decision = {"decision": "widen", "thresholds": wide, "cells": widened, "current": current}
    if short:
        return {**decision, "decision": "too few samples", "would_be": decision["decision"],
                "short": short, **found}
    return {**decision, **found}


class ReplayMismatch(Exception):
    pass


def check_replay(samples: list[dict]) -> None:
    """The replay under the version each sample was judged under must give it the auditor's own
    outcome and reason; anything else means the replay and `judge` disagree."""
    bad = []
    for x in samples:
        t = THRESHOLDS.get(x.get("thresholds_version"))
        try:
            if t and replay(x, t) != (x["outcome"], x["reason"]):
                bad.append(x["case"])
        except ValueError:
            bad.append(x["case"])
    if bad:
        raise ReplayMismatch(f"the replay disagrees with the auditor on {len(bad)} samples, e.g. {bad[:3]}")


def merge(files: list[Path], out: Path, thresholds: list[str] | None = None,
          summary_out: Path | None = None) -> int:
    """One report from shards: samples keep their shard's seed and the cell of the prover and
    the auditor (reports without fingerprints ran both on their `host`)."""
    shards = [json.loads(f.read_text()) for f in files]
    samples, fingerprints, seen = [], {}, set()
    for r in shards:
        prover = r.get("prover") or {"cpu": r.get("host")}
        auditor = r.get("auditor") or {"cpu": r.get("host")}
        p, a = cell_key(prover), cell_key(auditor)
        for key, fp in ((p, prover), (a, auditor)):
            if fp not in fingerprints.setdefault(key, []):
                fingerprints[key].append(fp)
        for x in r["samples"]:
            # The same case re-checked twice on the same kind of auditor counts once.
            ident = (r["seed"], x["case"], p, a)
            if ident in seen:
                print(f"a sample appears twice in cell {p} → {a}: seed {r['seed']} {x['case']}")
                return 1
            seen.add(ident)
            samples.append(dict(x, seed=r["seed"], host=auditor.get("cpu"), prover=p, auditor=a,
                                thresholds_version=x.get("thresholds_version", 2)))
    try:
        check_replay(samples)
    except ReplayMismatch as e:
        print(e)
        return 1
    s = summary(samples)
    report = {
        "seeds": sorted({r["seed"] for r in shards}),
        "honest": sum(variant_of(x["case"]) == "honest" for x in samples),
        "cheat": max([sum(variant_of(x["case"]) == v for x in samples) for v in CHEATS] or [0]),
        "thresholds_versions": sorted({v for r in shards for v in r["thresholds_versions"]}),
        "fingerprints": fingerprints,
        "cells": cells(samples),
        "summary": s,
        "honest_by_host": by_host(samples),
        "minimal_thresholds": minimal_thresholds(samples),
        "by_prompt_length": by_prompt_length(samples),
        "conclusion": conclusion(samples),
        "samples": samples,
    }
    if thresholds:
        t = parse_thresholds(thresholds)
        report["under_thresholds"] = {"thresholds": t, "cells": under(samples, t)}
    out.mkdir(parents=True, exist_ok=True)
    (out / "calibration.json").write_text(json.dumps(report, indent=1))
    if summary_out:
        summary_out.parent.mkdir(parents=True, exist_ok=True)
        summary_out.write_text(json.dumps(condensed(report), indent=1) + "\n")
    print(json.dumps({k: report[k] for k in ("seeds", "honest", "cheat", "thresholds_versions")}))
    print(json.dumps(report["cells"], indent=1, ensure_ascii=False))
    print(json.dumps({k: v for k, v in report["conclusion"].items() if k != "cells"}, ensure_ascii=False))
    if "under_thresholds" in report:
        print(json.dumps(report["under_thresholds"], indent=1, ensure_ascii=False))
    return 0


def condensed(report: dict) -> dict:
    """The report kept in the repository (design D6): fingerprints, cells, distributions,
    thresholds and conclusion, and the chunk metrics of every sample that did not go as it
    should (an honest one not passing, a cheating one not failing). Samples hold case names,
    outcomes and numbers only, never prompts or answers."""
    unexpected = [
        {k: x[k] for k in ("case", "seed", "prover", "auditor", "outcome", "reason", "prompt_tokens", "chunks") if k in x}
        for x in report["samples"]
        if (variant_of(x["case"]) == "honest") != (x["outcome"] == "pass")
    ]
    keep = ("seeds", "honest", "cheat", "thresholds_versions", "fingerprints", "cells", "summary", "by_prompt_length",
            "minimal_thresholds", "conclusion", "under_thresholds")
    return {**{k: report[k] for k in keep if k in report}, "unexpected": unexpected}


def sha256(path: Path) -> str:
    import hashlib

    return hashlib.sha256(path.read_bytes()).hexdigest()


def write_manifest(bundle: Path) -> None:
    lines = [f"{sha256(f)}  cases/{f.name}\n" for f in sorted((bundle / "cases").glob("*.json"))]
    (bundle / "MANIFEST.sha256").write_text("".join(lines))


def check_manifest(bundle: Path) -> dict:
    """The bundle against its manifest: listed cases that are missing, cases whose digest is not
    the listed one (re-checked anyway and flagged: the re-check, not the manifest, decides
    whether a case was changed), and cases the manifest does not list."""
    listed = {}
    for line in (bundle / "MANIFEST.sha256").read_text().splitlines():
        digest, _, name = line.partition("  ")
        listed[name.removeprefix("cases/")] = digest
    present = {f.name: f for f in (bundle / "cases").glob("*.json")}
    return {
        "missing": sorted(n for n in listed if n not in present),
        "mismatch": sorted(n for n, f in present.items() if n in listed and sha256(f) != listed[n]),
        "unlisted": sorted(n for n in present if n not in listed),
    }


def shard_of(names: list[str], shard: str | None) -> list[str]:
    """The cases of shard `K/N` (index ≡ K mod N in name order), or all of them."""
    names = sorted(names)
    if not shard:
        return names
    k, n = (int(v) for v in shard.split("/"))
    if not 0 <= k < n:
        raise ValueError(f"bad shard {shard!r}: use K/N with 0 <= K < N")
    return [x for i, x in enumerate(names) if i % n == k]


def generate_bundle(out: Path, honest: int, cheat: int, seed: int, min_words: int = 0) -> int:
    """Generates every variant into the bundle `out` (`cases/`, `prover.json`,
    `MANIFEST.sha256`; logs in `out/logs`)."""
    stale = stale_auditor()
    if stale:
        print(stale)
        return 1
    band = CURRENT["band"]
    cases = out / "cases"
    cases.mkdir(parents=True, exist_ok=True)
    for f in cases.glob("*.json"):
        f.unlink()
    logs = out / "logs"
    logs.mkdir(exist_ok=True)
    env = dict(os.environ)
    # A controlled experiment (m6-toploc-gpu-calibration design D4): the provider's engine with
    # another oneDNN ISA than the auditor's, e.g. AMX allowed.
    if os.environ.get("CALIBRATION_PROVER_ISA"):
        env["ONEDNN_MAX_CPU_ISA"] = os.environ["CALIBRATION_PROVER_ISA"]
    prints = []
    for v in VARIANTS:
        n = honest if v == "honest" else cheat
        fp = logs / f"fingerprint-{v}.json"
        with open(logs / f"generate-{v}.log", "w") as log:
            run = subprocess.run(
                [sys.executable, __file__, "--generate", v, "--count", str(n), "--seed", str(seed),
                 "--out", str(cases), "--fingerprint", str(fp), "--min-prompt-words", str(min_words),
                 "--band", f"{band[0]},{band[1]}"],
                stdout=log, stderr=subprocess.STDOUT, env=env,
            )
        lines = (logs / f"generate-{v}.log").read_text(errors="replace").strip().splitlines()
        if run.returncode != 0:
            # The log holds vLLM's messages, never the prompts (the marker check covers it).
            print(f"generating {v} failed; the end of its log:", *lines[-60:], sep="\n", flush=True)
            return 1
        print(*[l for l in lines if l.startswith(f"{v}: ")], sep="\n", flush=True)
        prints.append(json.loads(fp.read_text()))
    if any(p != prints[0] for p in prints):
        print("the variants ran on different hardware or software:", *map(json.dumps, prints), sep="\n")
        return 1
    lengths = {}
    for f in sorted(out.glob("prompt-tokens-*.json")):
        lengths.update(json.loads(f.read_text()))
        f.unlink()
    (out / "prompt_tokens.json").write_text(json.dumps(lengths, sort_keys=True) + "\n")
    unproven = {}
    for v in VARIANTS:
        f = out / f"unproven-{v}.json"
        if f.exists():
            unproven[v] = json.loads(f.read_text())
            f.unlink()
    (out / "prover.json").write_text(json.dumps(
        {"seed": seed, "honest": honest, "cheat": cheat, "min_prompt_words": min_words,
         "band": None if min_words else list(band),
         "unproven": unproven, "fingerprint": prints[0]}, indent=1) + "\n")
    write_manifest(out)
    print("prover:", json.dumps(prints[0]), flush=True)
    return 0


def recheck_bundle(bundle: Path, out: Path, shard: str | None = None) -> dict | None:
    """Re-checks the bundle's cases (or one shard of them) on this machine; the report, or None
    when cases listed in the manifest are missing."""
    prover = json.loads((bundle / "prover.json").read_text())
    stale = stale_auditor()
    if stale:
        print(stale)
        return None
    found = check_manifest(bundle)
    if found["missing"]:
        print(f"{len(found['missing'])} cases of the manifest are missing, e.g. {found['missing'][:3]}")
        return None
    for k in ("mismatch", "unlisted"):
        if found[k]:
            print(f"{len(found[k])} cases {k} (re-checked and flagged), e.g. {found[k][:3]}", flush=True)
    names = shard_of([f.name for f in (bundle / "cases").glob("*.json")], shard)
    logs = out / "logs"
    logs.mkdir(parents=True, exist_ok=True)
    picked = Path(tempfile.mkdtemp(prefix="ac-calibrate-cases-"))
    for n in names:
        (picked / n).symlink_to((bundle / "cases" / n).resolve())
    auditor = fingerprint()
    print("auditor:", json.dumps(auditor), flush=True)
    results = recheck(picked, logs)
    from_auditor = {r["case"] for r in results}
    missing = [n for n in names if n not in from_auditor]
    if missing:
        print(f"the auditor reported no result for {missing[:3]}")
        return None
    lengths = {}
    if (bundle / "prompt_tokens.json").exists():
        lengths = json.loads((bundle / "prompt_tokens.json").read_text())
    for r in results:
        r["manifest"] = next((k for k in ("mismatch", "unlisted") if r["case"] in found[k]), "ok")
        if r["case"] in lengths:
            r["prompt_tokens"] = lengths[r["case"]]
    return {
        "seed": prover["seed"], "host": auditor["cpu"], "prover": prover["fingerprint"], "auditor": auditor,
        "min_prompt_words": prover.get("min_prompt_words", 0),
        "unproven": prover.get("unproven", {}),
        "shard": shard or "0/1",
        "honest": sum(variant_of(n) == "honest" for n in names),
        "cheat": max([sum(variant_of(n) == v for n in names) for v in CHEATS] or [0]),
        "thresholds_versions": sorted({r["thresholds_version"] for r in results}),
        "manifest": {k: found[k] for k in ("mismatch", "unlisted")},
        "summary": summary(results), "samples": results,
    }


def same_outcomes(a: dict, b: dict) -> list[str]:
    """The cases whose outcome or reason differs between two reports of the same cases (empty
    when they agree): the split run against the one-machine run (m6-toploc-gpu-calibration 1.6)."""
    left = {x["case"]: (x["outcome"], x["reason"]) for x in a["samples"]}
    right = {x["case"]: (x["outcome"], x["reason"]) for x in b["samples"]}
    return sorted(c for c in left.keys() | right.keys() if left.get(c) != right.get(c))


def quick_failures(s: dict, unproven: dict, cheat: int, samples: list[dict] = ()) -> list[str]:
    """The CI regression's verdict on a report's summary and the prover's answers without proof.
    User decision (2026-10-01): an answer that cannot be re-tokenized is inconclusive, not a miss;
    at most one per variant is tolerated, while any honest failure or cheating pass fails the
    regression. An honest answer whose segments do not fit its usage (no proof at the provider,
    m6-toploc-async-stop) fails it too. An int8 answer to a prompt outside the audit length band
    (of `samples`) may pass: the rules do not require it to fail there (design D8)."""
    failures = []
    excused = sum(variant_of(x["case"]) == "int8" and x["outcome"] == "pass" and side(x) == "outside"
                  for x in samples)
    h = s.get("honest", {}).get("outcomes", {})
    if h.get("fail", 0) or h.get("inconclusive", 0) > 1:
        failures.append(f"honest samples: {h}")
    if unproven.get("honest"):
        failures.append(f"honest answers without proof at the prover: {unproven['honest']}")
    for v in CHEATS:
        o = s.get(v, {}).get("outcomes", {})
        passed = o.get("pass", 0) - (excused if v == "int8" else 0)
        if passed or o.get("inconclusive", 0) > 1:
            failures.append(f"{v} samples: {o} of {cheat}")
    return failures


def leaked(logs: Path) -> list[str]:
    return [f"{f.name} contains request content" for f in logs.iterdir()
            if f.is_file() and MARKER in f.read_text(errors="replace")]


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--quick", action="store_true", help="CI regression: 48 honest, 8 per cheat, assert")
    ap.add_argument("--honest", type=int)
    ap.add_argument("--cheat", type=int)
    ap.add_argument("--seed", type=int, default=1)
    ap.add_argument("--out", type=Path, default=Path("target/toploc-calibration"))
    ap.add_argument("--generate", choices=(*VARIANTS, "preempt"), help=argparse.SUPPRESS)
    ap.add_argument("--count", type=int, help=argparse.SUPPRESS)
    ap.add_argument("--fingerprint", type=Path, help=argparse.SUPPRESS)
    ap.add_argument("--band", help=argparse.SUPPRESS)
    ap.add_argument("--min-prompt-words", type=int, default=0,
                    help="lengthen every prompt to at least this many words (the prefill bound depends on it on GPUs)")
    ap.add_argument("--generate-only", action="store_true",
                    help="only generate: write a case bundle (cases, prover.json, MANIFEST.sha256) to --out")
    ap.add_argument("--recheck-only", type=Path, metavar="BUNDLE",
                    help="only re-check the cases of BUNDLE on this machine; report to --out")
    ap.add_argument("--shard", metavar="K/N", help="with --recheck-only: re-check every N-th case from the K-th")
    ap.add_argument("--merge", type=Path, nargs="+", metavar="JSON",
                    help="merge the calibration.json files of shards into --out, by cell")
    ap.add_argument("--thresholds", nargs="+", metavar="KIND=E,M,D",
                    help="with --merge: also judge every sample by these (prefill=E,M,D decode=E,M,D; M in hundredths)")
    ap.add_argument("--summary-out", type=Path, metavar="FILE",
                    help="with --merge: write the condensed report kept in the repository")
    ap.add_argument("--compare", type=Path, nargs=2, metavar="JSON",
                    help="fail unless two reports of the same cases have the same outcomes")
    args = ap.parse_args()
    if args.compare:
        a, b = (json.loads(f.read_text()) for f in args.compare)
        differ = same_outcomes(a, b)
        print(f"{len(a['samples'])} cases; differ: {differ[:10]}" if differ else f"{len(a['samples'])} cases, same outcomes")
        return 1 if differ else 0
    if args.generate:
        if args.fingerprint:
            # Before the engine starts: a GPU driver failure should not cost the fingerprint.
            args.fingerprint.write_text(json.dumps(fingerprint()))
        band = tuple(int(v) for v in args.band.split(",")) if args.band else None
        generate(args.generate, args.count, args.seed, args.out, args.min_prompt_words, band)
        return 0
    if args.merge:
        return merge(args.merge, args.out.resolve(), args.thresholds, args.summary_out)

    out = args.out.resolve()
    honest = args.honest or (48 if args.quick else 3000)
    cheat = args.cheat or (8 if args.quick else 100)
    if args.recheck_only:
        report = recheck_bundle(args.recheck_only.resolve(), out, args.shard)
        if report is None:
            return 1
        (out / "calibration.json").write_text(json.dumps(report, indent=1))
        print(json.dumps(report["summary"], indent=1))
        failures = leaked(out / "logs")
        print("FAILED:" if failures else "re-check done", *failures, sep="\n  ")
        return 1 if failures else 0

    print("host:", json.dumps(host()), flush=True)
    if generate_bundle(out, honest, cheat, args.seed, args.min_prompt_words):
        return 1
    if args.generate_only:
        failures = leaked(out / "logs")
        print("FAILED:" if failures else "bundle done", *failures, sep="\n  ")
        return 1 if failures else 0
    report = recheck_bundle(out, out)
    if report is None:
        return 1
    s = report["summary"]
    (out / "calibration.json").write_text(json.dumps(report, indent=1))
    print(json.dumps(s, indent=1))

    failures = leaked(out / "logs")
    if args.quick:
        failures += quick_failures(s, report.get("unproven", {}), cheat, report["samples"])
    print("FAILED:" if failures else "calibration done", *failures, sep="\n  ")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
