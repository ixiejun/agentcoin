#!/usr/bin/env bash
# Start a three-node AgentCoin local testnet (authorities alice, bob, charlie) on this machine.
#
# Usage: scripts/run-local-testnet.sh [--check]
#   (no flag)  run until Ctrl-C; RPC on 127.0.0.1:9944 (alice), :9945 (bob), :9946 (charlie)
#   --check    run for 40 s, then require every node to be at height >= 20 and exit
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
    --no-telemetry --no-prometheus --no-mdns --name "$name"
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
echo "local testnet running: logs and databases in $base"

height() {
  rpc "$1" chain_getHeader | python3 -c 'import json,sys; print(int(json.load(sys.stdin)["result"]["number"], 16))'
}

if $check; then
  sleep 40
  for port in 9944 9945 9946; do
    h="$(height "$port" || echo 0)"
    echo "node on $port at height $h"
    [[ "$h" -ge 20 ]] || { echo "node on $port is behind; see logs in $base" >&2; exit 1; }
  done
  echo "local testnet check passed"
else
  wait
fi
