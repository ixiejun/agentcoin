> 🌐 **English** | [简体中文](README.zh-CN.md)

# ac-runtime

The AgentCoin WASM runtime: post-quantum chain with scheduled emission and nominated proof of stake
(M1–M3), EVM contracts (M4) and the inference-market registry (M5).

| Index | Pallet | Notes |
|---|---|---|
| 0 | `System` | `Hashing = Blake3Hasher`: block hashes, extrinsics root and state root are BLAKE3-256 (D35) |
| 1 | `Timestamp` | 1 s blocks |
| 2 | `AuraPq` | ML-DSA-65 authority set from genesis |
| 3 | `Balances` | ATC, 18 decimals, existential deposit 0.001 ATC; the pallet name is a published well-known key (constitution layer 1) |
| 4 | `TransactionPayment` | weight + length fees; of every fee and tip 20% (rounded down) goes to the block author, the rest is burned through `Emission` |
| 5 | `PqAccounts` | public-key registry, `rotate_key` |
| 6 | `ValidatorSet` | epoch-based authority set, PoA roster and the one-way PoA → PoS switch; name published (`Phase`, `QualifiedSince`, `PoaAuthorities`, `TransitionParams`, `EpochLength`) |
| 7 | `Offences` | double-signing reports |
| 8 | `RandomnessCr` | commit–reveal randomness |
| 9 | `Emission` | scheduled emission and the burn ledger; name published (`Emission::TotalBurned`, `Emission::EpochLength`) |
| 10 | `TreasuryDual` | community grants, the locked holder treasury, the vesting floor |
| 11 | `PoaCouncil` | `pallet-collective` instance of the PoA multisig; name published (`PoaCouncil::Members`) |
| 12 | `PoaAdmin` | threshold origin, `dispatch_as_root`, members and threshold |
| 13 | `StakingPos` | candidates, nominations, unbonding queue, elections, rewards, slashing; name published (`Ledger`, `Candidates`) |
| 14 | `Revive` | `pallet-revive`: EVM bytecode contracts only, chain ID 4403, 1 wei = 1 smallest ATC unit; index published (`ac_primitives::evm::REVIVE_PALLET_INDEX`) |
| 15 | `EvmSupport` | contract fee payer, the non-minting currency adapter for `Revive`, PQ precompiles |
| 16 | `RefRate` | ATC/USD reference rate, set by the PoA administration within ±20% per day |
| 17 | `ModelRegistry` | content-addressed model registration with a storage deposit |
| 18 | `Providers` | inference providers: dollar-denominated stake, heartbeats, serviceability; slash/jail interface for M6 only |
| 19 | `Gateways` | inference gateways: stake and a fee capped at 5% |
| 20 | `Credits` | transparent credits: escrow channels and cumulative ML-DSA vouchers (`Credit` interface, D20) |
| 21 | `Work` | work reports, burn and payment split, held payments, challenge period, claims; the `WorkSource` and `MarketPayout` of `Emission` |

Transactions are v5 `General` extrinsics whose first extension is `PqAuthorize` (D36); the legacy
`Signed` form cannot be decoded. The `transaction` module builds signed transactions for wallets
and tests: `authorized_extensions`, `implicit_from` (chain facts → implicit data), `payload` (the
32-byte payload to sign with `agentcoin/tx/v1`) and `assemble`.

Genesis presets `development` (authority alice) and `local_testnet` (alice, bob, charlie, dave)
endow the public development accounts alice, bob, charlie and dave; no other preset may allocate
ATC. Their emission epochs are 10 and 20 blocks, their PoA administration alice (threshold 1) and
alice, bob, charlie (threshold 2), with 20-block motions. Their switch parameters let a test
chain reach PoS within a minute or two: development — one qualified candidate, conditions from
height 20 held for 20 blocks, `K` = 5; local — three candidates, height 40, 40 blocks, `K` = 10;
both with 10% of the issuance staked, and unbonding and commission delays of tens of blocks.
Live chains use the constitution values (10%, 21, 63,115,200 and 604,800 blocks) and `K` = 100.

The security budget of emission goes to `StakingPos` (paid only in PoS), and double signing is
slashed through it. Runtime APIs: `StakingApi` (stake, candidates, latest election, switch
progress) besides the M1–M3 ones.

