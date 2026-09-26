> 🌐 **English** | [简体中文](README.zh-CN.md)

# ac-primitives

Shared on-chain types for AgentCoin, used by the runtime, the node and clients.

- `Blake3Hasher`: BLAKE3-256 adapted to the Polkadot SDK hasher traits. It is the chain's
  `Hashing` type, so block hashes, extrinsics roots and state roots are all BLAKE3 (decision D35).
- Addresses: `encode_address` / `decode_address` turn a 32-byte account ID into an `atc1…`
  bech32m (BIP-350) string and back; one wrong character is always detected.
- `NoClassicSignature`: an uninhabited type that fills the signature slot of the SDK's legacy
  `Signed` extrinsic, so no classic signature scheme can authorize an account (decision D36).
- `ChainProfile` and the `ChainProfileApi` runtime API: lets clients check the chain's hashing
  scheme.

## Features

| Feature | Default | Purpose |
|---|---|---|
| `std` | yes | Standard-library support for the node, wallet and tests. Disable it for the WASM runtime. |

## Example

```rust
use ac_primitives::{Blake3Hasher, decode_address, encode_address};
use sp_runtime::traits::Hash;

let digest = Blake3Hasher::hash(b"agentcoin");
assert_eq!(digest.as_ref().len(), 32);

let address = encode_address(&[7u8; 32]);
assert!(address.starts_with("atc1"));
assert_eq!(decode_address(&address), Ok([7u8; 32]));
```
