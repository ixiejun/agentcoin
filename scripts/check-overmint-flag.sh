#!/usr/bin/env bash
# Keep the test-only runtime faults out of real builds (m3-economics 8.1, m3-pos 9.1, spec
# engineering/ci-quality-gates). Each fault is compiled only with its compiler flag, which only
# its own test runtime's build.rs may pass (to its own WASM build):
# - `ac_test_overmint`: over-minting (tests/overmint-runtime);
# - `ac_test_early_switch`: switching to PoS early (tests/early-switch-runtime).
# This check fails if:
# - a flag appears anywhere else that can set compiler flags or compile code (other build
#   scripts, cargo config, workflows, other sources);
# - any crate depends on a test runtime other than as a dev-dependency.
#
# Usage: scripts/check-overmint-flag.sh [--root <dir>] [--self-test]
# Requirements: bash, git, grep, cargo, python3.
set -euo pipefail

repo_root="$(cd "$(dirname "$0")/.." && pwd)"

# Files allowed to name each flag: where the fault and the test version live, the one build
# script that sets it, this check, and documentation.
allowed_overmint='^(pallets/emission/src/lib\.rs|pallets/emission/Cargo\.toml|runtime/src/lib\.rs|runtime/Cargo\.toml|tests/overmint-runtime/.*|scripts/check-overmint-flag\.sh|.*\.md)$'
allowed_early_switch='^(pallets/validator-set/src/lib\.rs|pallets/validator-set/Cargo\.toml|runtime/src/lib\.rs|runtime/Cargo\.toml|tests/early-switch-runtime/.*|scripts/check-overmint-flag\.sh|.*\.md)$'

# scan <root> <flag> <allowed regex> <test runtime directory>
scan() {
  local root="$1" flag="$2" allowed="$3" home="$4" bad=0 file
  while IFS= read -r file; do
    if [[ ! "$file" =~ $allowed ]]; then
      echo "$flag appears in $file: only $home may set it" >&2
      bad=1
    fi
  done < <(cd "$root" && grep -rIl --exclude-dir=target --exclude-dir=.git --exclude-dir=openspec \
    "$flag" . | sed 's|^\./||' | sort)
  return "$bad"
}

scan_all() {
  local bad=0
  scan "$1" ac_test_overmint "$allowed_overmint" tests/overmint-runtime || bad=1
  scan "$1" ac_test_early_switch "$allowed_early_switch" tests/early-switch-runtime || bad=1
  return "$bad"
}

check_dependents() {
  cargo metadata --format-version 1 --no-deps --manifest-path "$1/Cargo.toml" | python3 -c '
import json, sys
test_runtimes = {"ac-overmint-runtime", "ac-early-switch-runtime"}
bad = False
for p in json.load(sys.stdin)["packages"]:
    for d in p["dependencies"]:
        if d["name"] in test_runtimes and d["kind"] != "dev":
            kind = d["kind"] or "normal"
            print(p["name"], "depends on", d["name"], "as a", kind, "dependency; only dev-dependencies are allowed", file=sys.stderr)
            bad = True
sys.exit(1 if bad else 0)
'
}

if [[ "${1:-}" == "--self-test" ]]; then
  tmp="$(mktemp -d)"
  trap 'rm -rf "$tmp"' EXIT
  for pair in "ac_test_overmint:overmint-runtime" "ac_test_early_switch:early-switch-runtime"; do
    flag="${pair%%:*}"
    crate="${pair#*:}"
    rm -rf "$tmp"/* "$tmp/.cargo"
    mkdir -p "$tmp/.cargo" "$tmp/tests/$crate"
    echo "fn main() { let _ = \"--cfg $flag\"; }" > "$tmp/tests/$crate/build.rs"
    scan_all "$tmp" || { echo "self-test: the allowed build script of $flag was rejected" >&2; exit 1; }
    printf '[build]\nrustflags = ["--cfg", "%s"]\n' "$flag" > "$tmp/.cargo/config.toml"
    if scan_all "$tmp" 2>/dev/null; then
      echo "self-test: a cargo config setting $flag was not detected" >&2
      exit 1
    fi
  done
  echo "test runtime flag check self-test ok"
  exit 0
fi

root="$repo_root"
if [[ "${1:-}" == "--root" ]]; then
  root="$2"
fi
scan_all "$root"
check_dependents "$root"
echo "test runtime flag check ok"
