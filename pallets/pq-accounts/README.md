> 🌐 **English** | [简体中文](README.zh-CN.md)

# pallet-pq-accounts

Post-quantum accounts for the AgentCoin runtime (plan §3.3, decision D36).

- **Registry**: every account that has transacted has one current ML-DSA public key (with its
  AlgId) and a rotation counter. The account ID is derived from the account's **first** key, so
  funds can be sent to an address before the account ever transacts.
- **`PqAuthorize`**: the transaction extension that authorizes v5 `General` transactions. It must
  be the first extension. The account signs, with context `agentcoin/tx/v1`, the 32-byte payload
  `derive_key("agentcoin 2026-09 tx-payload v1", SCALE(inherited implication))` (see
  `signing_payload`). The first transaction carries the public key; later ones must not.
- **`rotate_key(new_key, proof)`**: replaces the current key, possibly with another algorithm, and
  keeps the account ID. `proof` is the new key's signature (context `agentcoin/key-rotation/v1`)
  over `rotation_statement(genesis hash, account, rotation count, new key)`. A key can belong to
  only one account.
- **Runtime API** `PqAccountsApi::current_key(account)` for wallets.

## Features

| Feature | Default | Purpose |
|---|---|---|
| `std` | yes | Native builds and tests. Disable it for the WASM runtime. |
| `runtime-benchmarks` | no | Benchmarks of `rotate_key` and `PqAuthorize` (per algorithm). |
| `try-runtime` | no | SDK try-runtime support. |

## Example

```rust
use ac_crypto::sig::{SecretSeed, SigningKey};
use ac_crypto::SigAlg;
use pallet_pq_accounts::{derived_account, signing_payload, TX_SIGNING_CONTEXT};

let key = SigningKey::from_seed(SigAlg::MlDsa44, &SecretSeed::new([1u8; 32]))?;
let account = derived_account(&key.public_key()?);
// A wallet signs the payload of the transaction's inherited implication.
let payload = signing_payload(&(0u8, b"call bytes"))?;
let signature = key.sign_deterministic(&payload, TX_SIGNING_CONTEXT)?;
assert_eq!(signature.alg(), SigAlg::MlDsa44);
assert_eq!(AsRef::<[u8; 32]>::as_ref(&account).len(), 32);
# Ok::<(), ac_crypto::Error>(())
```
