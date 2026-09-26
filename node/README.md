> 🌐 **English** | [简体中文](README.zh-CN.md)

# ac-node

The AgentCoin node client (M1: post-quantum chain).

- **Consensus**: Aura-PQ (`ac-consensus-aura-pq`): ML-DSA-65 block seals, 1 s slots, authorities
  from genesis. There is no finality gadget in M1 (GRANDPA is hard-wired to Ed25519); fork choice
  is the longest chain and AC-BFT finality arrives in M2.
- **Hashing**: blocks and state are hashed with BLAKE3-256.
- **Constitution layer 1**: a `Live` chain spec whose genesis allocates any ATC is refused at
  start-up (no premine, D9). A chain without Aura-PQ authorities is refused as well.
- **Keys**: the authority key comes from an encrypted file (`--pq-key-file` +
  `--pq-password-file`) or, on development and local chains only, from a public development
  name (`--dev-key alice`; `--dev` implies alice). Development keys are refused on live chains.

## Running

```bash
# Single-node development chain (authority alice, endowed dev accounts).
cargo build --release -p ac-node
target/release/ac-node --dev --tmp

# Three-node local testnet (alice, bob, charlie); RPC on 127.0.0.1:9944/9945/9946.
scripts/run-local-testnet.sh
# Same, but check after 40 s that every node reached height 20, then stop.
scripts/run-local-testnet.sh --check

# Create an encrypted authority key; prints the public key for the genesis authority list.
target/release/ac-node pq-key generate --output authority.json --password-file password.txt
target/release/ac-node --chain my-spec.json --validator \
  --pq-key-file authority.json --pq-password-file password.txt
```

Only `node/` may depend on the Polkadot SDK client crates licensed
`GPL-3.0-or-later WITH Classpath-exception-2.0` (decision D37); the node's own source is MIT.
Distributing an `ac-node` binary must honour the GPL terms of those components.
