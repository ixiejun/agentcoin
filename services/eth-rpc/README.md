> 🌐 **English** | [简体中文](README.zh-CN.md)

# ac-eth-rpc

The AgentCoin Ethereum JSON-RPC adapter (m4-evm, spec `evm/eth-rpc`). It lets Foundry and other
Ethereum tools read contract state, blocks, receipts and logs, and relays contract transactions
that the AgentCoin wallet has already signed with ML-DSA. It talks to the chain only through a
node's public JSON-RPC, holds no keys and signs nothing.

Licence: `GPL-3.0-or-later` (a service, decision D47).

## Running

```sh
ac-eth-rpc --node-url http://127.0.0.1:9944 --listen 127.0.0.1:8545
cast chain-id --rpc-url http://127.0.0.1:8545      # 4403
```

| Option | Default | Meaning |
|---|---|---|
| `--node-url` | `http://127.0.0.1:9944` | the node's JSON-RPC |
| `--listen` | `127.0.0.1:8545` | HTTP and WebSocket address (loopback only by default) |
| `--index-depth` | `10000` | blocks before start-up indexed for transactions and receipts |
| `--max-log-range` | `1000` | largest block span of one `eth_getLogs` query |
| `--max-connections` | `100` | simultaneous connections |
| `--log-level` | `info` | `error`, `warn`, `info` or `debug` |

## Methods

| Method | Answer |
|---|---|
| `web3_clientVersion`, `net_version`, `eth_chainId` | adapter version; chain ID 4403 (`0x1133`) |
| `eth_syncing`, `eth_accounts` | always `false`; always `[]` |
| `eth_blockNumber` | best block |
| `eth_gasPrice`, `eth_maxPriorityFeePerGas`, `eth_feeHistory` | fee of one gas unit (10,000 ps of weight); priority fee 0; the same base fee for every block |
| `eth_getBalance`, `eth_getTransactionCount`, `eth_getCode`, `eth_getStorageAt` | contract state at the block |
| `eth_call`, `eth_estimateGas` | dry run on the block's state, any caller; a revert is error 3 with the revert data |
| `eth_getBlockByNumber`, `eth_getBlockByHash` | the Ethereum view of the block; `transactions` lists its contract transactions |
| `eth_getTransactionByHash`, `eth_getTransactionReceipt` | contract transactions in the index, otherwise `null` |
| `eth_getLogs` | contract events by block range or block hash, address(es) and topics |
| `eth_sendRawTransaction` | relays an ML-DSA-signed AgentCoin contract transaction |
| `eth_sendTransaction`, `eth_sign*`, `personal_*` | refused, pointing to `ac-wallet evm` |

Any other method returns `-32601` (method not found).

## Block tags

`latest` and `pending` are the best block; `safe` and `finalized` are the latest AC-BFT finalized
block; `earliest` is block 0. A block's `hash` is the value a contract's `blockhash` returns for
it; both that hash and the native block hash find the block.

## Transactions

Only AgentCoin extrinsics signed with ML-DSA whose call is a contract call or an EVM deployment
are relayed; the returned hash is the node's (BLAKE3) transaction hash. RLP-encoded Ethereum
transactions of any type, other calls and undecodable bytes are refused before anything reaches
the node. Build and sign contract transactions with `ac-wallet evm deploy|send`, or sign
externally with `ac-wallet evm raw` and submit the bytes here.

Transaction objects carry `v`, `r` and `s` of zero: authorization is the ML-DSA signature.
Receipts take `status` from `ExtrinsicSuccess`/`ExtrinsicFailed`, `contractAddress` from
`Instantiated`, logs from `ContractEmitted`, and report `gasUsed = ceil(fee / gasPrice)` with
`effectiveGasPrice = gasPrice`, so `gasUsed × effectiveGasPrice` exceeds the fee paid by less
than one gas unit's price.

```rust
use ac_eth_rpc::chain::{REF_TIME_PER_GAS, Refusal, decode_contract_tx, gas_of};

// An EIP-1559 transaction (type byte, then an RLP list) is not an AgentCoin transaction.
let eip1559 = [0x02, 0xf8, 0x50, 0x82, 0x11, 0x33];
assert_eq!(decode_contract_tx(&eip1559), Err(Refusal::NotAgentCoin));

// Gas figures translate weight, rounded up.
assert_eq!(gas_of(REF_TIME_PER_GAS + 1), 2);
```

## Index

The adapter keeps in memory the blocks from `--index-depth` before start-up onwards: native and
Ethereum block hashes and the location of every contract transaction. It follows the best chain,
rewinds when the chain reorganizes, and rebuilds everything on restart (nothing is persisted).
Transactions outside the index answer `null`.

## Error codes

| Code | When |
|---|---|
| `3` | `eth_call` or `eth_estimateGas` reverted; `data` holds the revert data |
| `-32000` | refused raw transaction or signing method |
| `-32005` | `eth_getLogs` block span above `--max-log-range` |
| `-32601` | method not supported |
| `-32602` | invalid parameters or unknown block |
| `-32603` | the node could not answer |

## Logging

Only method names, durations and error codes are logged, never parameters or results.

## Non-goals

- Ethereum-signed (secp256k1) transactions, `eth_sendTransaction` and node-side signing.
- Filters and subscriptions (`eth_newFilter`, `eth_subscribe`), tracing and debugging methods.
- Persistent indexing and archive queries beyond the node's own state pruning.
