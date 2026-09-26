#!/usr/bin/env bash
# Enforce the licence boundary (decision D37, spec engineering/ci-quality-gates):
# GPL-family licences may appear only in the dependency closure of the node client (`node/`)
# and of the node-driven end-to-end tests (`tests/e2e/`). Every other workspace crate —
# `crates/`, `pallets/`, `runtime/`, `clients/` — must have a GPL-free normal + build closure.
# A licence expression with a non-GPL alternative (e.g. "Apache-2.0 OR GPL-3.0") is accepted.
#
# Usage: scripts/check-license-boundary.sh [--manifest-path <Cargo.toml>] [--self-test]
# Requirements: bash, cargo, python3.
set -euo pipefail

repo_root="$(cd "$(dirname "$0")/.." && pwd)"
manifest="$repo_root/Cargo.toml"

check() {
  local manifest="$1"
  local root
  root="$(dirname "$manifest")"
  local packages
  packages="$(cargo metadata --format-version 1 --no-deps --manifest-path "$manifest" |
    python3 -c '
import json, os, sys
root = os.path.realpath(sys.argv[1])
meta = json.load(sys.stdin)
for p in meta["packages"]:
    rel = os.path.relpath(os.path.realpath(os.path.dirname(p["manifest_path"])), root)
    exempt = rel == "node" or rel.startswith("node/") or rel.startswith("tests/e2e")
    if not exempt:
        print(p["name"])
' "$root")"
  local failed=0
  for pkg in $packages; do
    local offenders
    offenders="$(cargo tree --manifest-path "$manifest" -p "$pkg" --all-features -e normal,build \
      --target all --prefix none --format '{p}|{l}' |
      python3 -c '
import re, sys
bad = set()
for line in sys.stdin:
    name, _, lic = line.strip().partition("|")
    alternatives = re.split(r"\s+OR\s+|/", lic.strip())
    if lic and all("GPL" in alt for alt in alternatives):
        bad.add(name.split(" (")[0] + "  [" + lic + "]")
print("\n".join(sorted(bad)))
')"
    if [[ -n "$offenders" ]]; then
      echo "licence boundary violated: $pkg depends on GPL-licensed crates:" >&2
      echo "$offenders" | sed 's/^/  /' >&2
      failed=1
    fi
  done
  return "$failed"
}

self_test() {
  local tmp
  tmp="$(mktemp -d)"
  trap 'rm -rf "$tmp"' RETURN
  mkdir -p "$tmp/crates/lib/src" "$tmp/node/src"
  cat >"$tmp/Cargo.toml" <<'TOML'
[workspace]
resolver = "3"
members = ["crates/lib", "node"]
TOML
  # sc-chain-spec-derive (GPL-3.0-or-later WITH Classpath-exception-2.0) is a small SDK crate.
  for dir in crates/lib node; do
    cat >"$tmp/$dir/Cargo.toml" <<TOML
[package]
name = "fixture-$(basename "$dir")"
version = "0.1.0"
edition = "2024"
license = "MIT"

[dependencies]
sc-chain-spec-derive = "12.0"
TOML
    echo "" >"$tmp/$dir/src/lib.rs"
  done
  if check "$tmp/Cargo.toml" 2>/dev/null; then
    echo "self-test failed: a GPL dependency under crates/ was not detected" >&2
    return 1
  fi
  # Removing the library's dependency leaves only the (exempt) node depending on GPL code.
  sed -i "/sc-chain-spec-derive/d" "$tmp/crates/lib/Cargo.toml"
  if ! check "$tmp/Cargo.toml"; then
    echo "self-test failed: a GPL dependency under node/ was rejected" >&2
    return 1
  fi
  echo "licence boundary self-test passed"
}

case "${1:-}" in
  --self-test) self_test ;;
  --manifest-path) check "$2" && echo "licence boundary ok" ;;
  "") check "$manifest" && echo "licence boundary ok" ;;
  *) echo "usage: $0 [--manifest-path <Cargo.toml>] [--self-test]" >&2; exit 2 ;;
esac
