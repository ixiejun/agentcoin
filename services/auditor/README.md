> 🌐 **English** | [简体中文](README.zh-CN.md)

# ac-auditor

The AgentCoin auditor agent (M6, MVP plan §5.5; spec `market/auditor-agent`). An auditor
re-checks inferences **it requested itself**, as an ordinary user, so no user's content ever
enters an audit. This version re-checks one finished inference at a time; mystery-shopper
requests, sampling and verdicts on chain come with later M6 changes.

A re-check:

1. checks that the double-signed receipt is the answer's (model, prompt and output token
   counts); otherwise the result is **inconclusive (input mismatch)**;
2. re-checks only models registered as **bfloat16**; any other precision is **inconclusive
   (precision not supported)** and never a failure;
3. **fails (no proof)** on an all-zero TOPLOC commitment: an audited request without proofs fails
   whatever the reason; proofs that do not match the commitment, or whose parameters or count
   are not the market's, fail too;
4. re-creates the tokens with its own engine: the prompt from the messages under the model's chat
   template (`/tokenize`), the output from its text (`/tokenize`, then `/detokenize` must give the
   same text). The prompt count must equal the receipt's, and the output count must be one less
   than the receipt's when the answer stopped on the end token (`stop`; the end token is not in
   the text) and equal to it when it hit the length limit (`length`); otherwise the result is
   **inconclusive (tokens cannot be re-created)**;
5. prefills the prompt and every output token but the last (the last one is never fed back)
   through `/v1/completions` with a random `X-Request-Id`; the engine's TOPLOC plugin, in the
   **verify** mode, sends one candidate segment per token row to the auditor's socket;
6. rebuilds the original inference's chunks from the rows (the prompt rows are the prefill, every
   later row a decode step), compares them with the proofs (`ac_toploc::compare_from_candidates`)
   and judges them by the market's versioned thresholds (`ac_market_proto::toploc::judge`):
   **pass**, or **fail** on the first chunk out of bounds. Engine failures and missing rows are
   **inconclusive (re-check engine error)**.

## Usage

```bash
# The re-check engine: vLLM with the plugin in verify mode, same model, bfloat16. On CPUs,
# oneDNN is kept from AMX kernels (the thresholds are calibrated that way; the plugin enforces
# it on CPUs with AMX).
ONEDNN_MAX_CPU_ISA=AVX512_CORE_BF16 \
AGENTCOIN_TOPLOC_SOCKET=/run/agentcoin/recheck.sock AGENTCOIN_TOPLOC_MODE=verify \
VLLM_USE_V2_MODEL_RUNNER=0 vllm serve Qwen/Qwen2.5-0.5B-Instruct \
  --served-model-name qwen --dtype bfloat16 --no-enable-prefix-caching --port 8000

# Re-check one case, or every *.json of a directory; one JSON line per case.
ac-auditor recheck --engine http://127.0.0.1:8000 --socket /run/agentcoin/recheck.sock \
  --node http://127.0.0.1:9944 --cases cases/
```

| Flag | Meaning |
|---|---|
| `--case <file>` / `--cases <dir>` | one case, or every `*.json` of a directory in name order |
| `--engine <url>` | the re-check engine |
| `--engine-model <name>` | the engine's model name, instead of each case's `engine_model` |
| `--socket <path>` | the socket the engine's plugin sends to (created owner-only) |
| `--node <url>` / `--quant <p>` | read each model's registered precision from a node, or give it (`bf16`, `fp16`, `fp8`, `int8`, `int4`) |
| `--connect-wait <s>` | how long to wait for the plugin to connect (default 120) |
| `--log-level <l>` | log level; lines never contain request content |

A case (`RecheckCase`) is JSON:

```json
{
  "model": "0x<on-chain model id>",
  "engine_model": "qwen",
  "messages": [{"role": "user", "content": "..."}],
  "output": "the answer's text",
  "finish_reason": "length",
  "usage": {"prompt_tokens": 40, "completion_tokens": 24},
  "receipt": "<SCALE SignedReceipt, hex>",
  "toploc": "<SCALE ToplocProofs, hex, or null>"
}
```

Each output line has `case`, `outcome` (`pass`, `fail`, `inconclusive`), `reason`, `request`
(the receipt's request ID), `thresholds_version` and `chunks` (every chunk's exponent mismatches,
mantissa error sum and count, and median).

`ac-auditor calibration-case` turns a prover engine's candidates (the same JSON with
`segments: [{phase, len, candidates: [[index, bits], …]}]` or `null` instead of `receipt` and
`toploc`) into a case whose receipt is signed by a key made for the run. It serves the
calibration of the thresholds (`scripts/calibrate-toploc.py`) and tests, not audits.

