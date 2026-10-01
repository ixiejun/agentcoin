#!/usr/bin/env python3
"""Drive an OpenAI-compatible endpoint with the official SDK (m5-gateway-provider 3.2).

Runs N streamed and one non-streamed chat completion, measures the time to the first content
chunk, and prints one JSON object with the latency statistics, the texts and the reported usage.
Used by tests/e2e/tests/inference.rs, once against the mock engine directly and once through the
wallet's local proxy.

Usage: openai_client.py --base-url http://127.0.0.1:8411/v1 --model NAME [--runs 30] [--marker S]
       openai_client.py --base-url … --model NAME --provider ADDRESS
       (one non-streamed request pinned to a provider with the X-AgentCoin-Provider header,
       m6-auditor-agent 3.3; prints the answer's usage or the error's status and code)
"""

import argparse
import json
import statistics
import sys
import time

import openai
from openai import OpenAI


def percentile(values, q):
    ordered = sorted(values)
    if not ordered:
        return None
    # Nearest-rank percentile.
    k = max(0, min(len(ordered) - 1, int(round(q / 100 * len(ordered) + 0.5)) - 1))
    return ordered[k]


def main():
    p = argparse.ArgumentParser()
    p.add_argument("--base-url", required=True)
    p.add_argument("--model", required=True)
    p.add_argument("--runs", type=int, default=30)
    p.add_argument("--marker", default="marker")
    p.add_argument("--max-tokens", type=int, default=8)
    p.add_argument("--provider")
    a = p.parse_args()

    # The local proxy needs no key; the SDK insists on one.
    client = OpenAI(base_url=a.base_url, api_key="unused", max_retries=0, timeout=60)
    messages = [{"role": "user", "content": f"say {a.marker} please"}]

    if a.provider:
        try:
            full = client.chat.completions.create(
                model=a.model,
                messages=messages,
                max_tokens=a.max_tokens,
                extra_headers={"X-AgentCoin-Provider": a.provider},
            )
        except openai.APIStatusError as e:
            # The SDK unwraps `{"error": {...}}`; accept either shape.
            body = e.body if isinstance(e.body, dict) else {}
            code = body.get("code") or (body.get("error") or {}).get("code")
            print(json.dumps({"error": {"status": e.status_code, "code": code}}))
            return 0
        print(json.dumps({"usage": {"prompt_tokens": full.usage.prompt_tokens, "completion_tokens": full.usage.completion_tokens}}))
        return 0

    ttfts, streamed = [], []
    for _ in range(a.runs):
        start = time.perf_counter()
        first, text, usage = None, "", None
        stream = client.chat.completions.create(
            model=a.model,
            messages=messages,
            max_tokens=a.max_tokens,
            stream=True,
            stream_options={"include_usage": True},
        )
        for chunk in stream:
            if chunk.choices and chunk.choices[0].delta.content:
                if first is None:
                    first = time.perf_counter() - start
                text += chunk.choices[0].delta.content
            if chunk.usage is not None:
                usage = {"prompt_tokens": chunk.usage.prompt_tokens, "completion_tokens": chunk.usage.completion_tokens}
        if first is None:
            print(json.dumps({"error": "no content streamed"}))
            return 1
        ttfts.append(first * 1000)
        streamed.append({"text": text, "usage": usage})

    full = client.chat.completions.create(model=a.model, messages=messages, max_tokens=a.max_tokens)
    models = [m.id for m in client.models.list().data]
    print(json.dumps({
        "ttft_ms": {"p50": statistics.median(ttfts), "p95": percentile(ttfts, 95), "all": ttfts},
        "streamed": streamed,
        "completion": {
            "text": full.choices[0].message.content,
            "usage": {"prompt_tokens": full.usage.prompt_tokens, "completion_tokens": full.usage.completion_tokens},
        },
        "models": models,
    }))
    return 0


if __name__ == "__main__":
    sys.exit(main())
