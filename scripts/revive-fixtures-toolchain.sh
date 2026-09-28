#!/usr/bin/env bash
# Prepares what pallet-revive-fixtures needs to compile its benchmark contracts, so that
# `pallet_revive` can be benchmarked with scripts/benchmark-pallet.sh (change
# license-internal-features, design D6). For developer machines only: CI never compiles the
# fixtures (it sets SKIP_PALLET_REVIVE_FIXTURES=1).
#
# - Rust contracts are built for PolkaVM with `-Zbuild-std=core`; the fixtures' build script sets
#   RUSTC_BOOTSTRAP=1, so the pinned stable toolchain works once it has the `rust-src` component.
# - Solidity contracts are compiled with `solc` (EVM) and `resolc` (PolkaVM), pinned below by
#   SHA-256 and downloaded from their GitHub releases.
#
# Usage: eval "$(scripts/revive-fixtures-toolchain.sh)"   # puts solc and resolc on PATH
#        scripts/benchmark-pallet.sh pallet_revive …
# Environment: REVIVE_TOOLCHAIN_DIR (default: target/revive-toolchain).
# Requirements: bash, curl, sha256sum, rustup.
set -euo pipefail

repo_root="$(cd "$(dirname "$0")/.." && pwd)"
dir="${REVIVE_TOOLCHAIN_DIR:-$repo_root/target/revive-toolchain}"

# Some fixtures need solc ^0.8.30.
SOLC_URL="https://github.com/ethereum/solidity/releases/download/v0.8.30/solc-static-linux"
SOLC_SHA256="f3e987dc6ecebd4bd350c48edcbc320b46cf9e3109bd3fc3d88f1acaf4c428f7"
RESOLC_URL="https://github.com/paritytech/revive/releases/download/v1.4.0/resolc-x86_64-unknown-linux-musl"
RESOLC_SHA256="74e051ce27300c77e82e323f5c47acf2c452819b6a8d0eb21bf5b9111e7a686d"

fetch() { # fetch <url> <sha256> <path>
  if [[ ! -x "$3" ]] || ! echo "$2  $3" | sha256sum -c --status -; then
    curl -sSfL -o "$3.part" "$1"
    echo "$2  $3.part" | sha256sum -c --quiet - >&2
    chmod +x "$3.part"
    mv "$3.part" "$3"
  fi
}

mkdir -p "$dir"
fetch "$SOLC_URL" "$SOLC_SHA256" "$dir/solc"
fetch "$RESOLC_URL" "$RESOLC_SHA256" "$dir/resolc"
# The component belongs to the toolchain pinned in rust-toolchain.toml; nothing is switched.
(cd "$repo_root" && rustup component add rust-src >&2)
# The fixtures' build script does not rerun when SKIP_PALLET_REVIVE_FIXTURES changes, so outputs
# of an earlier skipped build (the usual `--all-features` clippy/test) would be reused as empty
# fixtures. Remove them so that the next benchmark build compiles the contracts.
find "$repo_root/target" -type d -path '*/build/pallet-revive-fixtures-*' -prune \
  -exec rm -rf {} + 2>/dev/null || true

echo "export PATH=\"$dir:\$PATH\""
