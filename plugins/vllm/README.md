> 🌐 **English** | [简体中文](README.zh-CN.md)

# agentcoin-vllm

The AgentCoin TOPLOC plugin for [vLLM](https://github.com/vllm-project/vllm) (spec
`market/engine-plugin`, MVP plan §2.1, decision D31). It runs inside the vLLM engine and hands the
top-k final-layer activations of every request to the local AgentCoin provider agent
(`ac-provider run --toploc-socket …`), which builds the TOPLOC proofs that receipts commit to.
The plugin holds no proof logic: proofs are built in Rust by `ac-toploc`.

Licence: `MIT OR Apache-2.0` (permissive zone, D47).

## How it works

- vLLM loads the plugin through the `vllm.general_plugins` entry point in every process it
  starts. Without `AGENTCOIN_TOPLOC_SOCKET` the plugin does nothing at all.
- When the model loads, the plugin checks its preconditions and makes the engine fail to start
  if one is unmet (below).
- After each forward step it reads the output of the model's final norm (the activations the
  TOPLOC reference implementation reads) from vLLM's V1 model runner, splits it by request, and
  takes on the device the top `k` values by magnitude of each request's part (a *segment*: the
  prompt tokens computed in this step, or one decoded token). Indices and bfloat16 bits are
  copied to the host asynchronously; a background thread sends them to the provider over the
  local Unix socket, with an end marker when a request finishes. The inference thread never
  waits for I/O: while the provider is unreachable, or when the queue is full, segments are
  dropped (and counted), and the plugin reconnects every second.
- The protocol is `crates/ac-market-proto/src/engine.rs` (version 2). The provider tells the
  plugin `k` (128 in the market) when it connects; a provider speaking another version stops the
  plugin from sending.
- **Modes.** `AGENTCOIN_TOPLOC_MODE` selects what the candidates are for: `prove` (the default,
  for a provider, as above) or `verify` (for an auditor's re-check, `ac-auditor recheck`). In the
  verify mode the engine only prefills "prompt + output" and the plugin sends one segment per
  prefilled token row (the row's top `k`), in row order, and nothing for decode steps; the
  auditor rebuilds the original inference's chunks from the rows. The plugin declares its mode
  when it connects: providers take only `prove`, auditors only `verify`, and a refused mode
  stops the plugin (logged as a mode mismatch). Other values make the engine fail to start.
- **AMX.** The re-check thresholds are calibrated with oneDNN kept from AMX kernels: on Intel
  CPUs with AMX, AMX kernels made honest prefill chunks inexact. In verify mode on such a CPU the
  engine fails to start unless `ONEDNN_MAX_CPU_ISA` is set below AMX (`AVX512_CORE_BF16`).
- The provider forwards requests with `X-Request-Id` set to the market request ID, so that
  vLLM's request ID (`chatcmpl-<X-Request-Id>-<suffix>`) identifies each segment.

## Requirements

| | |
|---|---|
| vLLM | `>=0.30.0,<0.31` (checked when the plugin loads) |
| Model runner | V1. On GPUs vLLM 0.30 defaults to the V2 runner: set `VLLM_USE_V2_MODEL_RUNNER=0`. The CPU backend uses V1 |
| Precision | `--dtype bfloat16` (TOPLOC works only with bfloat16) |
| Prefix caching | off: `--no-enable-prefix-caching` (cached prompt tokens are never recomputed) |
| Speculative decoding | off |
| Parallelism | tensor parallelism is fine (rank 0 sends); pipeline parallelism is refused |

A request the engine preempts and recomputes is proved (m6-public-jobs, I-008): in the step
that recomputes it, the rows inside the prompt form one prefill segment and every row after the
prompt is a decode segment of its own, so the provider can rebuild the candidates of a request
that was never preempted. Requests that ask for several choices (`n > 1`) are refused by the
gateway (issues I-009 to I-011 in `docs/issues.md` track what is left for later).

## Install and run

```bash
pip install ./plugins/vllm          # next to vLLM 0.30.x
export AGENTCOIN_TOPLOC_SOCKET=/run/agentcoin/toploc.sock   # the provider's --toploc-socket
VLLM_USE_V2_MODEL_RUNNER=0 vllm serve Qwen/Qwen2.5-0.5B-Instruct \
  --dtype bfloat16 --no-enable-prefix-caching
```

Start the provider with the same socket path (`ac-provider run … --toploc-socket
/run/agentcoin/toploc.sock`); either may start first. An auditor's re-check engine runs the same
way with `AGENTCOIN_TOPLOC_MODE=verify` and the socket of `ac-auditor recheck --socket`.

## Privacy

The plugin sends only indices and activation values, only to the local socket. It writes no
files and logs only its connection state, request IDs and counts; never prompts, outputs, tokens
or activations.

## Tests

```bash
pip install pytest torch   # the extraction and runner tests need PyTorch
python -m pytest plugins/vllm
```

`tests/test_client.py` checks the encoding against the Rust vectors
(`crates/ac-market-proto/tests/vectors/engine_protocol.json`) and the sender against a fake
provider; `tests/test_extract.py` and `tests/test_runner.py` check the extraction and the
wrappers with stand-in vLLM objects. `scripts/check-vllm-plugin.py` runs the plugin in a real CPU
vLLM and compares its proofs with the reference implementation (CI job `vllm-plugin`).
