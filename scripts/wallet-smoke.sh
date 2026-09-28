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
echo "wallet smoke test passed"
