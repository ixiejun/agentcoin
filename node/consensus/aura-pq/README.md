> 🌐 **English** | [简体中文](README.zh-CN.md)

# ac-consensus-aura-pq

Node side of Aura-PQ, AgentCoin's post-quantum block authoring (plan §4.1).

The SDK's `sc-consensus-aura` signs through the keystore, which only supports classic key types
(sr25519, Ed25519, ECDSA, BLS). Aura-PQ reuses the generic slot machinery of
`sc-consensus-slots` and holds the ML-DSA-65 authority key in memory instead.

- **Authoring** (`start_aura_pq`): slot `s` belongs to authority `s mod N` (1 s slots). The
  author adds a pre-runtime digest with the slot (engine ID `acpq`) and seals the block with a
  hedged ML-DSA-65 signature over the hash of the unsealed header, context
  `agentcoin/aura-seal/v1`.
- **Import** (`import_queue`): rejects blocks whose seal is missing or invalid, whose slot digest
  is missing or duplicated, whose slot is more than one slot ahead of the local clock or not above
  the parent's, or whose author is not entitled to the slot. Inherents are checked as usual.
- **Equivocation**: two headers from one author in one slot are logged with both SCALE-encoded
  headers (`equivocation::check`). Slashing comes with AC-BFT in M2.
- There is no finality gadget in M1; fork choice is the longest chain.

## Example

```rust
use ac_consensus_aura_pq::seal::{check_header, seal};
use ac_crypto::sig::{SecretSeed, SigningKey};
use ac_crypto::SigAlg;
use ac_primitives::aura_pq::{pre_digest, Slot};
use sp_runtime::traits::Header as _;
use sp_runtime::{generic, Digest};

type Header = generic::Header<u32, ac_primitives::Blake3Hasher>;

let key = SigningKey::from_seed(SigAlg::MlDsa65, &SecretSeed::new([1u8; 32]))?;
let authorities = vec![key.public_key()?];

let mut digest = Digest::default();
digest.push(pre_digest(Slot::from(10)));
let mut header = Header::new(1, Default::default(), Default::default(), Default::default(), digest);
let pre_hash = header.hash();
header.digest_mut().push(seal(&key, pre_hash.as_ref())?);

let checked = check_header(header, None, Slot::from(10), &authorities).expect("valid header");
assert_eq!(checked.slot, Slot::from(10));
# Ok::<(), ac_crypto::Error>(())
```
