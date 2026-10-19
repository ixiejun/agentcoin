#!/usr/bin/env python3
"""Probe the vLLM interfaces the auditor's re-check relies on (m6-toploc-verify 1.1, CI job
vllm-plugin).

Starts `vllm serve` (CPU, bfloat16, prefix caching off, the plugin sending to a fake provider
socket) and checks, for a few chat requests:

1. `/tokenize` with `messages` and `add_generation_prompt` gives as many tokens as the chat
   completion's `usage.prompt_tokens`;
2. `/tokenize` with `prompt` and `add_special_tokens: false` tokenizes an output text and
   `/detokenize` turns the tokens back into the same text (reported: how often the count fits
   the completion's `completion_tokens` for its finish reason);
3. `/v1/completions` accepts a list of token IDs as the prompt and counts them as prompt tokens;
4. the engine-internal request ID of a completion sent with `X-Request-Id` contains that ID (the
   plugin's segments carry it);
5. a chat completion with `return_token_ids`, streamed and not, gives the prompt's token IDs
   (top level; the first chunk when streamed) and the output token IDs (the choice; each chunk's
   delta when streamed), as many as `usage` counts, and the prompt IDs are those of `/tokenize`
   (m6-token-binding 1.1: the provider commits to these IDs in the receipt).

Interface failures (1, 3, 4, 5, an error status or a missing field) fail the script; the
round-trip statistics of 2 are printed (re-tokenizing is no longer used by the re-check, which
takes the committed IDs; how often the generated IDs differ from re-tokenizing the text is
printed too, issue I-023).

Usage: scripts/probe-vllm-recheck.py   (after scripts/setup-vllm-cpu.sh; env AC_VLLM_MODEL,
AC_VLLM_REVISION)
"""

import json
import os
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

MODEL = os.environ.get("AC_VLLM_MODEL", "")
REVISION = os.environ.get("AC_VLLM_REVISION", "")
NAME = "probe-model"
TOPK = 128
PROMPTS = [
    "Name three rivers in Europe and one sentence about each.",
    "Write a haiku about a lighthouse.",
    "Explain what a hash function is to a ten-year-old.",
    "List the first ten prime numbers, separated by commas.",
    "Translate 'good morning, how are you?' into French and Spanish.",
    "Give a short recipe for pancakes.",
    "What is 17 times 23? Show the steps.",
    "Describe the colour blue without using the word 'blue'.",
]

failures: list[str] = []


def check(ok: bool, what: str) -> None:
    print(("ok   " if ok else "FAIL ") + what, flush=True)
    if not ok:
        failures.append(what)


class FakeProvider:
    """Answers Hello with Welcome and records the request IDs of Segment frames."""

    def __init__(self, path: str) -> None:
        self.requests: set[str] = set()
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
                    # Answer with the version the plugin sent.
                    conn.sendall(struct.pack(">IBBH", 4, 0x11, p[5], TOPK))
                    greeted = True
                    continue
                (idlen,) = struct.unpack(">H", p[1:3])
                self.requests.add(p[3 : 3 + idlen].decode())


def token_ids_of(chunks: list[dict]) -> tuple[list[int] | None, list[int], str, dict | None]:
    """The prompt token IDs (first chunk), the output token IDs (each choice's delta, in order),
    the streamed text and the usage of a streamed chat completion's chunks."""
    prompt = None
    output: list[int] = []
    text = ""
    usage = None
    for c in chunks:
        if prompt is None and c.get("prompt_token_ids") is not None:
            prompt = list(c["prompt_token_ids"])
        for choice in c.get("choices") or []:
            output.extend(choice.get("token_ids") or [])
            text += (choice.get("delta") or {}).get("content") or ""
        if c.get("usage"):
            usage = c["usage"]
    return prompt, output, text, usage


def token_ids_of_response(resp: dict) -> tuple[list[int] | None, list[int] | None, str, dict | None]:
    """The prompt token IDs, the output token IDs, the text and the usage of a non-streamed chat
    completion (one choice)."""
    prompt = resp.get("prompt_token_ids")
    choices = resp.get("choices") or [{}]
    choice = choices[0]
    output = choice.get("token_ids")
    text = (choice.get("message") or {}).get("content") or ""
    return (None if prompt is None else list(prompt), None if output is None else list(output), text,
            resp.get("usage"))


def stream(base: str, body: dict) -> list[dict]:
    """The JSON chunks of a streamed request (SSE `data:` lines up to `[DONE]`)."""
    req = urllib.request.Request(
        base + "/v1/chat/completions",
        data=json.dumps(body).encode(),
        headers={"Content-Type": "application/json"},
    )
    chunks = []
    with urllib.request.urlopen(req, timeout=300) as r:
        for raw in r:
            line = raw.decode().strip()
            if not line.startswith("data:"):
                continue
            data = line[len("data:"):].strip()
            if data == "[DONE]":
                break
            chunks.append(json.loads(data))
    return chunks


def post(base: str, path: str, body: dict, headers: dict | None = None) -> dict:
    req = urllib.request.Request(
        base + path,
        data=json.dumps(body).encode(),
        headers={"Content-Type": "application/json", **(headers or {})},
    )
    with urllib.request.urlopen(req, timeout=300) as r:
        return json.load(r)


