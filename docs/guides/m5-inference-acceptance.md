> 🌐 **English** | [简体中文](m5-inference-acceptance.zh-CN.md)

# M5 inference acceptance with a real model

The CI end-to-end test (`tests/e2e/tests/inference.rs`) runs the whole market against a mock
engine. This guide repeats it by hand with a real model on a GPU machine, the M5 acceptance of the
MVP plan (§10): the OpenAI SDK streams Qwen through the market, with at most 150 ms of extra time to
first token, and the receipts settle.

## Prerequisites

- A Linux machine with an NVIDIA GPU (8 GB is enough for Qwen2.5-0.5B-Instruct), Python 3.10+.
- `vllm` installed (`pip install vllm`); any OpenAI-compatible engine that supports
  `stream_options.include_usage` works the same way.
- The repository built: `cargo build --release -p ac-node -p ac-wallet -p ac-provider -p ac-gateway`.
- `pip install -r tests/e2e/python/requirements.txt` (the official `openai` package).

## Steps

```bash
# 1. A development chain.
target/release/ac-node --dev --tmp &

# 2. The engine.
vllm serve Qwen/Qwen2.5-0.5B-Instruct --port 8000 &

# 3. Wallets: the development account funds a provider, a gateway and the user.
target/release/ac-wallet import --wallet user.json --password-file pw.txt   # dev mnemonic
target/release/ac-wallet new --wallet provider.json --password-file pw.txt
target/release/ac-wallet new --wallet gateway.json --password-file pw.txt
target/release/ac-wallet transfer --wallet user.json --password-file pw.txt --to <provider address> --amount 1000
target/release/ac-wallet transfer --wallet user.json --password-file pw.txt --to <gateway address> --amount 5000

# 4. The model (a manifest with the real shard hashes) and the provider.
target/release/ac-wallet market model register --wallet user.json --password-file pw.txt --file qwen.json
target/release/ac-provider keygen --out provider-kem.json --password-file pw.txt
target/release/ac-wallet market provider register --wallet provider.json --password-file pw.txt \
  --tier t2 --endpoint http://127.0.0.1:8421 --kem-key <printed kem key> --model <model id>:0.1:0.2
target/release/ac-provider run --wallet provider.json --password-file pw.txt --kem-key provider-kem.json \
  --engine http://127.0.0.1:8000 --model <model id>=Qwen/Qwen2.5-0.5B-Instruct --listen 127.0.0.1:8421 &

# 5. The gateway.
target/release/ac-gateway keygen --out gateway-kem.json --password-file pw.txt
target/release/ac-wallet market gateway register --wallet gateway.json --password-file pw.txt \
  --endpoint http://127.0.0.1:8431 --fee-bps 300
target/release/ac-gateway run --wallet gateway.json --password-file pw.txt --kem-key gateway-kem.json \
  --listen 127.0.0.1:8431 --report-interval 5 &

# 6. The user's escrow and local proxy.
target/release/ac-wallet market escrow deposit --wallet user.json --password-file pw.txt --gateway <gateway address> --amount 10
target/release/ac-wallet market serve --wallet user.json --password-file pw.txt --gateway <gateway address> --max-usd 1 &
```

## Checks

1. **Streaming through the OpenAI SDK, and the latency.** Run the same client directly against
   the engine and through the proxy, and compare the p95 time to first token:

   ```bash
   python3 tests/e2e/python/openai_client.py --base-url http://127.0.0.1:8000/v1 --model Qwen/Qwen2.5-0.5B-Instruct --runs 30
   python3 tests/e2e/python/openai_client.py --base-url http://127.0.0.1:8411/v1 --model Qwen2.5-0.5B-Instruct --runs 30
   ```

   The difference of the two `ttft_ms.p95` values must be at most 150 ms. Record both values,
   the GPU and the build profile (release).
2. **Settlement.** After a few blocks `ac-wallet market report show --id 0` shows the gateway's
   report; after the challenge period (two emission epochs) `ac-wallet market work --account
   <provider address>` shows claimed payments, because the provider agent claims by itself.
3. **Privacy.** The logs of `ac-provider`, `ac-gateway` and `ac-wallet market serve` contain no
   prompt or output text, also with `--log-level debug`.

Receipts carry an all-zero TOPLOC commitment in this phase; engine-side proofs come with the next
change.
