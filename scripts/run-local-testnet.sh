#!/usr/bin/env bash
# Start a four-node AgentCoin local testnet (authorities alice, bob, charlie, dave) on this machine.
#
# Usage: scripts/run-local-testnet.sh [--check]
#   (no flag)  run until Ctrl-C; RPC on 127.0.0.1:9944 (alice), :9945 (bob), :9946 (charlie),
#              :9947 (dave); Prometheus metrics on 127.0.0.1:9615-9618
#   --check    run for 40 s, then require every node to be at best height >= 20 and finalized
#              height >= 15, and alice's acbft_finalized_number metric to be above 0; then exit
# Environment: AC_NODE (node binary, default target/release/ac-node or target/debug/ac-node),
#              AC_TESTNET_DIR (base directory for databases and logs, default a temp dir).
# Requirements: bash, curl, python3.
set -euo pipefail

repo_root="$(cd "$(dirname "$0")/.." && pwd)"
node="${AC_NODE:-}"
if [[ -z "$node" ]]; then
  for candidate in "$repo_root/target/release/ac-node" "$repo_root/target/debug/ac-node"; do
    [[ -x "$candidate" ]] && node="$candidate" && break
  done
fi
[[ -x "$node" ]] || { echo "ac-node binary not found; build it or set AC_NODE" >&2; exit 1; }
base="${AC_TESTNET_DIR:-$(mktemp -d)}"
mkdir -p "$base"
check=false
[[ "${1:-}" == "--check" ]] && check=true

pids=()
cleanup() { for pid in "${pids[@]}"; do kill "$pid" 2>/dev/null || true; done; wait 2>/dev/null || true; }
trap cleanup EXIT INT TERM

rpc() { # rpc <port> <method>
  curl -sf -H 'Content-Type: application/json' \
    -d "{\"id\":1,\"jsonrpc\":\"2.0\",\"method\":\"$2\",\"params\":[]}" "http://127.0.0.1:$1"
}

start() { # start <name> <index> [bootnode]
  local name="$1" i="$2" boot="${3:-}"
  local args=(--chain local --validator --dev-key "$name" --base-path "$base/$name"
    --rpc-port "$((9944 + i))" --listen-addr "/ip4/127.0.0.1/tcp/$((30333 + i))"
    --prometheus-port "$((9615 + i))" --no-telemetry --no-mdns --name "$name"
    # Local testing only: generate the libp2p identity key on first start.
    --unsafe-force-node-key-generation)
  [[ -n "$boot" ]] && args+=(--bootnodes "$boot")
  "$node" "${args[@]}" >"$base/$name.log" 2>&1 &
  pids+=("$!")
}

start alice 0
for _ in $(seq 1 120); do
  peer="$(rpc 9944 system_localPeerId 2>/dev/null | python3 -c 'import json,sys; print(json.load(sys.stdin)["result"])' 2>/dev/null || true)"
  [[ -n "$peer" ]] && break
  sleep 1
done
[[ -n "${peer:-}" ]] || { echo "alice did not start; see $base/alice.log" >&2; exit 1; }
bootnode="/ip4/127.0.0.1/tcp/30333/p2p/$peer"
start bob 1 "$bootnode"
start charlie 2 "$bootnode"
start dave 3 "$bootnode"
echo "local testnet running: logs and databases in $base"

height() {
  rpc "$1" chain_getHeader | python3 -c 'import json,sys; print(int(json.load(sys.stdin)["result"]["number"], 16))'
}

finalized() {
  local hash
  hash="$(rpc "$1" chain_getFinalizedHead | python3 -c 'import json,sys; print(json.load(sys.stdin)["result"])')"
  curl -sf -H 'Content-Type: application/json' \
    -d "{\"id\":1,\"jsonrpc\":\"2.0\",\"method\":\"chain_getHeader\",\"params\":[\"$hash\"]}" \
    "http://127.0.0.1:$1" |
    python3 -c 'import json,sys; print(int(json.load(sys.stdin)["result"]["number"], 16))'
}

if $check; then
  sleep 40
  for port in 9944 9945 9946 9947; do
    h="$(height "$port" || echo 0)"
    f="$(finalized "$port" || echo 0)"
    echo "node on $port at height $h, finalized $f"
    [[ "$h" -ge 20 ]] || { echo "node on $port is behind; see logs in $base" >&2; exit 1; }
    [[ "$f" -ge 15 ]] || { echo "node on $port is not finalizing; see logs in $base" >&2; exit 1; }
  done
  metric="$(curl -sf http://127.0.0.1:9615/metrics | awk '/^acbft_finalized_number/ {print $2}')"
  echo "alice acbft_finalized_number $metric"
  [[ "${metric:-0}" -gt 0 ]] || { echo "AC-BFT metrics missing; see logs in $base" >&2; exit 1; }
  echo "local testnet check passed"
else
  wait
fi
