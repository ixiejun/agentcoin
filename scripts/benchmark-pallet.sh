#!/usr/bin/env bash
# Benchmark a pallet and regenerate its weights file (AGENT.md §8: weights come from benchmarks).
#
# Usage: scripts/benchmark-pallet.sh <pallet_name> <output weights.rs>
#   e.g. scripts/benchmark-pallet.sh pallet_pq_accounts pallets/pq-accounts/src/weights.rs
# Environment: OMNI_BENCHER (path to frame-omni-bencher; default: download the pinned release).
# Requirements: bash, cargo, curl, sha256sum.
set -euo pipefail

pallet="$1"
output="$2"
repo_root="$(cd "$(dirname "$0")/.." && pwd)"

# frame-omni-bencher of the Polkadot SDK release matching our crates, pinned by SHA-256.
release="polkadot-stable2606-2"
sha256="adb99802e75869d5ac0d93bc979551fa9207b74b09ffc8db81808ad08458a22e"
bencher="${OMNI_BENCHER:-}"
if [[ -z "$bencher" ]]; then
  bencher="$repo_root/target/frame-omni-bencher-$release"
  if [[ ! -x "$bencher" ]]; then
    curl -sSfL -o "$bencher" \
      "https://github.com/paritytech/polkadot-sdk/releases/download/$release/frame-omni-bencher"
    echo "$sha256  $bencher" | sha256sum -c -
    chmod +x "$bencher"
  fi
fi

cargo build -p ac-runtime --release --features runtime-benchmarks
wasm="$repo_root/target/release/wbuild/ac-runtime/ac_runtime.compact.compressed.wasm"

"$bencher" v1 benchmark pallet \
  --runtime "$wasm" \
  --genesis-builder-preset development \
  --pallet "$pallet" --extrinsic '*' \
  --steps 50 --repeat 20 \
  --template "$repo_root/scripts/frame-weight-template.hbs" \
  --output "$output"
cargo fmt --all
