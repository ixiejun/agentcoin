> 🌐 **English** | [简体中文](README.zh-CN.md)

# ac-gateway

The AgentCoin inference gateway (M5, spec `market/gateway-service`). Users reach it through the
wallet's local proxy (`ac-wallet market serve`), which gives unmodified OpenAI SDKs a local
endpoint. The gateway:

- publishes its X-Wing key signed by its account key (`GET /ac/v1/key`) and the models that have
  serviceable providers (`GET /ac/v1/models`);
- accepts OpenAI Chat Completions requests only through sealed channels (`POST /ac/v1/sealed`)
  from users with a credit channel, streamed or not;
- is **postpaid** on transparent credits (D54): a request carries a cumulative voucher that must
  equal the channel's billed total **exactly**, and the escrow must cover the billed but
  unredeemed total plus the maximum fee of every request in flight. After each request the user
  pays with a voucher for the new total (a separate `Pay` message). Exact vouchers make every
  work report's totals equal the redeemed increments;
- routes to the cheapest serviceable provider, then the fastest (smoothed time to first token);
  before the first chunk it fails over to the next provider, after it the response ends with an
  error and nothing is billed. Unreachable providers are paused 30 s, providers that sign an
  invalid receipt one hour; T0/TEE providers are not used in this phase. A request **pinned**
  to one provider (`ChatTo`, the proxy's `X-AgentCoin-Provider` header; open to every user and
  used by auditors) goes to that provider only if it is serviceable for the model and not
  paused, else `no_provider` with nothing billed; it never fails over to another provider;
- checks every provider receipt (parties, request, token counts equal to the usage and within the
  request's bounds, fee equal to the on-chain price) and its TOPLOC proofs (an all-zero
  commitment comes without proofs; otherwise the market parameters, one proof per chunk of the
  output and a recomputed commitment equal to the receipt's, `ac_market_proto::toploc::check`),
  co-signs it, bills it, returns it to the user with the fee, the new total and the proofs, and
  hands it back to the provider; a receipt whose proofs do not fit counts as invalid;
- submits work reports every `--report-interval` blocks with the receipts each channel's latest
  voucher covers (at most 16 vouchers and 128 entries per report, split otherwise), keeps the
  receipts and proofs of each report until its maturity epoch is settled, then claims its
  gateway fee;
- **never logs or stores prompts or outputs**: the data directory holds the channel ledger,
  receipts and TOPLOC proofs only, log lines carry IDs, token counts, fees, latencies and error
  codes.

## Usage

```bash
ac-gateway keygen --out gateway-kem.json --password-file pw.txt
ac-wallet market gateway register --wallet gateway.json --endpoint https://gateway.example --fee-bps 300
ac-gateway run --wallet gateway.json --password-file pw.txt --kem-key gateway-kem.json \
  --node http://127.0.0.1:9944 --listen 0.0.0.0:8431 --data-dir /var/lib/ac-gateway
```

The account must be a registered, active gateway. Options: `--report-interval <blocks>`
(default 3,600, capped at one emission epoch), `--max-output-tokens` (limit applied when a request
gives none, default 4,096), `--log-level`.

## Operation

- **TLS**: terminate it in a reverse proxy if the endpoint is public; request content is sealed
  end to end regardless. Rate limiting belongs in the proxy too.
- **Clock**: handshakes are valid for ±120 s, so run NTP.
- **Data directory**: `channels/` (one JSON file per user: channel number, billed total, latest
  voucher, unreported receipts with their TOPLOC proofs; written and synced before a bill is
  acknowledged) and `reports/` (receipts and proofs of submitted reports until they mature). Back it up: unreported receipts
  are money owed to providers.

## Feature flags

None.
