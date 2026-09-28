#!/usr/bin/env bash
# Benchmark a pallet and regenerate its weights file (AGENT.md §8: weights come from benchmarks).
#
# Usage: scripts/benchmark-pallet.sh <pallet_name> <output weights.rs> [<template.hbs>]
#   e.g. scripts/benchmark-pallet.sh pallet_pq_accounts pallets/pq-accounts/src/weights.rs
#   The default template writes a pallet's own `weights.rs`; for an upstream pallet whose
#   weights live in the runtime, pass scripts/frame-weight-template-runtime.hbs, e.g.
#   scripts/benchmark-pallet.sh pallet_revive runtime/src/weights/pallet_revive.rs \
#     scripts/frame-weight-template-runtime.hbs
#   (pallet_revive also needs its fixtures' toolchain: see scripts/revive-fixtures-toolchain.sh).
# Environment: OMNI_BENCHER (path to frame-omni-bencher; default: download the pinned release).
# Requirements: bash, cargo, curl, sha256sum.
set -euo pipefail

pallet="$1"
output="$2"
repo_root="$(cd "$(dirname "$0")/.." && pwd)"
template="${3:-$repo_root/scripts/frame-weight-template.hbs}"

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

# Only the WASM runtime is benchmarked, and it is always built optimized: a release native
# build of the whole runtime would add gigabytes of artifacts for nothing.
WASM_BUILD_TYPE=release cargo build -p ac-runtime --features runtime-benchmarks
wasm="$repo_root/target/debug/wbuild/ac-runtime/ac_runtime.compact.wasm"

"$bencher" v1 benchmark pallet \
  --runtime "$wasm" \
  --genesis-builder-preset development \
  --pallet "$pallet" --extrinsic '*' \
  --steps 50 --repeat 20 \
  --template "$template" \
  --output "$output"
cargo fmt --all
