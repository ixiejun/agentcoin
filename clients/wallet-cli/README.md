> 🌐 **English** | [简体中文](README.zh-CN.md)

# ac-wallet

AgentCoin's command-line wallet (M1), and the external signer for EVM contracts (M4).

- Keys: 24-word BIP-39 mnemonic over 256-bit entropy; account keys are derived per algorithm
  and index (`agentcoin 2026-09 wallet-key v1`). The default account algorithm is ML-DSA-44.
- The wallet file stores only the Argon2id + XChaCha20-Poly1305 encrypted entropy and public
  bookkeeping; passphrases come from the terminal or `--password-file`, never from arguments.
- Addresses are bech32m `atc1…` strings; a single mistyped character is rejected.
- Transactions are v5 general transactions authorized by `PqAuthorize`; the first transaction
  of an account carries its public key automatically.
- `rotate` moves to the next derivation index (optionally another ML-DSA parameter set); the
  address never changes.

## Usage

```bash
ac-wallet new --wallet alice.json            # prints the address and, once, the mnemonic
ac-wallet import --wallet restored.json      # reads the mnemonic from standard input
ac-wallet address --wallet alice.json
ac-wallet balance --wallet alice.json --node http://127.0.0.1:9944
ac-wallet transfer --wallet alice.json --to atc1... --amount 1.5
ac-wallet rotate --wallet alice.json --alg ml-dsa-65
```

`scripts/wallet-smoke.sh` runs these commands against a development node.

## EVM contracts

Contract transactions are AgentCoin transactions signed with the account's ML-DSA key; Foundry
builds and reads, the wallet signs. The account's EVM address is `keccak256(account)[12..]`.

```bash
ac-wallet evm address --wallet alice.json    # EIP-55, e.g. 0x05a0…9F70
forge build                                  # in a Foundry project
ac-wallet evm deploy --wallet alice.json --artifact out/ERC20.sol/ERC20.json \
  --constructor "constructor(string,string,uint256)" Token TKN 1000000000000000000000
ac-wallet evm send --wallet alice.json --to 0x… --sig "transfer(address,uint256)" 0x… 5
cast call 0x… "balanceOf(address)(uint256)" 0x… --rpc-url http://127.0.0.1:8545   # ac-eth-rpc
```

| Command | What it does |
|---|---|
| `evm address` | the account's EVM address (EIP-55) |
| `evm deploy` | deploys a `forge build` artifact (`--artifact`) or init code (`--bytecode`); constructor arguments as text after `--constructor <signature>` or encoded with `--constructor-args` |
| `evm send` | calls a contract with `--sig <signature>` and arguments, or `--data` |
| `evm raw deploy`, `evm raw send` | signs without submitting; prints the hash and the bytes for `eth_sendRawTransaction` |
| `evm broadcast --file run-latest.json` | sends the transactions of a `forge script` simulation, one at a time |
| `evm sign-message` | signs a message under `agentcoin/evm-verify/v1` for the `pq_verify` precompile |

Before signing, the wallet dry-runs the transaction on the latest state and sets the weight
limit to the requirement plus 20% and the storage-deposit limit to the peak plus 10%
(`--ref-time-limit`, `--proof-size-limit`, `--deposit-limit` override them). A transaction the
dry run shows reverting is not submitted; the reason is printed (`--force` submits anyway).
Values (`--value`, `--deposit-limit`) are in ATC.

`evm broadcast` needs a simulation made with the wallet's address as sender
(`forge script <script> --sender <evm address> --rpc-url <adapter>`, without `--broadcast`), and
stops at the first transaction that fails or that creates a contract elsewhere than the script
predicted; later transactions are not sent. Note that a native deployment advances the account
nonce by two (once for the transaction, once by `pallet-revive`), so a script can predict only
its first deployment correctly.

## Library example

```rust
use ac_wallet::amount::{format_atc, parse_atc};

let units = parse_atc("1.5")?;
assert_eq!(format_atc(units), "1.5 ATC");

// Call data from a Solidity signature and textual arguments.
use ac_wallet::evm::abi::Signature;
let transfer = Signature::parse("transfer(address,uint256)")?;
assert_eq!(transfer.selector(), [0xa9, 0x05, 0x9c, 0xbb]);
let data = transfer.calldata(&["0x00000000000000000000000000000000000000aa".into(), "1000".into()])?;
assert_eq!(data.len(), 4 + 32 + 32);
# Ok::<(), anyhow::Error>(())
```
