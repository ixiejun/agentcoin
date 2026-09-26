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
- `aura_pq`: Aura-PQ slot, pre-runtime digest and seal digest helpers.
- `ac_bft`: AC-BFT finality types — versioned signed messages (proposals, prepare / commit votes,
  timeouts) signed with context `agentcoin/bft-vote/v1`, finality proofs and
  `verify_finality_proof`, and the authority-set change digest (engine `acbf`). Unknown format
  versions fail to decode instead of being read as another format.
- `offences`: double-signing evidence (two blocks sealed in one slot, or two conflicting AC-BFT
  messages in one round) and `verify_evidence`, used by the runtime and the node alike.
- `epoch`: epoch numbering (`epoch_of`, `is_boundary`) and the minimum epoch length.

Byte-level regression vectors for the AC-BFT formats live in `tests/vectors/` (see `SOURCES.md`).

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

## Verifying a finality proof

A finality proof is checked with nothing but the genesis hash and the authority set it names, so
light clients can verify finality without trusting a node.

```rust
# #[cfg(feature = "std")] {
use ac_crypto::{SigAlg, dev_seed, sig::SigningKey};
use ac_primitives::ac_bft::{
    Authority, BlockRef, FinalityProof, Message, VOTE_CONTEXT, VersionedFinalityProof, VoteKind,
    signing_payload, verify_finality_proof,
};
use sp_core::H256;

let keys: Vec<SigningKey> = ["alice", "bob", "charlie", "dave"]
    .iter()
    .map(|n| SigningKey::from_seed(SigAlg::MlDsa65, &dev_seed(n)?))
    .collect::<Result<_, _>>()?;
let set: Vec<Authority> = keys
    .iter()
    .map(|k| k.public_key().map(Authority::poa))
    .collect::<Result<_, _>>()?;

let genesis = H256::repeat_byte(1);
let target = BlockRef { hash: H256::repeat_byte(2), number: 10 };
let commit = Message::Vote { kind: VoteKind::Commit, round: 0, target };
let payload = signing_payload(&genesis, 0, &commit).expect("fixed context");

// Three of four members (q = ⌊2·4/3⌋ + 1 = 3) signed the commit vote.
let mut commits = Vec::new();
for (index, key) in keys.iter().enumerate().take(3) {
    commits.push((index as u16, key.sign_deterministic(&payload, VOTE_CONTEXT)?));
}
let proof = VersionedFinalityProof::V1(FinalityProof {
    set_id: 0,
    round: 0,
    target,
    commits: commits.try_into().expect("at most 100 members"),
});
assert_eq!(verify_finality_proof(&genesis, 0, &set, &proof), Ok(target));
# }
# Ok::<(), ac_crypto::Error>(())
```

