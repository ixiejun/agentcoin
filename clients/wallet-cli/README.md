> 🌐 **English** | [简体中文](README.zh-CN.md)

# ac-wallet

AgentCoin's command-line wallet (M1), the external signer for EVM contracts (M4) and the
inference-market client (M5).

- Keys: 24-word BIP-39 mnemonic over 256-bit entropy; account keys are derived per algorithm
  and index (`agentcoin 2026-09 wallet-key v1`). The default account algorithm is ML-DSA-44.
- The wallet file stores only the Argon2id + XChaCha20-Poly1305 encrypted entropy and public
  bookkeeping; passphrases come from the terminal or `--password-file`, never from arguments.
- Addresses are bech32m `atc1…` strings; a single mistyped character is rejected.
- Transactions are v5 general transactions authorized by `PqAuthorize`; the first transaction
  of an account carries its public key automatically.
- `rotate` moves to the next derivation index (optionally another ML-DSA parameter set); the
  address never changes.

## Usage

```bash
ac-wallet new --wallet alice.json            # prints the address and, once, the mnemonic
ac-wallet import --wallet restored.json      # reads the mnemonic from standard input
ac-wallet address --wallet alice.json
ac-wallet balance --wallet alice.json --node http://127.0.0.1:9944
ac-wallet transfer --wallet alice.json --to atc1... --amount 1.5
ac-wallet rotate --wallet alice.json --alg ml-dsa-65
```

`scripts/wallet-smoke.sh` runs these commands against a development node.

## EVM contracts

Contract transactions are AgentCoin transactions signed with the account's ML-DSA key; Foundry
builds and reads, the wallet signs. The account's EVM address is `keccak256(account)[12..]`.

```bash
ac-wallet evm address --wallet alice.json    # EIP-55, e.g. 0x05a0…9F70
forge build                                  # in a Foundry project
ac-wallet evm deploy --wallet alice.json --artifact out/ERC20.sol/ERC20.json \
  --constructor "constructor(string,string,uint256)" Token TKN 1000000000000000000000
ac-wallet evm send --wallet alice.json --to 0x… --sig "transfer(address,uint256)" 0x… 5
cast call 0x… "balanceOf(address)(uint256)" 0x… --rpc-url http://127.0.0.1:8545   # ac-eth-rpc
```

| Command | What it does |
|---|---|
| `evm address` | the account's EVM address (EIP-55) |
| `evm deploy` | deploys a `forge build` artifact (`--artifact`) or init code (`--bytecode`); constructor arguments as text after `--constructor <signature>` or encoded with `--constructor-args` |
| `evm send` | calls a contract with `--sig <signature>` and arguments, or `--data` |
| `evm raw deploy`, `evm raw send` | signs without submitting; prints the hash and the bytes for `eth_sendRawTransaction` |
| `evm broadcast --file run-latest.json` | sends the transactions of a `forge script` simulation, one at a time |
| `evm sign-message` | signs a message under `agentcoin/evm-verify/v1` for the `pq_verify` precompile |

Before signing, the wallet dry-runs the transaction on the latest state and sets the weight
limit to the requirement plus 20% and the storage-deposit limit to the peak plus 10%
(`--ref-time-limit`, `--proof-size-limit`, `--deposit-limit` override them). A transaction the
dry run shows reverting is not submitted; the reason is printed (`--force` submits anyway).
Values (`--value`, `--deposit-limit`) are in ATC.

`evm broadcast` needs a simulation made with the wallet's address as sender
(`forge script <script> --sender <evm address> --rpc-url <adapter>`, without `--broadcast`), and
stops at the first transaction that fails or that creates a contract elsewhere than the script
predicted; later transactions are not sent. Note that a native deployment advances the account
nonce by two (once for the transaction, once by `pallet-revive`), so a script can predict only
its first deployment correctly.

## Inference market

`ac-wallet market …` registers and operates in the inference market (M5): models, providers,
gateways, escrow and vouchers. Dollar amounts are decimals with at most six places (`0.5`);
ATC amounts use the usual format. A failed transaction prints the chain's error name, for
example `Providers(BelowThreshold)`.

```bash
ac-wallet market rate                                            # reference rate (ATC per USD)
ac-wallet market model id --file model.json                      # the model ID, offline
ac-wallet market model register --wallet alice.json --file model.json
ac-wallet market provider register --wallet p.json --tier t2 --endpoint https://p.example \
  --kem-key 0x01… --model 0x<model id>:0.1:0.2                    # $ per million input:output tokens
ac-wallet market providers --model 0x<model id>                  # serviceable providers
ac-wallet market gateway register --wallet g.json --endpoint https://g.example --fee-bps 300
ac-wallet market escrow deposit --wallet alice.json --gateway atc1… --amount 5
ac-wallet market voucher sign --wallet alice.json --gateway atc1… --usd 0.5
ac-wallet market voucher check --voucher 0x…
```