EVM contracts (m4-evm) are deployed and called with ordinary ML-DSA-signed transactions
(`Revive::instantiate_with_code`, `Revive::call`). The call filter `RuntimeCallFilter`
(`HolderTreasuryLock` and `EvmOnly`) refuses the Ethereum transaction entry points, every PolkaVM
path and address-mapping changes; every account gets its EVM address (`keccak256(account)[12..]`)
when it is created. Contracts never create issuance: the existential deposit revive would mint
into a new contract is paid by the transaction signer (`SetEvmPayer` extension), and revive's own
account exists through a provider reference. `ReviveApi` offers dry runs, storage and code
queries; Ethereum-signed payloads, code uploads and tracing are refused. The development runtime
WASM is 4.8 MB before compression.

The inference market (m5-market-registry) adds five pallets at indices 16–20. `MarketApi` offers
the reference rate, models, providers (and the serviceable providers of a model, paged by 256),
stake thresholds, gateways, credit channels and `check_voucher`, which applies the redemption
rules without changing state. In the `development` and `local_testnet` presets 1 ATC = 1 USD,
the rate interval, heartbeat interval and escrow withdrawal delay are 10 blocks and provider and
gateway unbonding 20 blocks (live chains: one day, 600 blocks, one day and seven days).

Work settlement (m5-work-settlement) adds `Work` at index 21. A gateway submits a work report
(the receipt Merkle root, per-(provider, model) totals, up to 16 vouchers and 128 entries); the
vouchers are redeemed into the gateway's account, 20% is burned through `Emission`, the gateway
fee and the provider shares stay held on the gateway until the report matures two emission epochs
later. The verified work of an epoch is `Emission`'s `WorkSource`; market emission is minted into
the `Work` pot (`MarketPayout`) and claimed together with the shares. A provider jailed before
maturity loses its pending shares and emission (`OnJail`). `WorkApi` offers the parameters,
reports, held payments, per-epoch work and lifetime totals. The presets keep reports for 20
epochs (live chains: 720). `spec_version` 6, `transaction_version` 6.

The administration origin is a `PoaCouncil` motion approved by at least the threshold of members
(`PoaAdmin::dispatch_as_root` runs calls as Root, e.g. `System::set_code`). The holder treasury
is locked: `HolderTreasuryLock` is both the base call filter and the Root call filter of
`dispatch_as_root`. Dust of reaped accounts is burned through `Emission` too, so the issuance
always changes by exactly minted minus burned.

## Features

| Feature | Default | Purpose |
|---|---|---|
| `std` | yes | Native runtime and the WASM builder. |
| `runtime-benchmarks` | no | Benchmarks of the included pallets, `pallet_revive` among them. |

`runtime-benchmarks` is an internal feature (see `LICENSE`): it pulls in `pallet-revive-fixtures`
(GPL-3.0-only), so a runtime built with it is only for generating weights on a developer machine
and is never distributed; CI checks that the release runtime exposes no benchmarking API
(`scripts/check-release-runtime.sh`). Compiling those fixtures needs a nightly RISC-V toolchain,
`solc` and `resolc`; set `SKIP_PALLET_REVIVE_FIXTURES=1` to build or test with `--all-features`
without them (revive's benchmarks then cannot run, nothing else changes).

## Example

```rust
use ac_runtime::transaction::{TxParams, authorized_extensions, implicit_from, payload, ChainContext};
use ac_runtime::{RuntimeCall, ATC};
use sp_runtime::generic::Era;

let context = ChainContext { genesis_hash: [1u8; 32].into(), spec_version: 1, transaction_version: 1 };
let params = TxParams { nonce: 0, tip: 0, era: Era::Immortal, era_birth_hash: context.genesis_hash };
let call = RuntimeCall::Balances(pallet_balances::Call::transfer_allow_death {
    dest: [2u8; 32].into(),
    value: ATC,
});
let extensions = authorized_extensions(&params);
let to_sign = payload(&call, &extensions, &implicit_from(&context, &params))?;
assert_eq!(to_sign.len(), 32);
# Ok::<(), ac_crypto::Error>(())
```
