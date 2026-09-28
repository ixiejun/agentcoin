> 🌐 **English** | [简体中文](README.zh-CN.md)

# pallet-evm-support

EVM support for the AgentCoin runtime (m4-evm). `pallet-revive` runs EVM contracts; this pallet
keeps it inside AgentCoin's rules:

- **Fee payer** (`EvmPayer`, `SetEvmPayer`): the signer of a contract transaction
  (`Revive::call`, `Revive::instantiate_with_code`, or either wrapped in
  `Revive::dispatch_as_fallback_account`) is recorded as the payer for the dispatch. The
  transaction extension carries no data; the payer is removed after dispatch and again at the end
  of every block. Dry runs set it with `Pallet::with_payer`.
- **Non-minting currency** (`ReviveCurrency<C, P, B>`): the currency `pallet-revive` sees. It
  forwards the fungible API to `C` (the `Balances` pallet) except where revive would create ATC:
  `mint_into` (a new contract's existential deposit) transfers from the payer instead, `issue`
  withdraws from the payer, burns go to `B` (the `Emission` pallet, so they are counted in
  `Emission::TotalBurned`), `set_balance` never raises a balance, and `restore`, `shelve`,
  `burn_held` and `burn_all_held` fail. No EVM path can mint (decision D9; the node's issuance
  check would reject such a block).
- **Revive's own account**: revive holds code deposits on its pallet account and upstream mints
  an existential deposit into it at genesis. This pallet gives the account a provider reference
  at genesis and on runtime upgrade instead, so it exists with a zero balance.
- **PQ precompiles** (`PqPrecompiles`), Solidity ABI, benchmarked weights charged before any
  work, no state, no account:

  | Precompile | Address | Interface |
  |---|---|---|
  | `pq_verify` | `0x000000000000000000000000000000000a010000` | `verify(uint8 alg, bytes publicKey, bytes message, bytes signature) returns (bool)` — ML-DSA-44/65/87 under the fixed context `agentcoin/evm-verify/v1`; any failure is `false` |
  | `blake3` | `0x000000000000000000000000000000000a020000` | `hash(bytes data) returns (bytes32)` |
  | `stark_verify` | `0x000000000000000000000000000000000a100000` | reserved (D27): every call reverts |

  Address `0x…0a030000` is kept for `poseidon2`, pending the outcome of its research gate.

## Features

| Feature | Default | Purpose |
|---|---|---|
| `std` | yes | Native build. |
| `try-runtime` | no | Propagates `try-runtime` to FRAME and `pallet-revive`. |

## Example

The extension adds no bytes to a transaction:

```rust
use parity_scale_codec::Encode;
use pallet_evm_support::SetEvmPayer;

// `SetEvmPayer` is generic over the runtime; its encoding is empty for any runtime type.
struct AnyRuntime;
assert!(SetEvmPayer::<AnyRuntime>::new().encode().is_empty());
```