| Command | What it does |
|---|---|
| `market model register --file`, `model show --id`, `model id --file` | registers a model from a manifest file (the ID is printed before submitting), shows one, or computes an ID offline |
| `market provider register` | `--tier t1\|t2`, `--endpoint`, `--kem-key` (hex of the AlgId-tagged X-Wing key), one or more `--model ID:INPUT_USD:OUTPUT_USD`, `--stake` (defaults to the current threshold) |
| `market provider update`, `heartbeat`, `bond`, `unbond`, `exit`, `withdraw`, `show` | change the endpoint, key or models; heartbeat; add or unbond stake; leave; withdraw due stake; show a provider and whether it is serviceable |
| `market providers --model` | every serviceable provider of a model |
| `market gateway register`, `update`, `bond`, `unbond`, `exit`, `withdraw`, `show` | the same for gateways (`--fee-bps` at most 500) |
| `market escrow deposit`, `request-withdrawal`, `withdraw`, `change-key` | escrow ATC with a gateway; withdraw it after the delay; make the wallet's current key the channel's voucher key after the delay |
| `market channel --gateway` | the channel's escrow, number, redeemed amount, voucher key and pending requests |
| `market voucher sign --gateway --usd` | signs a cumulative voucher (`agentcoin/voucher/v1`) and prints its hex |
| `market voucher check --voucher` | checks a voucher against the chain exactly as redemption would |

A manifest file:

```json
{ "name": "Qwen2.5-0.5B-Instruct", "arch": "qwen2", "quant": "int4",
  "shards": ["0x…32-byte BLAKE3 of shard 1…", "0x…"],
  "lineage": { "parent": "0x<model id>", "kind": "quantize" },
  "licenseTag": "apache-2.0" }
```

`quant` is one of `bf16`, `fp16`, `fp8`, `int8`, `int4`; `lineage` (kinds `finetune`,
`quantize`, `distill`, `merge`) and `licenseTag` are optional.

## Work settlement

Providers and gateways sign one receipt per request; the gateway submits a work report and,
after the challenge period (two emission epochs), everyone claims (`pallet-work`). A receipt
file is JSON with the hex of the receipt body and each party's key and signature; it never
holds the prompt, the output or the user.

```bash
ac-wallet market receipt new --wallet p.json --gateway atc1… --provider atc1… \
  --model 0x<model id> --in-tokens 1000 --out-tokens 2000 --out r1.json   # fee from the chain's price
ac-wallet market receipt cosign --wallet g.json --file r1.json
ac-wallet market receipt check --file r1.json
ac-wallet market report submit --wallet g.json --receipt r1.json --receipt r2.json --voucher 0x…
ac-wallet market report show --id 0
ac-wallet market claim --wallet any.json --account atc1…                  # everything matured
ac-wallet market work --account atc1…                                     # held payments and work
ac-wallet market work --epoch 12                                          # an epoch's verified work
```

