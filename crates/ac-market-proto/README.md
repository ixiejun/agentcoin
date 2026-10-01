> 🌐 **English** | [简体中文](README.zh-CN.md)

# ac-market-proto

Wire protocol of the AgentCoin inference market: what the wallet's local proxy, gateways
(`ac-gateway`) and providers (`ac-provider`) exchange inside sealed channels
(`ac_crypto::sealed`). The crate does no I/O and is permissively licensed, so future client SDKs
can reuse it.

## HTTP transport

- `POST /ac/v1/sealed` with `Content-Type: application/x-agentcoin-sealed`. The request body is a
  sequence of frames (`frame`): a 4-byte big-endian length followed by the payload, at most
  128 KiB. The first frame is the signed handshake, the others are sealed request chunks. The
  response body carries sealed response chunks, framed the same way and streamed.
- `GET /ac/v1/key` (gateways): the gateway's encapsulation key, signed by its account key
  (`announce::GatewayKey`, context `agentcoin/gateway-kem/v1`). Clients check it against the
  account's on-chain key before sealing anything to it.

## Messages

Every message is SCALE-encoded behind a version byte (`PROTOCOL_VERSION` = 2; version 1
carried no TOPLOC proofs and is refused). Enum indices are wire format and never change.

| Direction | Message | Index | Meaning |
|---|---|---|---|
| user → gateway | `UserMsg::Chat` | 0 | Chat Completions request (JSON) and a voucher equal to the channel's billed total |
| | `UserMsg::Pay` | 1 | a voucher on its own, sent after a bill |
| gateway → user | `GatewayMsg::Delta` | 0 | one streamed chunk (JSON) |
| | `GatewayMsg::Completion` | 1 | a whole non-streamed response (JSON) |
| | `GatewayMsg::Billing` | 2 | the double-signed receipt, its fee, the new billed total and the TOPLOC proofs (if any) |
| | `GatewayMsg::Error` | 3 | error code and message (never request content) |
| | `GatewayMsg::Paid` | 4 | a payment was accepted |
| gateway → provider | `ProviderReq::Infer` | 0 | request ID, job kind, model ID, request (JSON) |
| | `ProviderReq::Cosigned` | 1 | the co-signed receipt, for the provider's records |
| provider → gateway | `ProviderMsg::Delta` | 0 | one streamed chunk (JSON) |
| | `ProviderMsg::Receipt` | 1 | the receipt with the provider's key and signature, the engine's usage and the TOPLOC proofs (if any) |
| | `ProviderMsg::Error` | 2 | error code and message |
| | `ProviderMsg::Ack` | 3 | acknowledges `Cosigned` |

`Payment` has one variant, `Transparent` (index 0, a cumulative channel voucher, D54); index 1
is reserved for shielded vouchers.

## TOPLOC proofs

`toploc::ToplocProofs` carries the proofs of one inference with their parameters (spec
`market/toploc`). The market uses `MARKET_PARAMS`: top-k 128, decode batches of 32 and a prefill
chunk, so an inference with `n` output tokens has `1 + ⌈(n − 1) / 32⌉` proofs of 258 bytes.
`toploc::check` is what gateways and wallet proxies run on every receipt: an all-zero commitment
goes with no proofs; otherwise the parameters, the number of proofs, their encodings and the
recomputed commitment must all match the receipt.

## Engine plugin protocol

`engine` is the local protocol between an inference engine plugin and its receiver (spec
`market/engine-plugin`, version `ENGINE_PROTOCOL_VERSION` = 2): frames of a 4-byte big-endian
length and a fixed big-endian layout (at most 1 MiB), so that the Python plugin encodes them with
`struct`. The plugin sends `Hello` (magic `ACTL`, version, hidden size, mode), the receiver
answers `Welcome` (version, top-k); then the plugin sends `Segment`s (engine request ID, phase,
number of values, top-k candidates) and a `Finish` per request. In the **prove** mode (mode 0) a
provider receives one segment per forward step and request, for proofs; in the **verify** mode
(mode 1) an auditor's re-check receives one segment per prefilled token row. `answer_hello` is
the receivers' handshake: a plugin of another version gets the receiver's version, a plugin in a
mode the receiver does not take gets a top-k of 0, and either way the connection is closed (a
version-1 `Hello`, without a mode byte, still decodes so that it can be answered).
`market_request_id` finds the market request ID in an engine request ID: receivers send requests
with `X-Request-Id` set to its 64 lowercase hex digits and vLLM names them
`chatcmpl-<X-Request-Id>-<suffix>` (chat) or `cmpl-<X-Request-Id>-<suffix>` (completions).
Byte-level vectors shared with the plugin's tests are in `tests/vectors/engine_protocol.json`.

## Audit thresholds

`toploc::judge` is the audit's pass / fail rule over `ac_toploc::compare`'s per-chunk results
(spec `market/toploc` "复核判定规则与阈值"): a chunk passes if its exponent mismatches, its mean
mantissa error (as `sum ≤ bound × count`) and its median mantissa error stay within the
`Thresholds` and at least one exponent matches; an inference passes if every chunk does, and
otherwise fails on the first chunk out of bounds. `AUDIT_THRESHOLDS` are versioned and come from
the calibration of `scripts/calibrate-toploc.py`.

## OpenAI subset

`openai::ChatRequest` keeps requests as JSON objects and rewrites only `model`, `stream`
(adding `stream_options.include_usage`) and the output-token limit; every other field passes
through. `input_bound` bounds prompt tokens by the UTF-8 bytes of the messages plus 16 per
message; `route::max_fee` turns the bounds into the most a request can cost. Parse errors never
quote the input, and `Debug` of a request shows only its model. `SseDecoder`, `sse_data`,
`usage_of`, `has_content` and `Assembler` handle streamed responses; `error_json` builds
OpenAI-format errors.

## Routing

`route::RouteScore` orders candidate providers; `PriceThenLatency` (cheapest first, then lowest
smoothed time to first token) is the default. SLA and reputation join after the M6 audits.

## Example

```rust
use ac_market_proto::{frame, msg, GatewayMsg};

let wire = frame::encode(&msg::encode(&GatewayMsg::Delta(br#"{"choices":[]}"#.to_vec())))?;
let mut decoder = frame::Decoder::new();
decoder.push(&wire[..3]);
assert!(decoder.next_frame()?.is_none());
decoder.push(&wire[3..]);
let payload = decoder.next_frame()?.unwrap_or_default();
let back: GatewayMsg = msg::decode(&payload)?;
assert_eq!(back, GatewayMsg::Delta(br#"{"choices":[]}"#.to_vec()));
# Ok::<(), ac_market_proto::Error>(())
```