### Evidence (audits on chain)

A failing verdict on chain commits to the evidence of the re-check (m6-audit-chain; spec
`market/auditor-agent` "审计证据"). `ac-auditor evidence --case c.json --out e.bin` writes the
evidence (SCALE bytes: messages, answer, usage, receipt, proofs) and prints
`{"commitment": "<hex>"}`. A reviewer of a dispute re-checks it with
`ac-auditor recheck --evidence e.bin --commitment <hex from the chain> --engine-model <name> …`:
if the bytes do not match the commitment the output line is `{"outcome": "mismatch", …}` and no
engine is involved; otherwise the evidence is re-checked like its case. Every report line also
carries `onchain`: the outcome as the chain encodes it (hex), which `ac-wallet audit verdict`
submits.

### The agent (`ac-auditor run`)

The service mode is the mystery shopper of MVP plan §5.5 (m6-auditor-agent; spec
`market/auditor-agent` "审计员代理服务" and after). It runs as a registered auditor account
(`ac-wallet audit register`) and, with no one at the keyboard:

- **audits** each provider it is assigned in a round once, at a random block of the first three
  quarters of the round: one non-streamed request pinned to that provider
  (`X-AgentCoin-Provider`, open to every user), through a random gateway with a random payment
  account, with a prompt from the built-in generator or the operator's bank, for a model of the
  provider it has an engine for. It re-checks the answer and submits the verdict before the
  round ends. A failure's evidence is stored before the verdict is sent.
- **serves evidence** at its endpoint (registered on chain at start-up with its X-Wing key). It
  hands over a verdict's evidence, over a sealed channel, only to a reviewer of an open dispute
  this auditor accused in. A refusal says nothing more. Evidence is deleted once no dispute can
  use it, and expired disputes it is party to are closed.
- **reviews** the disputes it is drawn for. It fetches every accuser's evidence, checks it
  against the commitment and the receipt on chain, and re-checks it. It votes *confirm* only
  when two distinct accusers' evidence fails, otherwise *reject*. If its own engine is down it
  retries and, failing that, does not vote.

```bash
ac-auditor keygen --out auditor-kem.json --password-file auditor.pass
ac-auditor run --config auditor.json
```

The configuration is a JSON file; relative paths are taken from its directory:

```json
{
  "node": "http://127.0.0.1:9944",
  "wallet": "auditor.json", "password_file": "auditor.pass", "kem_key": "auditor-kem.json",
  "listen": "0.0.0.0:8500", "public_endpoint": "https://auditor.example:8500",
  "data_dir": "auditor-data",
  "engines": [{"model": "0x…", "engine": "http://127.0.0.1:8000",
               "engine_model": "qwen", "socket": "/run/agentcoin/recheck.sock"}],
  "payers": [{"wallet": "payer1.json", "password_file": "payer1.pass",
              "gateways": ["atc1…"], "max_usd": "5"}],
  "prompts": {"bank": "bank.jsonl", "bank_percent": 20, "min_tokens": 32, "max_tokens": 256},
  "margin_percent": 25
}
```

Operating notes:

- **Payment accounts.** Each must already have a credit channel (escrow) at each listed gateway.
  It must never be the auditor account: the start refuses that. Gateways see payment accounts,
  and escrow deposits are public, so fund them from sources unrelated to the auditor and rotate
  them. Anonymous vouchers (M7) replace this.
- **Engines.** One verify-mode engine per model (see above, AMX off on CPUs). Providers serving
  only models without an engine are skipped and counted.
- **Thresholds.** The agent judges by its built-in `AUDIT_THRESHOLDS` version. When the chain
  accepts another version, it neither audits nor votes and says why: upgrade it.
- **Bank.** JSONL, one `messages` array per line; `bank_percent` of prompts come from it.

## Privacy

Re-checks only use the auditor's own requests. The agent logs request IDs, model IDs, counts,
metrics and verdicts; never messages, outputs, tokens or candidates, and lines from libraries
are dropped. A case's `Debug` output leaves out the messages and the output.

## Tests

`cargo test -p ac-auditor` re-checks answers of the deterministic mock engine
(`tests/mock-engine`): an honest provider, another model, no proofs, an int4 model, mismatched
proofs, inputs and tokens, a missing engine and a prove-mode plugin on the auditor's socket. The
CI job `vllm-plugin` re-checks real vLLM answers (`scripts/calibrate-toploc.py --quick`). The
agent's decisions (`src/agent`) are tested with stand-ins for the chain, gateways, engines and
accusers; `tests/e2e/tests/auditor_agent.rs` runs six agents against a provider that starts
cheating (`AC_E2E=1 cargo test -p ac-e2e --test auditor_agent -- --test-threads 1`).
