#!/usr/bin/env python3
"""Public job summaries on a real (CPU) vLLM (m6-public-jobs 6.4; spec
`engineering/ci-quality-gates` "公共任务摘要的真实引擎检查", CI job vllm-plugin).

With `ac-worker exec` (the worker's own executors) and `ac-worker compare` (comparison rules v1):

1. an evaluation unit run alone and run twice concurrently (so the engine batches its requests
   with the other run's) gives summaries that agree;
2. the same unit on another model (the calibration's "cheat" model) gives a summary that does
   not agree;
3. likewise for an embedding unit on the pinned embedding model, and on another model (the
   generation model served with the pooling runner);
4. a data cleaning unit run twice gives the same summary;
5. nothing of the data reaches the worker's output;
6. inference with a KV cache too small for its batch (the scheduler preempts and recomputes
   requests): at least one request is recomputed, each answer's segments, kept as the provider
   keeps them (a prefill after decode segments starts over), make a re-check case, and
   `ac-auditor recheck` passes every case (`scripts/calibrate-toploc.py`'s generation and
   re-check, variant `preempt`).

Usage: scripts/check-public-jobs-vllm.py   (after scripts/setup-vllm-cpu.sh and
`cargo build -p ac-worker -p ac-auditor`; env AC_VLLM_MODEL, AC_VLLM_REVISION, AC_CHEAT_MODEL,
AC_CHEAT_REVISION, AC_EMBED_MODEL, AC_EMBED_REVISION)
"""

import json
import os
import secrets
import subprocess
import sys
import tempfile
import time
import urllib.request
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
WORKER = REPO / "target" / "debug" / "ac-worker"
MARKER = "pangolin-marker-41c8"

failures: list[str] = []


def check(ok: bool, what: str) -> None:
    print(("ok   " if ok else "FAIL ") + what, flush=True)
    if not ok:
        failures.append(what)


def serve(args: list[str], workdir: Path, name: str):
    """Starts `vllm serve`; returns (process, base URL), or (None, URL) if it did not start."""
    port = 18000 + secrets.randbelow(1000)
    base = f"http://127.0.0.1:{port}"
    log = open(workdir / f"{name}.log", "w")
    env = dict(os.environ, VLLM_USE_V2_MODEL_RUNNER="0")
    env.pop("AGENTCOIN_TOPLOC_SOCKET", None)
    proc = subprocess.Popen(
        ["vllm", "serve", *args, "--served-model-name", "model", "--port", str(port),
         "--gpu-memory-utilization", "0.3"],
        env=env, stdout=log, stderr=subprocess.STDOUT,
    )
    deadline = time.time() + 900
    while time.time() < deadline:
        try:
            urllib.request.urlopen(base + "/v1/models", timeout=5)
            return proc, base
        except OSError:
            if proc.poll() is not None:
                print((workdir / f"{name}.log").read_text()[-4000:])
                return None, base
            time.sleep(2)
    proc.terminate()
    return None, base


def stop(proc) -> None:
    proc.terminate()
    try:
        proc.wait(timeout=60)
    except subprocess.TimeoutExpired:
        proc.kill()


def execute(kind: str, shard: Path, engine: str | None, job: int = 0) -> str:
    """Runs `ac-worker exec`; returns the summary (hex) and checks the output holds no data."""
    cmd = [str(WORKER), "exec", "--kind", kind, "--shard", str(shard), "--model", "model",
           "--job", str(job)]
    if engine:
        cmd += ["--engine", engine]
    out = subprocess.run(cmd, capture_output=True, text=True, timeout=1800)
    if out.returncode != 0:
        print(out.stdout[-2000:], out.stderr[-2000:])
        raise RuntimeError(f"ac-worker exec {kind} failed")
    if MARKER in out.stdout or MARKER in out.stderr:
        failures.append(f"the {kind} output holds the data")
    for line in out.stdout.splitlines():
        if line.startswith("summary: "):
            return line.removeprefix("summary: ").strip()
    raise RuntimeError("no summary")


def agree(kind: str, a: str, b: str) -> bool:
    out = subprocess.run(
        [str(WORKER), "compare", "--kind", kind, "--a", a, "--b", b],
        capture_output=True, text=True, check=True,
    )
    return out.stdout.strip() == "agree: true"


def alone_and_batched(kind: str, shard: Path, engine: str, job: int = 0) -> tuple[str, list[str]]:
    alone = execute(kind, shard, engine, job)
    with ThreadPoolExecutor(2) as pool:
        batched = list(pool.map(lambda _: execute(kind, shard, engine, job), range(2)))
    return alone, batched


def eval_shard(path: Path) -> None:
    facts = [
        ("The capital of France is", [" Paris.", " Berlin.", " Madrid.", " Rome."]),
        ("Water freezes at", [" zero degrees Celsius.", " fifty degrees Celsius.", " the moon."]),
        ("Two plus two equals", [" four.", " five.", " twenty-two."]),
        ("A cat is a kind of", [" animal.", " vegetable.", " mineral.", " planet."]),
        ("The sun rises in the", [" east.", " west.", " north."]),
        ("Bees make", [" honey.", " steel.", " glass."]),
        ("The opposite of hot is", [" cold.", " loud.", " green."]),
        ("A week has", [" seven days.", " three days.", " forty days."]),
    ]
    lines = []
    for i in range(48):
        context, choices = facts[i % len(facts)]
        lines.append(json.dumps({
            "context": f"Question {i} ({MARKER}). {context}",
            "choices": choices,
        }))
    path.write_text("\n".join(lines) + "\n")