def main() -> int:
    if not MODEL or not REVISION:
        print("set AC_VLLM_MODEL and AC_VLLM_REVISION (scripts/setup-vllm-cpu.sh)")
        return 1
    workdir = tempfile.mkdtemp(prefix="ac-vllm-probe-")
    sock = os.path.join(workdir, "toploc.sock")
    provider = FakeProvider(sock)
    port = 18000 + secrets.randbelow(1000)
    base = f"http://127.0.0.1:{port}"
    env = dict(os.environ, AGENTCOIN_TOPLOC_SOCKET=sock, VLLM_USE_V2_MODEL_RUNNER="0")
    log = open(Path(workdir) / "vllm.log", "w")
    engine = subprocess.Popen(
        [
            "vllm", "serve", MODEL, "--revision", REVISION, "--served-model-name", NAME,
            "--dtype", "bfloat16", "--no-enable-prefix-caching", "--max-model-len", "1024",
            "--gpu-memory-utilization", "0.3", "--port", str(port),
        ],
        env=env, stdout=log, stderr=subprocess.STDOUT,
    )
    try:
        deadline = time.time() + 900
        while time.time() < deadline:
            try:
                urllib.request.urlopen(base + "/v1/models", timeout=5)
                break
            except OSError:
                if engine.poll() is not None:
                    print(Path(workdir, "vllm.log").read_text()[-4000:])
                    check(False, "vllm serve started")
                    return 1
                time.sleep(2)
        else:
            check(False, "vllm serve started within 15 minutes")
            return 1

        fits = round_trips = differ = 0
        for i, text in enumerate(PROMPTS):
            messages = [{"role": "user", "content": text}]
            chat = post(base, "/v1/chat/completions", {
                "model": NAME, "messages": messages, "temperature": 0,
                "max_tokens": 24 if i % 2 else 200,
            })
            usage, choice = chat["usage"], chat["choices"][0]
            output, finish = choice["message"]["content"], choice["finish_reason"]

            tok = post(base, "/tokenize", {"model": NAME, "messages": messages, "add_generation_prompt": True})
            check(len(tok["tokens"]) == usage["prompt_tokens"],
                  f"prompt {i}: /tokenize(messages) {len(tok['tokens'])} = prompt_tokens {usage['prompt_tokens']}")

            out = post(base, "/tokenize", {"model": NAME, "prompt": output, "add_special_tokens": False})
            back = post(base, "/detokenize", {"model": NAME, "tokens": out["tokens"]})
            same = back["prompt"] == output
            want = usage["completion_tokens"] - (1 if finish == "stop" else 0)
            round_trips += same
            fits += same and len(out["tokens"]) == want
            print(f"     prompt {i}: finish {finish}, completion_tokens {usage['completion_tokens']}, "
                  f"re-tokenized {len(out['tokens'])}, round trip {'same' if same else 'differs'}")

            request_id = secrets.token_hex(32)
            ids = tok["tokens"] + out["tokens"][: max(want, 0)]
            comp = post(base, "/v1/completions", {"model": NAME, "prompt": ids, "max_tokens": 1, "temperature": 0},
                        {"X-Request-Id": request_id})
            check(comp["usage"]["prompt_tokens"] == len(ids),
                  f"prompt {i}: /v1/completions with token IDs counts {comp['usage']['prompt_tokens']} = {len(ids)}")
            time.sleep(1)  # the plugin's sender thread
            check(any(request_id in r for r in provider.requests),
                  f"prompt {i}: the engine-internal request ID contains X-Request-Id")

            # 5. The token IDs a provider commits to (m6-token-binding).
            chunks = stream(base, {
                "model": NAME, "messages": messages, "temperature": 0.7, "top_p": 0.95,
                "max_tokens": 24 if i % 2 else 200, "stream": True,
                "stream_options": {"include_usage": True}, "return_token_ids": True,
            })
            prompt_ids, output_ids, text, s_usage = token_ids_of(chunks)
            check(prompt_ids is not None and s_usage is not None,
                  f"prompt {i}: the stream has prompt_token_ids and usage")
            if prompt_ids is None or s_usage is None:
                continue
            check(len(prompt_ids) == s_usage["prompt_tokens"] and prompt_ids == tok["tokens"],
                  f"prompt {i}: {len(prompt_ids)} prompt IDs = prompt_tokens {s_usage['prompt_tokens']} "
                  "and = /tokenize(messages)")
            check(len(output_ids) == s_usage["completion_tokens"],
                  f"prompt {i}: {len(output_ids)} output IDs = completion_tokens {s_usage['completion_tokens']}")
            again = post(base, "/tokenize", {"model": NAME, "prompt": text, "add_special_tokens": False})
            differ += again["tokens"] != output_ids[: len(again["tokens"])] or len(again["tokens"]) > len(output_ids)

            whole = post(base, "/v1/chat/completions", {
                "model": NAME, "messages": messages, "temperature": 0.7, "top_p": 0.95,
                "max_tokens": 24 if i % 2 else 200, "return_token_ids": True,
            })
            prompt_ids, output_ids, _, w_usage = token_ids_of_response(whole)
            check(prompt_ids is not None and output_ids is not None and w_usage is not None,
                  f"prompt {i}: the response has prompt_token_ids, the choice's token_ids and usage")
            if prompt_ids is None or output_ids is None or w_usage is None:
                continue
            check(prompt_ids == tok["tokens"] and len(prompt_ids) == w_usage["prompt_tokens"],
                  f"prompt {i}: non-streamed prompt IDs = /tokenize(messages) = prompt_tokens")
            check(len(output_ids) == w_usage["completion_tokens"],
                  f"prompt {i}: non-streamed {len(output_ids)} output IDs = completion_tokens "
                  f"{w_usage['completion_tokens']}")

        print(f"re-tokenizing: {round_trips}/{len(PROMPTS)} round trips exact, "
              f"{fits}/{len(PROMPTS)} with the expected token count; "
              f"{differ}/{len(PROMPTS)} sampled answers re-tokenize to other IDs than generated")
        print("internal request IDs seen:", sorted(provider.requests)[:4], "...")
    finally:
        engine.terminate()
        engine.wait(timeout=60)
        log.close()

    print("FAILED:" if failures else "all checks passed", *failures, sep="\n  ")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
