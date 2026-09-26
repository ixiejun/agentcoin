> 🌐 **English** | [简体中文](README.zh-CN.md)

# pallet-aura-pq

Runtime side of Aura-PQ, AgentCoin's post-quantum block authoring (plan §3.5, §4.1).

- **Authority set**: ML-DSA-65 public keys in slot-assignment order; slot `s` belongs to authority
  `s mod N`. In M1 the set comes from genesis (PoA). Genesis rejects duplicates, non-ML-DSA-65
  keys and sets larger than `MaxAuthorities`; an empty default genesis installs nothing and the
  node refuses to run such a chain.
- **Slot tracking**: `on_initialize` records the slot announced in the block's Aura-PQ pre-runtime
  digest. Block validity (slot order, author, seal) is enforced by the node's import verifier; the
  runtime never panics during block execution and only logs inconsistencies.
- **`AuthoritySetWriter`**: the entry point reserved for M3's validator-set state machine
  (PoA → PoS).
- The `AuraPqApi` runtime API, digest helpers and the seal context `agentcoin/aura-seal/v1` live
  in `ac_primitives::aura_pq`, shared with the node.

## Features

| Feature | Default | Purpose |
|---|---|---|
| `std` | yes | Native builds and tests. Disable it for the WASM runtime. |
| `try-runtime` | no | SDK try-runtime support. |

## Example

```rust
use ac_crypto::sig::{SecretSeed, SigningKey};
use ac_crypto::SigAlg;
use ac_primitives::aura_pq::{Slot, slot_author, validate_authorities, MAX_AUTHORITIES};

let authorities: Vec<_> = (1u8..=3)
    .map(|i| SigningKey::from_seed(SigAlg::MlDsa65, &SecretSeed::new([i; 32]))?.public_key())
    .collect::<Result<_, _>>()?;
assert!(validate_authorities(&authorities, MAX_AUTHORITIES).is_ok());
assert_eq!(slot_author(Slot::from(4), &authorities), authorities.get(1));
# Ok::<(), ac_crypto::Error>(())
```
