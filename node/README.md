> 🌐 **English** | [简体中文](README.zh-CN.md)

# ac-node

The AgentCoin node client (M2: post-quantum chain with finality).

- **Block production**: Aura-PQ (`ac-consensus-aura-pq`): ML-DSA-65 block seals, 1 s slots,
  authorities from the validator-set pallet, which changes them only at epoch boundaries.
- **Finality**: AC-BFT (`ac-consensus-bft`), a two-phase BFT gadget with ML-DSA-65 votes
  (`agentcoin/bft-vote/v1`). With `n = 3f + 1` validators it finalizes while at most `f` are
  faulty or offline; blocks keep coming but finality pauses when more are. A finality proof (the
  commit votes) is stored with every block that changes the authority set and at least every 64
  blocks; nodes that sync verify these proofs against the set that must have signed them.
  Validators persist their votes before sending them, so a restart never makes them sign twice.
- **Double signing**: two headers sealed by one key in one slot (seen by the block verifier) or
  two conflicting AC-BFT votes (seen by the gadget) are turned into an unsigned report through
  the runtime and submitted to the local transaction pool. The chain records the offence and
  removes the offender from the set at the next epoch boundary (no stake is slashed in M2).
- **Randomness**: block authors supply commit–reveal secrets as inherent data; epoch `e`'s
  randomness is published at the start of epoch `e + 2`. Secrets derive from the validator key
  and never appear in logs.
- **Hashing**: blocks and state are hashed with BLAKE3-256.
- **Constitution layer 1**: a `Live` chain spec whose genesis allocates any ATC is refused at
  start-up (no premine, D9). A chain without Aura-PQ authorities is refused as well.
- **Keys**: the validator key comes from an encrypted file (`--pq-key-file` +
  `--pq-password-file`) or, on development and local chains only, from a public development
  name (`--dev-key alice`; `--dev` implies alice). Development keys are refused on live chains,
  and only ML-DSA-65 keys are accepted. One key serves block seals, AC-BFT votes and randomness
  (distinct signing and hashing contexts). A node with a key votes whenever the key is in the
  current authority set; a node without one only follows finality.

## Running

```bash
# Single-node development chain (authority alice, endowed dev accounts).
cargo build --release -p ac-node
target/release/ac-node --dev --tmp

# Four-node local testnet (alice, bob, charlie, dave); RPC on 127.0.0.1:9944-9947,
# Prometheus metrics on 127.0.0.1:9615-9618.
scripts/run-local-testnet.sh
# Same, but check after 40 s that every node reached height 20 and finalized height 15, and
# that alice exports acbft_finalized_number; then stop.
scripts/run-local-testnet.sh --check

# AC-BFT metrics of alice.
curl -s http://127.0.0.1:9615/metrics | grep '^acbft_'

# Create an encrypted authority key; prints the public key for the genesis authority list.
target/release/ac-node pq-key generate --output authority.json --password-file password.txt
target/release/ac-node --chain my-spec.json --validator \
  --pq-key-file authority.json --pq-password-file password.txt
```

## Monitoring

With Prometheus enabled (the default, port 9615) the node exports, besides the Substrate metrics:

| Metric | Meaning |
|---|---|
| `acbft_round` | Current AC-BFT round |
| `acbft_finalized_number` | Last block finalized by AC-BFT |
| `acbft_finality_latency_seconds` | Histogram of the time from importing a block to finalizing it |

Log targets: `ac-bft` (gadget; `-lac-bft=debug` logs every signed message), `aura-pq` (block
production and seal double-signing warnings), `ac-offences` (double-signing reports).

## License

Only `node/` may depend on the Polkadot SDK client crates licensed
`GPL-3.0-or-later WITH Classpath-exception-2.0` (decision D37); the node's own source is MIT.
Distributing an `ac-node` binary must honour the GPL terms of those components.
