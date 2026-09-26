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
- Wallet support: 24-word BIP-39 encoding of wallet entropy, deterministic wallet-key and
  development-key derivation, password-encrypted secret files (Argon2id + XChaCha20-Poly1305)
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
| `getrandom` | `OsRng`: CSPRNG seeded from the operating system, never panics | nodes, wallets |
| `mnemonic` | 24-word BIP-39 (English) encoding of wallet entropy (`no_std`) | wallets |
| `keystore` | password-encrypted secret files (implies `std`, `rand` and `getrandom`) | nodes, wallets |

Key-seed derivation (`wallet_key_seed`, `dev_seed`) is always available: a seed is
`derive_key(context, input)` with the contexts below. Development seeds are **public** and only
valid on development and local chains.

## Context registry

Signing contexts (`agentcoin/<purpose>/v<version>`, at most 255 bytes) and hashing contexts
(`agentcoin <YYYY-MM> <purpose> v<version>`) must be registered here before use and must never be
reused for another purpose.

| Context | Kind | Purpose | Status |
|---|---|---|---|
| `agentcoin 2026-09 account-id v1` | hash | account-ID derivation | **in use (consensus-critical)** |
| `agentcoin 2026-09 test-rng v1` | hash | deterministic RNG in tests | tests only |
| `agentcoin 2026-09 tx-payload v1` | hash | 32-byte transaction signing payload | in use from M1 (consensus-critical) |
| `agentcoin 2026-09 wallet-key v1` | hash | wallet key seeds: `AlgId ‖ u32_le(index) ‖ entropy` | in use from M1 |
| `agentcoin 2026-09 dev-seed v1` | hash | public development seeds from a name | in use from M1 (dev chains only) |
| `agentcoin 2026-09 keystore-aad v1` | hash | associated data of encrypted secret files | in use from M1 |
| `agentcoin 2026-09 os-rng v1` | hash | output stream of `OsRng` (seeded from the OS) | in use from M1 |
| `agentcoin/tx/v1` | signature | transaction signatures | in use from M1 (consensus-critical) |
| `agentcoin/aura-seal/v1` | signature | Aura-PQ block seals | in use from M1 (consensus-critical) |
| `agentcoin/key-rotation/v1` | signature | proof of possession of a rotated-in key | in use from M1 (consensus-critical) |
| `agentcoin/bft-vote/v1` | signature | finality votes | reserved for M2 |
| `agentcoin/receipt/v1` | signature | inference receipts | reserved for M5 |

## Encrypted secret files (format v1)

A JSON document with `version` (1), `kind` (`signing-seed` or `wallet-entropy`), `alg` and
`public_key` (canonical hex, signing seeds only), `kdf` (`argon2id` with `m_kib`, `t`, `p`, 16-byte
`salt`) and `cipher` (`xchacha20poly1305` with a 24-byte `nonce` and the `ciphertext`). The
associated data is `derive_key("agentcoin 2026-09 keystore-aad v1", …)` over every metadata field,
so tampering with any field makes decryption fail. Files whose KDF parameters are below
64 MiB / 3 passes / 1 lane are rejected.

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

`tests/vectors/` holds filtered NIST ACVP (ML-DSA, ML-KEM-768), X-Wing, Argon2id (RFC 9106),
XChaCha20-Poly1305 (draft-irtf-cfrg-xchacha-03) and BIP-39 vectors, reproducible with
`scripts/fetch-test-vectors.sh`, plus repository regression vectors; see
`tests/vectors/SOURCES.md`.
