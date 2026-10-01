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
| `poseidon2` | Poseidon2-256 over Goldilocks (`no_std`), see below | EVM precompile (runtime) |
| `sealed` | sealed request channel (implies `kem` and `rand`; `no_std`), see below | gateways, providers, wallets |

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
| `agentcoin 2026-09 bft-message v1` | hash | 32-byte signing payload of AC-BFT messages | in use from M2 (consensus-critical) |
| `agentcoin 2026-09 randomness-secret v1` | hash | validators' per-epoch randomness secrets: `seed ‖ genesis ‖ u64_le(epoch)` | in use from M2 |
| `agentcoin 2026-09 randomness-commit v1` | hash | commitments to randomness secrets | in use from M2 (consensus-critical) |
| `agentcoin 2026-09 randomness v1` | hash | epoch randomness: `u64_le(epoch) ‖ reveals sorted by account ID` | in use from M2 (consensus-critical) |
| `agentcoin 2026-09 randomness-subject v1` | hash | per-subject values derived from epoch randomness | in use from M2 |
| `agentcoin 2026-09 model-id v1` | hash | model IDs: SCALE encoding of the weight manifest | in use from M5 |
| `agentcoin 2026-09 voucher-payload v1` | hash | 32-byte signing payload of transparent credit vouchers | in use from M5 |
| `agentcoin 2026-09 receipt-payload v1` | hash | 32-byte signing payload of inference receipts | in use from M5 |
| `agentcoin 2026-09 receipt-leaf v1` | hash | leaves of a work report's receipt tree: SCALE encoding of the signed receipt | in use from M5 |
| `agentcoin 2026-09 receipt-node v1` | hash | inner nodes of a receipt tree: `left ‖ right` | in use from M5 |
| `agentcoin 2026-09 toploc-commit v1` | hash | TOPLOC commitment in a receipt: parameters and proof encodings | in use from M5 |
| `agentcoin 2026-09 sealed-recipient v1` | hash | sealed channel: digest of the recipient's encapsulation key | in use from M5 |
| `agentcoin 2026-09 sealed-handshake v1` | hash | sealed channel: message signed in a handshake; replay-cache key | in use from M5 |
| `agentcoin 2026-09 sealed-key v1` | hash | sealed channel: per-direction keys from the shared secret and the signed handshake | in use from M5 |
| `agentcoin 2026-09 gateway-kem-payload v1` | hash | payload signed when a gateway announces its encapsulation key | in use from M5 |
| `agentcoin 2026-10 audit-evidence v1` | hash | commitment to audit evidence: its SCALE encoding (`ac-market-proto::audit`) | in use from M6 |
| `agentcoin 2026-10 audit-assign v1` | hash | audit assignment draw: `seed ‖ provider ‖ u32_le(i)` | in use from M6 (on chain) |
| `agentcoin 2026-10 audit-review v1` | hash | dispute reviewer draw: `seed ‖ provider ‖ u64_le(dispute) ‖ u32_le(i)` | in use from M6 (on chain) |
| `agentcoin/tx/v1` | signature | transaction signatures | in use from M1 (consensus-critical) |
| `agentcoin/aura-seal/v1` | signature | Aura-PQ block seals | in use from M1 (consensus-critical) |
| `agentcoin/key-rotation/v1` | signature | proof of possession of a rotated-in key | in use from M1 (consensus-critical) |
| `agentcoin/bft-vote/v1` | signature | every AC-BFT message (proposals, votes, timeouts) | in use from M2 (consensus-critical) |
| `agentcoin/validator-pop/v1` | signature | proof of possession of a validator key registered for staking | in use from M3 (consensus-critical) |
| `agentcoin/evm-verify/v1` | signature | messages verified by contracts through the `pq_verify` precompile | in use from M4 |
| `agentcoin/voucher/v1` | signature | transparent credit vouchers (cumulative, per channel) | in use from M5 |
| `agentcoin/receipt/v1` | signature | inference receipts (signed by provider and gateway) | in use from M5 |
| `agentcoin/sealed-channel/v1` | signature | sealed-channel handshakes (sender authentication) | in use from M5 |
| `agentcoin/gateway-kem/v1` | signature | a gateway's announcement of its encapsulation key | in use from M5 |

## Poseidon2

`poseidon2::hash` (feature `poseidon2`) is a 256-bit Poseidon2 hash for contracts (the
`poseidon2` precompile at `0x…0a030000`) and later STARK work. It wraps Plonky3 (`p3-goldilocks`
0.8, `MIT OR Apache-2.0`); the permutation and the sponge are Plonky3's, nothing is
re-implemented.

