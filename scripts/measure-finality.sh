#!/usr/bin/env bash
# Measure AC-BFT finality latency on local networks of 4, 7 and 10 authorities (release build).
#
# Usage: scripts/measure-finality.sh [seconds]   (default 60 s of stable operation per size)
# For each size, prints the number of blocks and the P50, P95 and maximum time from importing a
# block to finalizing it on the first node, observed over WebSocket subscriptions. Fails if a
# size finalizes nothing or its P95 exceeds 3 s (spec consensus/ac-bft "测量脚本").
# Environment: AC_E2E_LOGS (directory for node logs, default the system temp dir).
# Requirements: a machine with at least 4 cores.
set -euo pipefail

repo_root="$(cd "$(dirname "$0")/.." && pwd)"
seconds="${1:-60}"
cd "$repo_root"
cargo build --release -p ac-node
cargo build --release -p ac-e2e --bin measure-finality
export AC_NODE="$repo_root/target/release/ac-node"

status=0
for n in 4 7 10; do
  line="$(target/release/measure-finality "$n" "$seconds")"
  echo "$line"
  p95="$(sed -E 's/.*P95 ([0-9.]+) s.*/\1/' <<<"$line")"
  python3 -c "import sys; sys.exit(0 if float('$p95') <= 3.0 else 1)" ||
    { echo "P95 above 3 s with $n nodes" >&2; status=1; }
done
exit "$status"
