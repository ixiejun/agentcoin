#!/usr/bin/env python3
"""Does the engine feed an answer's end token back before it knows the answer ended?
(m6-toploc-gpu-calibration, the long-prompt experiment: on GPUs the honest answers that end with
`stop` re-check with a last chunk one position longer than the auditor recomputes.)

The suspected cause is vLLM's asynchronous scheduling, on by default on GPUs: the next step is
scheduled before the previous step's token is known, so a request whose last token was its end
token gets one more forward pass, and the plugin sends that row too. The provider expects one
decode segment per output token but the last (the end token is never fed back), and so does the
auditor's re-check.

The probe answers the same prompts (short answers that end with `stop`, and a few that end on
the length limit) in prove mode with the plugin, as `scripts/calibrate-toploc.py` generates,
once with the engine's default scheduling and once with asynchronous scheduling off, each in its
own process, and counts every request's decode segments against its output tokens − 1.

Usage: scripts/probe-stop-boundary.py   (after scripts/gpu-calibration.sh setup, or
scripts/setup-vllm-cpu.sh; env AC_VLLM_MODEL, AC_VLLM_REVISION)
Prints one line per mode and finish reason: answers, and how many have the expected number of
decode segments, one more, or another count; then the conclusion. Exit status 0 whatever the
outcome (a measurement, not a check), 1 if a mode could not run.
"""

from __future__ import annotations

import importlib.util
import json
import os
import subprocess
import sys
import tempfile
import time
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
MODES = ("default", "async-off")

# Questions with short answers (they end with `stop`), and open ones cut by the length limit.
SHORT = [
    "Reply with the single word yes.",
    "What is 2 + 3? Answer with the number only.",
    "Name the capital of France in one word.",
    "Say hello.",
    "Give one color of the sky, one word.",
    "Is water wet? Answer yes or no.",
    "Write the word lighthouse and nothing else.",
    "Count from one to three in words.",
    "What is the opposite of cold? One word.",
    "Answer with a single emoji-free word: what animal says moo?",
    "Write a haiku about tides.",
    "In one short sentence, what is a compiler?",
]
LONG = [
    "Explain in detail how bread rises.",
    "Write a long story about a lighthouse keeper.",
    "Describe the history of the printing press at length.",
    "List twenty facts about glaciers.",
]


def calibrate():
    spec = importlib.util.spec_from_file_location("calibrate", REPO / "scripts" / "calibrate-toploc.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def child(mode: str) -> None:
    """Generates in prove mode with the plugin, as the calibration does; prints one JSON line."""
    cal = calibrate()
    workdir = tempfile.mkdtemp(prefix="ac-probe-stop-")
    sock = os.path.join(workdir, "toploc.sock")
    provider = cal.FakeProvider(sock)
    os.environ["AGENTCOIN_TOPLOC_SOCKET"] = sock
    os.environ["AGENTCOIN_TOPLOC_MODE"] = "prove"
    os.environ["VLLM_ENABLE_V1_MULTIPROCESSING"] = "0"
    os.environ["VLLM_USE_V2_MODEL_RUNNER"] = "0"
    from vllm import LLM, SamplingParams

    extra = {"async_scheduling": False} if mode == "async-off" else {}
    llm = LLM(
        model=os.environ["AC_VLLM_MODEL"],
        revision=os.environ["AC_VLLM_REVISION"],
        dtype="bfloat16",
        enable_prefix_caching=False,
        max_model_len=1024,
        max_num_batched_tokens=256,
        seed=7,
        gpu_memory_utilization=cal.MEMORY,
        **extra,
    )
    resolved = None
    try:
        resolved = llm.llm_engine.vllm_config.scheduler_config.async_scheduling
    except AttributeError:
        pass
    deadline = time.time() + 120
    while not provider.connected and time.time() < deadline:
        time.sleep(0.2)
    if not provider.connected:
        sys.exit("the plugin did not connect")
    prompts = [([{"role": "user", "content": p}], 96) for p in SHORT] + [([{"role": "user", "content": p}], 40) for p in LONG]
    # Twice over, all at once, so that the engine batches them as a provider's would.
    prompts = prompts * 2
    params = [SamplingParams(temperature=0.7, top_p=0.95, max_tokens=m) for _, m in prompts]
    outputs = llm.chat([m for m, _ in prompts], params, use_tqdm=False)
    time.sleep(2)  # the sender thread drains its queue
    rows = []
    for o in outputs:
        keys = [k for k in provider.segments if k == o.request_id or k.startswith(o.request_id + "-")]
        if len(keys) != 1:
            rows.append({"finish": o.outputs[0].finish_reason, "tokens": len(o.outputs[0].token_ids), "decode": None})
            continue
        segs = provider.segments[keys[0]]
        rows.append({
            "finish": o.outputs[0].finish_reason,
            "tokens": len(o.outputs[0].token_ids),
            "decode": sum(1 for s in segs if s["phase"] == "decode"),
        })
    print("RESULT " + json.dumps({"mode": mode, "async_scheduling": resolved, "rows": rows}), flush=True)


def main() -> int:
    if len(sys.argv) == 3 and sys.argv[1] == "--child":
        child(sys.argv[2])
        return 0
    results = {}
    logs = Path(tempfile.mkdtemp(prefix="ac-probe-stop-logs-"))
    for mode in MODES:
        log = logs / f"{mode}.log"
        with open(log, "w") as f:
            run = subprocess.run([sys.executable, __file__, "--child", mode], stdout=f, stderr=subprocess.STDOUT)
        text = log.read_text(errors="replace")
        line = next((l for l in text.splitlines() if l.startswith("RESULT ")), None)
        if run.returncode != 0 or line is None:
            print(f"{mode}: did not run; the end of its log ({log}):", *text.splitlines()[-30:], sep="\n")
            return 1
        results[mode] = json.loads(line.removeprefix("RESULT "))
    extra_rows = {}
    for mode, r in results.items():
        print(f"{mode}: async scheduling resolved to {r['async_scheduling']}")
        for finish in ("stop", "length"):
            rs = [x for x in r["rows"] if x["finish"] == finish]
            if not rs:
                continue
            expected = sum(x["decode"] == x["tokens"] - 1 for x in rs)
            one_more = sum(x["decode"] == x["tokens"] for x in rs)
            other = len(rs) - expected - one_more
            extra_rows[(mode, finish)] = one_more
            print(f"  {finish:6}: {len(rs):2} answers, {expected:2} with output tokens - 1 decode segments, "
                  f"{one_more:2} with one more, {other:2} other")
    stop_default = extra_rows.get(("default", "stop"), 0)
    stop_off = extra_rows.get(("async-off", "stop"), 0)
    if stop_default and not stop_off:
        print("conclusion: with the default scheduling, answers that end with stop get one decode segment "
              "too many, and not with asynchronous scheduling off: the end token is fed back")
    elif not stop_default and not stop_off:
        print("conclusion: no extra decode segment in either mode: the cause is elsewhere")
    else:
        print("conclusion: extra decode segments with asynchronous scheduling off too: the cause is not (only) it")
    print(f"logs: {logs}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
