> 🌐 **English** | [简体中文](README.zh-CN.md)

# ac-wallet

AgentCoin's command-line wallet (M1).

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

## Library example

```rust
use ac_wallet::amount::{format_atc, parse_atc};

let units = parse_atc("1.5")?;
assert_eq!(format_atc(units), "1.5 ATC");
# Ok::<(), anyhow::Error>(())
```
