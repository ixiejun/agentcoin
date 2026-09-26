> 🌐 **English** | [简体中文](README.zh-CN.md)

# ac-crypto

AgentCoin's post-quantum cryptography library. Every public key, signature and ciphertext carries
an **algorithm identifier (AlgId)**, so algorithms can be added or replaced by allocating a new
number, without changing the meaning of any existing data. This is the only crate in the
repository allowed to call concrete cryptographic implementations (AGENT.md §6).

- Signatures: ML-DSA-44 / 65 / 87 (FIPS 204, pure interface, mandatory context string)
- Hybrid KEM: X-Wing = ML-KEM-768 + X25519 (draft-connolly-cfrg-xwing-kem-06)
- Hashing: BLAKE3-256, SHA3-256, domain-separated BLAKE3 (`derive_key`)
- 32-byte account IDs derived from public keys
- `no_std`; verification, hashing and account IDs need neither `std` nor an RNG (WASM runtime)

## Example

```rust
# #[cfg(feature = "deterministic")] {
use ac_crypto::sig::{SecretSeed, SigningKey, verify};
use ac_crypto::{SigAlg, account_id};

let key = SigningKey::from_seed(SigAlg::MlDsa44, &SecretSeed::new([7u8; 32]))?;
let public_key = key.public_key()?;
let signature = key.sign_deterministic(b"transfer 1 ATC", b"agentcoin/tx/v1")?;

assert!(verify(&public_key, b"transfer 1 ATC", b"agentcoin/tx/v1", &signature).is_ok());
assert!(verify(&public_key, b"transfer 1 ATC", b"agentcoin/bft-vote/v1", &signature).is_err());

let wire = signature.to_canonical(); // 1-byte AlgId ‖ raw signature
assert_eq!(wire[0], 0x01);
assert_eq!(wire.len(), 1 + 2420);
let _account = account_id(&public_key);
# }
# Ok::<(), ac_crypto::Error>(())
```

## AlgId table

Each category (signatures, KEMs) has its own 1-byte space. Numbers are never reused and never
change meaning; `0x00` is never allocated and `0xFF` is reserved in every category as an
extension marker. Families are grouped: ML-DSA `0x01–0x0F`, SLH-DSA `0x10–0x1F`,
FN-DSA `0x20–0x2F`, XMSS `0x30–0x3F`.

| Category | AlgId | Algorithm | Status | Raw public key | Raw signature / ciphertext |
|---|---|---|---|---|---|
| signature | `0x01` | ML-DSA-44 | implemented | 1312 | 2420 |
| signature | `0x02` | ML-DSA-65 | implemented | 1952 | 3309 |
| signature | `0x03` | ML-DSA-87 | implemented | 2592 | 4627 |
| signature | `0x10` | SLH-DSA-SHA2-128s | reserved | — | — |
| signature | `0x20` | FN-DSA-512 | reserved | — | — |
| signature | `0x30` | XMSS (lean) | reserved | — | — |
| KEM | `0x01` | X-Wing (ML-KEM-768 + X25519, draft-06) | implemented | 1216 | 1120 |
| KEM | `0x02` | ML-KEM-1024 | reserved | — | — |
| every category | `0xFF` | extension marker | reserved | — | — |

## Wire format

`PqPublicKey`, `PqSignature`, `KemPublicKey` and `KemCiphertext` are enums whose variant index is
the AlgId and whose payload is the algorithm's fixed-length bytes. Their canonical encoding is
`AlgId (1 byte) ‖ raw bytes`, which is also exactly their SCALE encoding, their on-chain encoding
and what their `TypeInfo` metadata describes (decision D34) — one byte form everywhere.
Unknown AlgIds, reserved AlgIds, wrong lengths and trailing bytes are rejected with an error.

## Features

| Feature | Enables | Used by |
|---|---|---|
| *(none)* | types, decoding, verification, hashing, account IDs (`no_std`, no RNG) | WASM runtime |
| `std` | `std::error::Error` for `Error` | native binaries |
| `rand` | key generation from an RNG, hedged signing, KEM encapsulation | nodes, wallets |
| `kem` | X-Wing hybrid KEM | node P2P, gateways, clients |
| `deterministic` | deterministic signing, derandomized encapsulation | tests, tooling only |
| `scale` | SCALE `Encode` / `Decode` / `MaxEncodedLen` / `TypeInfo` for tagged types | pallets |

## Context registry

Signing contexts (`agentcoin/<purpose>/v<version>`, at most 255 bytes) and hashing contexts
(`agentcoin <YYYY-MM> <purpose> v<version>`) must be registered here before use and must never be
reused for another purpose.

| Context | Kind | Purpose | Status |
|---|---|---|---|
| `agentcoin 2026-09 account-id v1` | hash | account-ID derivation | **in use (consensus-critical)** |
| `agentcoin 2026-09 test-rng v1` | hash | deterministic RNG in tests | tests only |
| `agentcoin/tx/v1` | signature | transaction signatures | reserved for M1 |
| `agentcoin/bft-vote/v1` | signature | finality votes | reserved for M2 |
| `agentcoin/receipt/v1` | signature | inference receipts | reserved for M5 |

## Adding an algorithm

1. Propose it through OpenSpec (AGENT.md §3), citing the decisions it serves.
2. Allocate a new AlgId in `src/alg.rs` (or turn a reserved one into an implemented one), add the
   variant with `codec(index = AlgId)` in `src/tagged.rs`, and extend the frozen tables in
   `tests/alg_table.rs` and `tests/scale_codec.rs` — never edit existing rows.
3. Implement the backend in its own module that alone depends on the new library; dispatch on the
   AlgId in `sig/mod.rs` or `kem/mod.rs`.
4. Add official test vectors via `scripts/fetch-test-vectors.sh` and record them in
   `tests/vectors/SOURCES.md`.
5. Update this README (both languages) and the context registry if new contexts are needed.

## Test vectors

`tests/vectors/` holds filtered NIST ACVP (ML-DSA, ML-KEM-768) and X-Wing specification vectors,
reproducible with `scripts/fetch-test-vectors.sh`; see `tests/vectors/SOURCES.md`.
