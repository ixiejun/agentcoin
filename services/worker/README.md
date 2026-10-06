> 🌐 **English** | [简体中文](README.zh-CN.md)

# ac-worker

The AgentCoin public job worker and the publisher's tools (MVP plan §5.6; OpenSpec change
`m6-public-jobs`; spec `market/public-worker`). A worker runs the units the chain
(`pallet-public-jobs`) assigns it with a local inference engine: it checks the data against the
manifest's hashes, executes the unit with the fixed rules of its kind, commits to the summary,
reveals it after the commit deadline, uploads the full result to the publisher and claims its
rewards. Logs carry jobs, units, counts and durations, never data or results.

## Commands

| Command | Who | What |
|---|---|---|
| `ac-worker run --config worker.json` | worker | the worker service |
| `ac-worker collect --config collect.json` | publisher | the result collector; reveals canaries |
| `ac-worker canary --job N --kind K --manifest m.json --units 0,7,… --out c.json [--engine URL --model NAME]` | publisher | builds a job's canaries and prints the root |
| `ac-worker exec --kind K --shard s.jsonl [--engine URL --model NAME --job N --out r]` | anyone | runs one shard, prints summary and result hash |
| `ac-worker compare --kind K --a HEX --b HEX` | anyone | applies comparison rules v1 to two summaries |

### The worker service

```json
{
  "node": "http://127.0.0.1:9944",
  "wallet": "worker.json", "password_file": "worker.pass",
  "data_dir": "worker-data",
  "engines": [{"model": "0x…", "engine": "http://127.0.0.1:8000", "engine_model": "qwen"}],
  "poll_ms": 1000
}
```

The account must be registered first (`ac-wallet public worker register --model 0x…`); at start
the service updates the registered models to the configured ones. Every tick it declares itself
ready once per round, executes newly assigned units in the background (one at a time per
engine), commits before the deadline, reveals after it, uploads the result of every unit that
passed with it in the majority (retrying until the chain prunes the unit), claims settled epochs
and withdraws unlocked rewards. Transactions are sent without waiting for inclusion (the nonce
counts those still in the pool) and the effect is read back from the chain. A unit whose data
does not match its hashes, or whose engine fails, is not committed: a miss is better than a
summary that was not computed. Executed units (summary, result, salt) are kept in
`data_dir/units` until the chain prunes them, so a restarted worker can still reveal and upload.

### The collector

```json
{"node": "http://127.0.0.1:9944", "listen": "0.0.0.0:8600", "dir": "results",
 "wallet": "publisher.json", "password_file": "publisher.pass",
 "canaries": ["job-3-canaries.json"]}
```

Workers `PUT /<job>/<unit>` with their address in `X-AgentCoin-Worker`. A result is saved to
`dir/<job>/<unit>` only when the unit passed, the uploader is in its majority, the body's BLAKE3
is the result hash it revealed, and nothing is saved yet; otherwise the answer is 403 or 409 with
no explanation. With canary files (and a wallet to sign with) the collector reveals each canary
once its unit passed.

## Execution rules (version 1)

- **Evaluation.** Each item has a context and 2–16 continuations. A continuation's score is the
  sum of its tokens' log probabilities after the context (`/v1/completions` with the prompt's
  token IDs, `max_tokens: 1`, `prompt_logprobs: 0`; nothing is sampled), in thousandths of a nat
  rounded to the nearest. The answer is the highest score, the lower index among equals; the
  context's tokens must be a prefix of context + continuation, or the unit is not committed. The
  summary is the answers; the result holds every score.
- **Embedding.** Each text's vector (`/v1/embeddings`) is L2-normalized; bit j of its 32-bit
  fingerprint is whether its dot product with direction j is positive. Dimension d of direction
  j is +1 or −1 as bit d mod 256 (least significant bit of each byte first) of the 32-byte block
  d / 256 of `derive("agentcoin 2026-10 public-direction v1", job ‖ j ‖ block)` is set or not.
  The summary is the fingerprints; the result holds the normalized vectors.
- **Data cleaning** (no engine; byte-for-byte reproducible): Unicode NFC (`unicode-normalization`
  0.1.25, Unicode 17.0), control characters removed except line feed and tab, runs of whitespace
  in a line collapsed and lines trimmed, documents under 32 characters dropped, exact duplicates
  dropped, near duplicates dropped (MinHash over lower-cased character 5-grams, 128
  permutations from a fixed seed, 16 bands of 8 rows, confirmed at 103 equal minima, an
  estimated Jaccard similarity of 0.8); the first of duplicates is kept. The result is the kept
  documents as JSON Lines `{"index", "text"}`; the summary is its BLAKE3. A change to any of this,
  or to the Unicode tables, is a new rules version: `tests/vectors/public_clean_v1.json` pins
  the bytes.

## Publishing a job

1. **Data.** Split the work into units and serve each shard (JSON Lines: evaluation
   `{"context": …, "choices": […]}`, texts `{"text": …}`) and a manifest
   `{"units": [{"url": …, "blake3": hex, "items": n}, …]}`, one entry per unit in unit order.
2. **Canaries.** Pick canary units at random and keep the choice secret: `ac-worker canary`
   computes their expected summaries with your own engine (the job number is the one the job
   will get, `ac-wallet public jobs` shows the latest) and prints the root. A colluding majority
   is caught only on canary units, so the share sets how fast: with a canary share `c`, a
   majority that forges `n` units escapes with probability about `(1 − c)^n`; 5–10% is a
   reasonable default. Keep the canary file private until each unit passed.
3. **Collector.** Run `ac-worker collect` at the results URL with the canary file.
4. **Publish.** `ac-wallet public publish --kind … --manifest m.json --manifest-url … --results-url
   … --units N --price 0.05 [--model 0x…] [--canary-root 0x…]` checks the manifest and the price
   and prints the call; the administration proposes it as a motion.

## Example

Data cleaning is reproducible, and its summary is the result's hash:

```rust
use ac_worker::exec::clean_unit;

let docs = vec![
    "A document long enough to be kept by the cleaning rules.".to_string(),
    "  A document long enough to be kept by the cleaning rules.  ".to_string(),
    "too short".to_string(),
];
let out = clean_unit(&docs).unwrap();
assert_eq!(out.summary, out.result_hash.to_vec());
assert_eq!(out, clean_unit(&docs).unwrap());
assert_eq!(String::from_utf8(out.result).unwrap().lines().count(), 1);
```

## Features

None.
