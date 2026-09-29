#!/usr/bin/env bash
# Runs the ac-wallet README commands against a fresh development node (task 8.5 of m1-pq-chain).
#
# Usage: scripts/wallet-smoke.sh
# Environment: AC_NODE, AC_WALLET (binaries; default target/debug/*).
# Requirements: bash, a built ac-node and ac-wallet.
set -euo pipefail

repo_root="$(cd "$(dirname "$0")/.." && pwd)"
node="${AC_NODE:-$repo_root/target/debug/ac-node}"
wallet="${AC_WALLET:-$repo_root/target/debug/ac-wallet}"
work="$(mktemp -d)"
url="http://127.0.0.1:19933"

"$node" --dev --tmp --rpc-port 19933 --listen-addr /ip4/127.0.0.1/tcp/30399 \
  --no-telemetry --no-prometheus --no-mdns >"$work/node.log" 2>&1 &
node_pid=$!
trap 'kill $node_pid 2>/dev/null || true; wait 2>/dev/null || true' EXIT
for _ in $(seq 1 120); do
  curl -sf -H 'Content-Type: application/json' \
    -d '{"id":1,"jsonrpc":"2.0","method":"chain_getHeader","params":[]}' "$url" >/dev/null && break
  sleep 1
done

echo "wallet-smoke-password" >"$work/password"
pw=(--password-file "$work/password")

# 1. Import the public development wallet (endowed on dev chains) and create a fresh wallet.
echo "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon art" |
  "$wallet" import --wallet "$work/dev.json" "${pw[@]}"
"$wallet" new --wallet "$work/fresh.json" "${pw[@]}" 2>/dev/null
dev_addr="$("$wallet" address --wallet "$work/dev.json")"
fresh_addr="$("$wallet" address --wallet "$work/fresh.json")"
"$wallet" key-info --wallet "$work/dev.json" "${pw[@]}"
# The dev account's EVM address (EIP-55; keccak256(account)[12..], m4-evm 6.6).
[[ "$("$wallet" evm address --wallet "$work/dev.json")" == "0x05a036924C72687962e062f30378CD17907b9F70" ]]

# 2. Transfers: dev wallet -> fresh wallet (first transaction of the dev wallet), then back.
"$wallet" transfer --wallet "$work/dev.json" "${pw[@]}" --node "$url" --to "$fresh_addr" --amount 5
"$wallet" transfer --wallet "$work/fresh.json" "${pw[@]}" --node "$url" --to "$dev_addr" --amount 1.25
"$wallet" balance --wallet "$work/fresh.json" --node "$url"

# 3. A mistyped address is rejected before anything is submitted.
bad="${fresh_addr%?}q"
[[ "$bad" == "$fresh_addr" ]] && bad="${fresh_addr%?}p"
if "$wallet" transfer --wallet "$work/dev.json" "${pw[@]}" --node "$url" --to "$bad" --amount 1 2>/dev/null; then
  echo "a mistyped address was accepted" >&2
  exit 1
fi

# 4. Rotate the fresh wallet to ML-DSA-65 and keep using the same address.
"$wallet" rotate --wallet "$work/fresh.json" "${pw[@]}" --node "$url" --alg ml-dsa-65
[[ "$("$wallet" address --wallet "$work/fresh.json")" == "$fresh_addr" ]]
"$wallet" transfer --wallet "$work/fresh.json" "${pw[@]}" --node "$url" --to "$dev_addr" --amount 1
"$wallet" balance --wallet "$work/fresh.json" --node "$url"
# 5. The inference market (m5-market-registry): rate, a model, a provider, a gateway, escrow and
#    a voucher checked against the chain.
"$wallet" market rate --node "$url"
printf '{"name":"smoke","arch":"llama","quant":"bf16","shards":["0x%s"]}' "$(printf '11%.0s' $(seq 32))" >"$work/model.json"
model="$("$wallet" market model id --file "$work/model.json")"
"$wallet" market model register --wallet "$work/dev.json" "${pw[@]}" --node "$url" --file "$work/model.json"
kem="0x01$(printf '07%.0s' $(seq 1216))"
"$wallet" transfer --wallet "$work/dev.json" "${pw[@]}" --node "$url" --to "$fresh_addr" --amount 2000
"$wallet" market provider register --wallet "$work/fresh.json" "${pw[@]}" --node "$url" \
  --tier t2 --endpoint https://provider.example --kem-key "$kem" --model "$model:0.1:0.2"
"$wallet" market providers --node "$url" --model "$model" | grep -q "provider: $fresh_addr"
"$wallet" market gateway register --wallet "$work/fresh.json" "${pw[@]}" --node "$url" \
  --endpoint https://gateway.example --fee-bps 300
"$wallet" market escrow deposit --wallet "$work/dev.json" "${pw[@]}" --node "$url" \
  --gateway "$fresh_addr" --amount 5
voucher="$("$wallet" market voucher sign --wallet "$work/dev.json" "${pw[@]}" --node "$url" \
  --gateway "$fresh_addr" --usd 0.5 | sed -n 's/^voucher: //p')"
"$wallet" market voucher check --node "$url" --voucher "$voucher" | grep -q "valid: yes"
"$wallet" market channel --wallet "$work/dev.json" --node "$url" --gateway "$fresh_addr"

# 6. Work settlement (m5-work-settlement): the fresh account is both provider and gateway, so one
# signature completes the receipt; 5M prompt tokens at $0.1 per million match the $0.5 voucher.
"$wallet" market receipt new --wallet "$work/fresh.json" "${pw[@]}" --node "$url" \
  --gateway "$fresh_addr" --provider "$fresh_addr" --model "$model" \
  --in-tokens 5000000 --out-tokens 0 --out "$work/receipt.json" | grep 'fee: \$0.5' >/dev/null
"$wallet" market receipt check --node "$url" --file "$work/receipt.json" | grep "receipt valid" >/dev/null
"$wallet" market report submit --wallet "$work/fresh.json" "${pw[@]}" --node "$url" \
  --receipt "$work/receipt.json" --voucher "$voucher" | grep "^root: " >/dev/null
"$wallet" market report show --node "$url" --id 0 | grep "settled: 0.5 ATC" >/dev/null
"$wallet" market work --node "$url" --wallet "$work/fresh.json"
"$wallet" market claim --wallet "$work/dev.json" "${pw[@]}" --node "$url" \
  --account "$fresh_addr" | grep "nothing to claim" >/dev/null
echo "wallet smoke test passed"