| Command | What it does |
|---|---|
| `market receipt new` | builds a receipt (fee = the provider's price for the model, rounded up to a micro-dollar; `--toploc`, `--request-id`, `--ttft-ms`, `--total-ms` optional) and signs it as its provider or gateway |
| `market receipt cosign --file` | adds the wallet's signature as the other party |
| `market receipt check --file` | checks both signatures, the genesis and the fee against the chain's keys and prices |
| `market report submit` | builds the Merkle root and the per-(provider, model) totals, checks every receipt and that the totals equal the vouchers' increments, then submits; an inconsistency is reported with the file or voucher at fault and nothing is submitted |
| `market report show --id` | a stored report with its shares and work |
| `market claim [--account] [--epochs]` | claims every held payment and emission share of settled epochs for an account (by default the wallet's) |
| `market work [--account \| --epoch]` | an account's lifetime work, held payments and unclaimed work, or an epoch's verified work and market emission |

## Audits

Auditors (m6-audit-chain; `pallet-audit`) stake, get assigned to providers each round, submit
verdicts from `ac-auditor` re-checks and vote in disputes. The wallet checks before signing that
the account is assigned to the receipt's provider in the current round.

```bash
ac-wallet audit register --wallet a.json                   # stake: the current threshold
ac-wallet audit assignments --wallet a.json                # this round's providers
ac-wallet audit verdict --wallet a.json --report r.json --case c.json
ac-wallet audit disputes --provider atc1…                   # open dispute, reviewers, counts
ac-wallet audit vote --wallet a.json --provider atc1… --id 0 --vote confirm
ac-wallet audit close --wallet any.json --provider atc1… --id 0   # after the deadline
ac-wallet audit status --address atc1…
ac-wallet audit pot && ac-wallet audit params
```

| Command | What it does |
|---|---|
| `audit register [--stake]`, `bond`, `unbond`, `exit`, `withdraw` | auditor stake, as for providers |
| `audit assignments` | the current round and the providers the account is assigned to (submitted or pending) |
| `audit verdict --report --case` | submits the verdict of an `ac-auditor recheck` line on a case: outcome and thresholds version from the line, receipt from the case and, for a failure, the evidence commitment (printed); refuses before signing if the account is not assigned |
| `audit disputes --provider` | a provider's verdict counts, open dispute, accusers and reviewers' votes |
| `audit vote --provider --id --vote confirm\|reject`, `audit close` | votes as a reviewer; closes an undecided dispute after its deadline |
| `audit endpoint --set URL --kem-key KEY` / `--clear` | publishes (or removes) where the auditor serves evidence to reviewers; `ac-auditor run` does it by itself |
| `audit status`, `audit pot`, `audit params` | an auditor's record, counts and evidence endpoint; the pot; the parameters |

## Public jobs

Workers (m6-public-jobs; `pallet-public-jobs`) register with no stake and run `ac-worker`;
publishing goes through the administration. `public publish` and `public cancel` only print the
call (and its length bound) for the administration's motion; `publish` first checks that the
manifest file lists exactly `--units` shards, that the model is registered and that the price is
within the current cap.

```bash
ac-wallet public worker register --wallet w.json --model 0x<model id>   # none: cleaning only
ac-wallet public worker status --address atc1…                          # record, unsettled units
ac-wallet public publish --kind eval --model 0x… --manifest m.json \
  --manifest-url https://data.example/m.json --results-url https://collect.example \
  --units 100 --price 0.05 --canary-root 0x…                             # prints `call: 0x…`
ac-wallet public jobs && ac-wallet public unit --job 0 --unit 3
ac-wallet public canary --wallet any.json --file canaries.json --unit 3
ac-wallet public claim --wallet w.json && ac-wallet public withdraw --wallet w.json
ac-wallet public rewards --address atc1… && ac-wallet public params
```

| Command | What it does |
|---|---|
| `public worker register --model…`, `models`, `deregister`, `ready`, `status` | worker registration, its models, readiness for the round (`ac-worker run` does it), its record and unsettled units |
| `public publish …`, `public cancel --job` | print the publish or cancel call for the administration's motion after the local checks |
| `public jobs`, `public unit --job --unit`, `public params` | jobs in progress and their progress; a unit's workers, reveals and state; the parameters, the round and the payout balance |
| `public canary --file --unit` | reveals a unit's canary from an `ac-worker canary` file |
| `public claim [--epoch…]`, `public withdraw`, `public rewards` | claims settled epochs (default: all), withdraws unlocked rewards, shows pending work and locked rewards with their unlock blocks |

## Local inference proxy

`ac-wallet market serve` runs an OpenAI-compatible endpoint on this machine (`GET /v1/models`,
`POST /v1/chat/completions`, streamed or not) that pays a gateway from your credit channel, so
unmodified OpenAI SDKs can use the market:

```bash
ac-wallet market escrow deposit --wallet user.json --gateway atc1… --amount 10
ac-wallet market serve --wallet user.json --gateway atc1… --max-usd 5
# listening on http://127.0.0.1:8411/v1
```

```python
from openai import OpenAI
client = OpenAI(base_url="http://127.0.0.1:8411/v1", api_key="unused")
for chunk in client.chat.completions.create(
        model="Qwen2.5-0.5B-Instruct",  # a model name or its 0x… ID
        messages=[{"role": "user", "content": "Hello!"}], stream=True):
    print(chunk.choices[0].delta.content if chunk.choices else "", end="")
```

- At start it checks that you have a channel with the gateway, that the wallet key is the
  channel's voucher key and that the gateway's encryption key is signed by the gateway account.
- Each request is sealed to the gateway (X-Wing + ML-DSA) with a voucher for exactly what you
  have paid so far. After the response the proxy checks the double-signed receipt (signatures,
  chain, gateway, model, token counts against the returned usage, fee at the provider's on-chain
  price, the gateway's billed total) and its TOPLOC proofs (an all-zero commitment comes
  without proofs; otherwise the market parameters, one proof per chunk of the output and a
  recomputed commitment equal to the receipt's), and only then pays that total. A receipt that
  fails any check stops all further payments and requests. The proofs are not stored.
- `--max-usd` caps the channel's paid total; the paid total is kept in
  `serve-<gateway>.json` next to the wallet (`--state`) so restarts never pay twice.
- A request with the header `X-AgentCoin-Provider: atc1…` is pinned to that provider: the
  gateway sends it there or answers `no_provider` (never another provider), and the proxy pays
  only a receipt naming that provider. With the OpenAI SDK:
  `client.chat.completions.create(..., extra_headers={"X-AgentCoin-Provider": "atc1…"})`.
- It listens on `127.0.0.1:8411` by default and has no authentication: never expose it. Your
  private key never leaves the machine, and the proxy logs no prompts or outputs.

## Library example

```rust
use ac_wallet::amount::{format_atc, parse_atc};

let units = parse_atc("1.5")?;
assert_eq!(format_atc(units), "1.5 ATC");

// Call data from a Solidity signature and textual arguments.
use ac_wallet::evm::abi::Signature;
let transfer = Signature::parse("transfer(address,uint256)")?;
assert_eq!(transfer.selector(), [0xa9, 0x05, 0x9c, 0xbb]);
let data = transfer.calldata(&["0x00000000000000000000000000000000000000aa".into(), "1000".into()])?;
assert_eq!(data.len(), 4 + 32 + 32);
# Ok::<(), anyhow::Error>(())
```