- **Instance**: Plonky3's default Goldilocks instance (p = 2^64 − 2^32 + 1), width 12, S-box
  x^7, 8 external and 22 internal rounds, Plonky3's fixed round constants and internal diagonal.
  This is not the instance of the Poseidon2 authors' reference implementation, so it is checked
  against Plonky3's published known-answer vector, not author vectors (a user decision, m4-evm).
- **Sponge**: rate 8, capacity 4, all-zero initial state; each block of 8 elements overwrites
  the rate before a permutation; the output is the first 4 elements, as little-endian 8-byte
  words (32 bytes, 128-bit security).
- **Encoding (injective)**: the input's byte length (at most 2^32 − 1, otherwise
  `Error::InputTooLong`), then the input followed by `0x01` and zeros up to a multiple of 7 bytes,
  as little-endian 7-byte elements, then zero elements up to a multiple of 8.
- **Contexts**: the function is a raw primitive with no domain-separation context, as contracts
  expect; protocol uses of Poseidon2 must still prefix a registered context (see above).

```rust
# #[cfg(feature = "poseidon2")] {
use ac_crypto::poseidon2;

let digest = poseidon2::hash(b"")?;
assert_eq!(digest[..4], [0x81, 0x23, 0xae, 0x34]); // tests/vectors/poseidon2_hash.json
assert_ne!(poseidon2::hash(b"abc")?, poseidon2::hash(b"abc\0")?);
# }
# Ok::<(), ac_crypto::Error>(())
```

## Sealed request channel

`sealed` (feature `sealed`) carries one request and its streamed response between off-chain
services (wallet proxy → gateway → provider), encrypted end to end and bound to the sender's
account:

- **Handshake** (protocol version 1): the sender encapsulates to the recipient's X-Wing key and
  signs `{version, recipient digest, KEM ciphertext, sender account, sender key, creation time,
  nonce}` with its ML-DSA key under `agentcoin/sealed-channel/v1`. The recipient checks the
  digest, a ±120 s freshness window, that the key is the account's current on-chain key (the
  caller supplies the lookup), the signature and a replay cache keyed by the KEM ciphertext.
- **Keys**: one ChaCha20-Poly1305 key per direction,
  `derive_key("agentcoin 2026-09 sealed-key v1", direction ‖ shared secret ‖ signed handshake)`.
- **Chunks**: at most 64 KiB of plaintext each; the nonce is the chunk number and the
  associated data binds direction, number and the final flag, so tampering, reordering, data
  after the last chunk and truncation (`Opener::finish`) are all errors. The response is not
  signed: only the holder of the recipient key can derive its key.

```rust
# #[cfg(all(feature = "sealed", feature = "getrandom"))] {
use ac_crypto::kem::KemSecretKey;
use ac_crypto::sealed::{Acceptor, ReplayCache, accept_session, open_session, recipient_id};
use ac_crypto::sig::{SecretSeed, SigningKey};
use ac_crypto::{KemAlg, OsRng, SigAlg, account_id};

let provider = KemSecretKey::from_seed(KemAlg::XWing, &SecretSeed::new([7; 32]))?;
let gateway = SigningKey::from_seed(SigAlg::MlDsa44, &SecretSeed::new([8; 32]))?;
let (me, now) = (account_id(&gateway.public_key()?), 1_800_000_000);

let (handshake, mut tx) = open_session(&provider.public_key()?, &gateway, me, now, &mut OsRng::new()?)?;
let key = gateway.public_key()?;
let mut rx = accept_session(
    &handshake,
    &Acceptor { secret: &provider, recipient: recipient_id(&provider.public_key()?)?, now },
    |who| (*who == me).then_some(key),
    &mut ReplayCache::default(),
)?;
let chunk = tx.request.seal(b"prompt", true)?;
assert_eq!(rx.request.open(&chunk)?, (b"prompt".to_vec(), true));
rx.request.finish()?;
# }
# Ok::<(), ac_crypto::Error>(())
```

## Encrypted secret files (format v1)

A JSON document with `version` (1), `kind` (`signing-seed`, `wallet-entropy` or `kem-seed`),
`alg` and `public_key` (canonical hex: the signing key for signing seeds, the encapsulation key
for KEM seeds; absent for wallet entropy), `kdf` (`argon2id` with `m_kib`, `t`, `p`, 16-byte
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
XChaCha20-Poly1305 (draft-irtf-cfrg-xchacha-03), ChaCha20-Poly1305 (RFC 8439) and BIP-39 vectors and Plonky3's Poseidon2
permutation vector, reproducible with `scripts/fetch-test-vectors.sh`, plus repository
regression vectors (including Poseidon2 hashes and a sealed-channel session); see `tests/vectors/SOURCES.md`.
