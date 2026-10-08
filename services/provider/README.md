> 🌐 **English** | [简体中文](README.zh-CN.md)

# ac-provider

The AgentCoin inference provider agent (M5, spec `market/provider-agent`). It sits in front of
any OpenAI-compatible inference engine (vLLM, SGLang, a llama.cpp server, ...) and:

- accepts sealed requests (`ac_crypto::sealed`, X-Wing + ML-DSA) only from gateways that are
  registered and active on chain, for models the provider registered and maps to the engine;
- forwards them to the engine with streaming and usage reporting, and streams every chunk back
  sealed while it is generated;
- prices the request from the engine's token counts and the on-chain price (rounded up to a
  micro-dollar), signs the receipt with the provider account's ML-DSA key and returns it with the
  request's TOPLOC proofs (below);
- keeps the receipts the gateway co-signs, with their proofs, until their challenge period is
  over;
- sends a heartbeat every three quarters of the heartbeat interval (always free) and claims
  matured payments once per emission epoch;
- **never logs or stores prompts or outputs**: its log lines carry request IDs, model IDs, token
  counts, fees, latencies and error codes only, and lines from libraries underneath are dropped.

## Usage

```bash
# 1. The encryption key, then register it with the wallet.
ac-provider keygen --out kem.json --password-file pw.txt
ac-wallet market provider register --wallet provider.json --tier t2 \
  --endpoint https://provider.example --kem-key 0x01… --model <model id>:0.1:0.2 …

# 2. Run next to the engine (e.g. `vllm serve Qwen/Qwen2.5-0.5B-Instruct --port 8000`).
ac-provider run --wallet provider.json --password-file pw.txt --kem-key kem.json \
  --node http://127.0.0.1:9944 --engine http://127.0.0.1:8000 \
  --model 0x<model id>=Qwen/Qwen2.5-0.5B-Instruct --listen 0.0.0.0:8421 --data-dir /var/lib/ac-provider \
  --toploc-socket /run/agentcoin/toploc.sock
```

At start the agent refuses to run if the key file's public key is not the one registered on
chain or a mapped model is not registered. The registered endpoint must reach `--listen`;
terminate TLS in a reverse proxy if you like, although request content is already sealed.

## TOPLOC proofs

With `--toploc-socket <path>` the agent listens on that local Unix socket (mode 0600) for the
engine's TOPLOC plugin (`plugins/vllm`, spec `market/engine-plugin`; start vLLM with
`AGENTCOIN_TOPLOC_SOCKET` set to the same path). It forwards each request with `X-Request-Id` set
to the market request ID, collects the plugin's per-step top-k candidates for it (in memory
only), and when the engine has answered waits up to 2 s for the plugin's end marker. It then
checks the candidates against the engine's usage (the prefill is the prompt tokens times the
hidden size; one decode step per output token but the last), builds the proofs with `ac-toploc`
(top-k 128, decode batches of 32) and puts their commitment in the receipt; the proofs travel to
the gateway with the receipt and are stored with the co-signed receipt. The plugin must run in
its prove mode (the default; plugin protocol version 2): a plugin in the verify mode, which is
for auditors' re-checks, is turned away.

A request the engine preempts and recomputes is proved too (m6-public-jobs, I-008): the plugin
sends the recomputation as a prefill of the prompt followed by one decode segment per generated
row, and a prefill segment that arrives after decode segments starts the request's candidates
over, so the proofs are those of a request that was never preempted. Requests asking for several
choices (`n > 1`) never reach the provider: the gateway refuses them.

An engine may compute one step more than the answer needs: vLLM schedules asynchronously on GPUs
by default, and feeds an answer's end token back before it knows the answer ended
(m6-toploc-async-stop, I-022). The plugin then sends one decode segment per output token. The
provider drops that last segment, the end token's row, which no proof covers (an auditor never
recomputes it), so the proofs and the commitment are those of an engine that did not compute it.
Any other count (two segments more, or too few) gives no proof. The rule is shared with the
calibration (`ac_market_proto::toploc::fit_segments`), so that its cases are the provider's proofs.

Without the socket, when the plugin sends no end marker in time, or when the candidates do not
fit, the receipt carries an all-zero commitment and no proofs, and a `toploc_missing` line is
logged with the request ID prefix and the reason. Receipts without proofs stay valid for users,
but they earn no market work (the gateway reports their dollars as unproven), an auditor's
re-check fails a request without proofs (spec `market/auditor-agent`), and providers cannot tell
audit requests from others: run the plugin.

## Operation

- **Clock**: handshakes are valid for ±120 s, so run NTP.
- **Endpoints**: `POST /ac/v1/sealed` (gateways), `GET /health`.
- **Data directory**: `receipts/` holds co-signed receipts and their TOPLOC proofs (SCALE, hex, one
  JSON file per request ID), deleted three emission epochs after their challenge period.
- **Engine**: must support `stream: true` with `stream_options.include_usage` (vLLM, SGLang and
  llama.cpp do); a stream that ends without usage is treated as a failure and not billed.

## Feature flags

None.
