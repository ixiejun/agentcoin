#!/usr/bin/env bash
# Keep the test-only over-minting fault out of real builds (m3-economics 8.1, spec
# engineering/ci-quality-gates). The fault is compiled only with `--cfg ac_test_overmint`, which
# only tests/overmint-runtime/build.rs may pass (to its own WASM build). This check fails if:
# - the flag appears anywhere else that can set compiler flags or compile code (other build
#   scripts, cargo config, workflows, other sources);
# - any crate depends on ac-overmint-runtime other than as a dev-dependency.
#
# Usage: scripts/check-overmint-flag.sh [--root <dir>] [--self-test]
# Requirements: bash, git, grep, cargo, python3.
set -euo pipefail

repo_root="$(cd "$(dirname "$0")/.." && pwd)"

# Files allowed to name the flag: where the fault and its version live, the one build script
# that sets it, this check, and documentation.
allowed='^(pallets/emission/src/lib\.rs|pallets/emission/Cargo\.toml|runtime/src/lib\.rs|runtime/Cargo\.toml|tests/overmint-runtime/.*|scripts/check-overmint-flag\.sh|.*\.md)$'

scan() {
  local root="$1" bad=0 file
  while IFS= read -r file; do
    if [[ ! "$file" =~ $allowed ]]; then
      echo "ac_test_overmint appears in $file: only tests/overmint-runtime may set it" >&2
      bad=1
    fi
  done < <(cd "$root" && grep -rIl --exclude-dir=target --exclude-dir=.git --exclude-dir=openspec \
    'ac_test_overmint' . | sed 's|^\./||' | sort)
  return "$bad"
}

check_dependents() {
  cargo metadata --format-version 1 --no-deps --manifest-path "$1/Cargo.toml" | python3 -c '
import json, sys
bad = False
for p in json.load(sys.stdin)["packages"]:
    for d in p["dependencies"]:
        if d["name"] == "ac-overmint-runtime" and d["kind"] != "dev":
            kind = d["kind"] or "normal"
            print(p["name"], "depends on ac-overmint-runtime as a", kind, "dependency; only dev-dependencies are allowed", file=sys.stderr)
            bad = True
sys.exit(1 if bad else 0)
'
}

if [[ "${1:-}" == "--self-test" ]]; then
  tmp="$(mktemp -d)"
  trap 'rm -rf "$tmp"' EXIT
  mkdir -p "$tmp/.cargo" "$tmp/tests/overmint-runtime"
  echo 'fn main() { let _ = "--cfg ac_test_overmint"; }' > "$tmp/tests/overmint-runtime/build.rs"
  scan "$tmp" || { echo "self-test: the allowed build script was rejected" >&2; exit 1; }
  printf '[build]\nrustflags = ["--cfg", "ac_test_overmint"]\n' > "$tmp/.cargo/config.toml"
  if scan "$tmp" 2>/dev/null; then
    echo "self-test: a cargo config setting the flag was not detected" >&2
    exit 1
  fi
  echo "overmint flag check self-test ok"
  exit 0
fi

root="$repo_root"
if [[ "${1:-}" == "--root" ]]; then
  root="$2"
fi
scan "$root"
check_dependents "$root"
echo "overmint flag check ok"
