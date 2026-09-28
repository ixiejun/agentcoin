> 🌐 **English** | [简体中文](README.zh-CN.md)

# pallet-model-registry

Permissionless registration of inference models (plan §5.3). Gateways, providers and auditors
refer to a model by one content-addressed ID that anyone can recompute.

## Model IDs

A model's **manifest** lists its name and architecture (non-empty UTF-8, at most 128 bytes
each), its weight format (`QuantType`: `Bf16 = 1`, `Fp16 = 2`, `Fp8 = 3`, `Int8 = 4`,
`Int4 = 5`; numbers never reused) and the BLAKE3-256 hash of each weight shard in load order
(1 to 1,024 shards). The model ID is

```text
model_id = BLAKE3-derive_key("agentcoin 2026-09 model-id v1", SCALE(manifest))
```

computed by the chain from the submitted manifest. `ac_primitives::market::ModelManifest::id`
computes the same value off chain; changing the shard order changes the ID.

## Calls

| Call | Origin | Effect |
|---|---|---|
| `register(manifest, lineage, license_tag, royalty)` | signed | Registers the model under its computed ID and holds a storage deposit on the caller. |

A registration fails if the manifest was already registered (by anyone), if the manifest or the
licence tag is not valid UTF-8, if the declared parent is not registered or is the model itself,
if a royalty is given (reserved for the full version, D22), or if the caller cannot cover the
deposit. Records are immutable and, in the MVP, never removed.

- **Lineage** (D28): an optional parent model and a kind (`Finetune`, `Quantize`, `Distill`,
  `Merge`). The protocol records the claim; it does not verify it.
- **Licence tag**: informational only (up to 64 bytes); the protocol never acts on it (D6).
- **Deposit**: `DepositPerItem + DepositPerByte × encoded record size`, held with the pallet's
  `Deposit` hold reason (0.01 ATC per item and 0.0001 ATC per byte in the runtime, the contract
  storage formula). Holding does not change the issuance.

## Other pallets

`pallet-providers` checks that the models a provider serves exist through
`ac_primitives::market::traits::ModelLookup`, which this pallet implements.

## Features

| Feature | Default | Purpose |
|---|---|---|
| `std` | yes | Native builds and tests. Disable it for the WASM runtime. |
| `runtime-benchmarks` | no | Benchmark of `register` (linear in the number of shards). |
| `try-runtime` | no | SDK try-runtime support. |

## Example

```rust
use ac_primitives::market::ModelManifest;
use ac_primitives::market::model::QuantType;

let manifest = ModelManifest::new(
    b"Qwen2.5-0.5B-Instruct",
    b"qwen2",
    QuantType::Int4,
    vec![[1u8; 32], [2u8; 32]],
)
.unwrap();
// The published regression vector of this manifest.
assert_eq!(
    format!("{:?}", manifest.id().unwrap()),
    "0x226bb1bf7ef7963e7bc8f666002192b287323e1343ab59f9f261cafa0fa8a1ab"
);
```