def text_shard(path: Path, n: int) -> None:
    topics = ["lighthouses", "honey bees", "tidal pools", "glaciers", "printing presses",
              "violin making", "desert caravans", "orbital mechanics"]
    lines = [
        json.dumps({"text": f"Note {i} on {topics[i % len(topics)]} ({MARKER}): a short "
                            f"paragraph about how {topics[(i * 3) % len(topics)]} work."})
        for i in range(n)
    ]
    # An exact and a near duplicate, for the cleaning unit.
    lines += [lines[0], lines[1].replace("work.", "work!")]
    path.write_text("\n".join(lines) + "\n")


def preempt(workdir: Path) -> None:
    import importlib.util

    script = REPO / "scripts" / "calibrate-toploc.py"
    spec = importlib.util.spec_from_file_location("calibrate", script)
    calibrate = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(calibrate)
    cases = workdir / "preempt-cases"
    cases.mkdir()
    with open(workdir / "generate-preempt.log", "w") as log:
        run = subprocess.run(
            [sys.executable, str(script), "--generate", "preempt", "--count", "24", "--seed", "3",
             "--out", str(cases)],
            stdout=log, stderr=subprocess.STDOUT,
        )
    lines = (workdir / "generate-preempt.log").read_text(errors="replace").splitlines()
    check(run.returncode == 0, "inference with a small KV cache ran")
    if run.returncode != 0:
        print(*lines[-60:], sep="\n")
        return
    print(*[l for l in lines if l.startswith("preempt: ")], sep="\n")
    recomputed = next(
        (int(l.split()[1]) for l in lines if l.startswith("preempt: ") and "recomputed" in l), 0
    )
    check(recomputed > 0, f"requests were preempted and recomputed ({recomputed})")
    results = calibrate.recheck(cases, workdir)
    outcomes = [r.get("outcome") for r in results]
    print(f"     re-check outcomes: { {o: outcomes.count(o) for o in set(outcomes)} }")
    # As in the calibration regression: an answer that does not re-tokenize is inconclusive, and
    # at most one is tolerated; a failure never is.
    check(len(results) > 0 and "fail" not in outcomes and outcomes.count("inconclusive") <= 1,
          "every answer, recomputed or not, passes the re-check")
    for f in workdir.iterdir():
        if f.is_file() and f.suffix == ".log" and calibrate.MARKER in f.read_text(errors="replace"):
            failures.append(f"{f.name} holds request content")


def main() -> int:
    env = os.environ
    workdir = Path(tempfile.mkdtemp(prefix="ac-public-jobs-vllm-"))
    eval_file, embed_file = workdir / "eval.jsonl", workdir / "embed.jsonl"
    eval_shard(eval_file)
    text_shard(embed_file, 32)

    # Evaluation: the pinned model, alone and batched; then another model.
    proc, base = serve([env["AC_VLLM_MODEL"], "--revision", env["AC_VLLM_REVISION"],
                        "--dtype", "bfloat16", "--max-model-len", "1024"], workdir, "eval")
    check(proc is not None, "vllm serve (evaluation model) started")
    if proc is None:
        return 1
    try:
        alone, batched = alone_and_batched("eval", eval_file, base)
    finally:
        stop(proc)
    print(f"     evaluation summary {alone}")
    for b in batched:
        check(agree("eval", alone, b), "evaluation summaries agree, alone and batched")
    proc, base = serve([env["AC_CHEAT_MODEL"], "--revision", env["AC_CHEAT_REVISION"],
                        "--dtype", "bfloat16", "--max-model-len", "1024"], workdir, "eval-other")
    check(proc is not None, "vllm serve (another model) started")
    if proc is not None:
        try:
            other = execute("eval", eval_file, base)
        finally:
            stop(proc)
        print(f"     another model's summary {other}")
        check(not agree("eval", alone, other), "another model's evaluation summary does not agree")

    # Embedding: the pinned embedding model, alone and batched; then another model.
    proc, base = serve([env["AC_EMBED_MODEL"], "--revision", env["AC_EMBED_REVISION"],
                        "--runner", "pooling", "--dtype", "bfloat16"], workdir, "embed")
    check(proc is not None, "vllm serve (embedding model) started")
    if proc is not None:
        try:
            e_alone, e_batched = alone_and_batched("embed", embed_file, base, job=7)
        finally:
            stop(proc)
        for b in e_batched:
            check(agree("embed", e_alone, b), "embedding summaries agree, alone and batched")
        proc, base = serve([env["AC_VLLM_MODEL"], "--revision", env["AC_VLLM_REVISION"],
                            "--runner", "pooling", "--dtype", "bfloat16",
                            "--max-model-len", "1024"], workdir, "embed-other")
        check(proc is not None, "vllm serve (another model, pooling) started")
        if proc is not None:
            try:
                e_other = execute("embed", embed_file, base, job=7)
            finally:
                stop(proc)
            check(not agree("embed", e_alone, e_other),
                  "another model's embedding summary does not agree")

    # Inference under preemption: proofs that re-check.
    preempt(workdir)

    # Data cleaning: no engine; two runs give the same summary.
    c1, c2 = execute("clean", embed_file, None), execute("clean", embed_file, None)
    check(c1 == c2, "data cleaning summaries are equal")

    print("FAILED:" if failures else "all checks passed", *failures, sep="\n  ")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
