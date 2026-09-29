> 🌐 **English** | [简体中文](README.zh-CN.md)

# pallet-credits

Transparent inference credits: the α implementation of the unified `Credit` interface (decision
D20; plan §5.2). Testnets pay for inference with these; β adds anonymous vouchers as a second
implementation of the same interface, and mainnet accepts only those.

## Channels and cumulative vouchers

A user escrows ATC with an active gateway (`deposit`). The escrow is held on the user's account
and recorded in the (user, gateway) **channel**:

| Field | Meaning |
|---|---|
| `escrow` | ATC still escrowed. |
| `number` | Channel number; incremented when the channel is reset. |
| `redeemed` | Cumulative dollars already redeemed. |
| `key` | Fingerprint of the **voucher key**: the user's registered key when the channel opened. |
| `pending_withdrawal`, `pending_key` | Requested withdrawal / key change and the block it takes effect. |

The user pays with **cumulative vouchers** instead of one voucher per request with a nonce:

```text
VoucherBody { genesis, user, gateway, channel, cumulative (micro-dollars) }
payload   = BLAKE3-derive_key("agentcoin 2026-09 voucher-payload v1", SCALE(body))
signature = ML-DSA.sign(voucher key, payload, context "agentcoin/voucher/v1")
```

Each voucher raises the total the gateway may take; the gateway keeps only its latest one. The
chain stores one counter per channel, so a replayed or older voucher pays nothing and no nonce
set grows.

## Redemption (settlement only)

`Credit::redeem(gateway, voucher, payee)` — there is no redemption transaction; settlement
(`pallet-work`, when a gateway submits a work report) calls it:

1. the voucher must name this chain's genesis, the redeeming gateway, the channel's current
   number, be signed with the channel's voucher key, and verify;
2. increment = cumulative − redeemed (zero pays nothing and is not an error);
3. the increment is converted at the reference rate **rounding down**; `min(value, escrow)` moves
   from the escrow to `payee` (an existing account);
4. `redeemed` becomes the voucher's cumulative amount even when the escrow was short; the rest
   is reported as a **shortfall**, borne by the gateway.

`check(voucher)` (and `MarketApi::check_voucher`) applies exactly the same rules without
changing anything, so a gateway knows before serving whether a voucher will redeem and whether
the escrow covers it. Both call `ac_primitives::market::voucher::check_voucher`.

## Calls

| Call | Effect |
|---|---|
| `deposit(gateway, amount)` | Escrows `amount` with an active gateway; opens the channel with the caller's current key. |
| `request_withdrawal(gateway, amount)` | Asks to withdraw after the delay (1 day live). Merges with a pending request and restarts the delay. Until then the gateway can still redeem. |
| `withdraw(gateway)` | After the delay, releases `min(requested, escrow)`. An emptied channel is **reset**: its number increments, `redeemed` returns to zero and earlier vouchers are void. |
| `request_key_change(gateway)` | After the delay, vouchers must be signed with the caller's current key. Rotating the account key alone never changes the voucher key, so a user cannot void vouchers a gateway already accepted. |

## Features

| Feature | Default | Purpose |
|---|---|---|
| `std` | yes | Native builds and tests. Disable it for the WASM runtime. |
| `runtime-benchmarks` | no | Benchmarks of every call and of one redemption (ML-DSA-87); enables deterministic signing in `ac-crypto` to build vouchers inside the runtime. |
| `try-runtime` | no | SDK try-runtime support. |

## Example

```rust
use ac_crypto::{SigAlg, sig::SigningKey};
use ac_primitives::market::voucher::{
    ChannelView, VOUCHER_CONTEXT, VoucherContext, check_voucher, key_fingerprint,
};
use ac_primitives::market::{AtcPerUsd, MicroUsd, SignedVoucher, VoucherBody};
use sp_core::H256;
use sp_runtime::AccountId32;

let key = SigningKey::from_seed(SigAlg::MlDsa44, &ac_crypto::dev_seed("alice").unwrap()).unwrap();
let (genesis, gateway) = (H256::repeat_byte(1), AccountId32::new([10; 32]));
// Half a dollar in total so far.
let body = VoucherBody {
    genesis,
    user: AccountId32::new([1; 32]),
    gateway: gateway.clone(),
    channel: 0,
    cumulative: MicroUsd(500_000),
};
let voucher = SignedVoucher {
    signature: key.sign_deterministic(&body.payload().unwrap(), VOUCHER_CONTEXT).unwrap(),
    public_key: key.public_key().unwrap(),
    body,
};
// The channel: 10 ATC escrowed, $0.20 already redeemed.
let channel = ChannelView {
    escrow: 10_000_000_000_000_000_000,
    number: 0,
    redeemed: MicroUsd(200_000),
    key: key_fingerprint(&key.public_key().unwrap()),
};
let ctx = VoucherContext {
    genesis: &genesis,
    gateway: &gateway,
    channel: Some(&channel),
    rate: Some(AtcPerUsd(1_000_000_000_000_000_000)), // 1 ATC = 1 USD
};
let check = check_voucher(&voucher, &ctx).unwrap();
assert_eq!(check.increment, MicroUsd(300_000)); // $0.30 more
assert_eq!(check.increment_atc, 300_000_000_000_000_000); // 0.3 ATC
assert!(check.covered);
```
