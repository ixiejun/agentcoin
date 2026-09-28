#!/usr/bin/env bash
# Enforce the licence zones (decision D47, spec engineering/ci-quality-gates):
#
# - GPL zone (`GPL-3.0-or-later`): node/, services/, clients/wallet-cli/, tests/, scripts/.
# - Permissive zone (`MIT OR Apache-2.0`): everything else, including any new directory.
#
# Two rules are checked for every workspace crate:
# 1. A permissive-zone crate's normal + build dependency closure contains no crate that can only
#    be used under a GPL-family licence — this includes AgentCoin's own GPL-zone crates. A
#    licence expression with a non-GPL alternative (e.g. "Apache-2.0 OR GPL-3.0") is accepted.
# 2. Every crate declares exactly its zone's licence expression in `Cargo.toml`.
#
# It also checks that the AC-BFT protocol core (node/consensus/ac-bft/src/protocol*) uses no
# node-client (`sc-*`) crate, so it stays a pure state machine that light clients and formal
# tools can reuse (design D1 of m2-finality).
#
# Usage: scripts/check-license-boundary.sh [--manifest-path <Cargo.toml>] [--self-test]
# Requirements: bash, cargo, python3.
set -euo pipefail

repo_root="$(cd "$(dirname "$0")/.." && pwd)"
manifest="$repo_root/Cargo.toml"

# Directories (relative to the workspace root) that form the GPL zone.
GPL_ZONES="node services clients/wallet-cli tests scripts"
PERMISSIVE_LICENSE="MIT OR Apache-2.0"
GPL_LICENSE="GPL-3.0-or-later"

# Prints "<name>\t<zone>\t<declared licence>" for every workspace crate.
list_packages() {
  local manifest="$1"
  cargo metadata --format-version 1 --no-deps --manifest-path "$manifest" |
    python3 -c '
import json, os, sys
root = os.path.realpath(sys.argv[1])
zones = sys.argv[2].split()
meta = json.load(sys.stdin)
for p in meta["packages"]:
    rel = os.path.relpath(os.path.realpath(os.path.dirname(p["manifest_path"])), root)
    gpl = any(rel == z or rel.startswith(z + "/") for z in zones)
    print(p["name"], "gpl" if gpl else "permissive", p.get("license") or "", sep="\t")
' "$(dirname "$manifest")" "$GPL_ZONES"
}

check() {
  local manifest="$1"
  local failed=0
  local name zone license expected offenders
  while IFS=$'\t' read -r name zone license; do
    if [[ "$zone" == gpl ]]; then expected="$GPL_LICENSE"; else expected="$PERMISSIVE_LICENSE"; fi
    if [[ "$license" != "$expected" ]]; then
      echo "licence zone violated: $name ($zone zone) declares \"$license\", expected \"$expected\"" >&2
      failed=1
    fi
    [[ "$zone" == permissive ]] || continue
    offenders="$(cargo tree --manifest-path "$manifest" -p "$name" --all-features -e normal,build \
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
      echo "licence boundary violated: $name (permissive zone) depends on GPL-licensed crates:" >&2
      echo "$offenders" | sed 's/^/  /' >&2
      failed=1
    fi
  done < <(list_packages "$manifest")
  return "$failed"
}

# --- self-test ---------------------------------------------------------------------------------

# Writes a fixture crate: fixture_crate <workspace> <dir> <licence> [<dependency line>...]
fixture_crate() {
  local ws="$1" dir="$2" license="$3"
  shift 3
  mkdir -p "$ws/$dir/src"
  {
    echo "[package]"
    echo "name = \"fixture-$(echo "$dir" | tr '/' '-')\""
    echo 'version = "0.1.0"'
    echo 'edition = "2024"'
    echo "license = \"$license\""
    echo
    echo "[dependencies]"
    printf '%s\n' "$@"
  } >"$ws/$dir/Cargo.toml"
  echo "" >"$ws/$dir/src/lib.rs"
}

# Builds a passing baseline workspace in "$1": a library, a pallet, the node and a service; the
# node and the service use a GPL SDK crate. `sc-chain-spec-derive`
# (GPL-3.0-or-later WITH Classpath-exception-2.0) is a small SDK crate.
fixture_baseline() {
  local ws="$1"
  rm -rf "$ws"
  mkdir -p "$ws"
  cat >"$ws/Cargo.toml" <<'TOML'
[workspace]
resolver = "3"
members = ["crates/lib", "pallets/p", "clients/wallet-cli", "node", "services/svc"]
TOML
  fixture_crate "$ws" crates/lib "$PERMISSIVE_LICENSE"
  fixture_crate "$ws" pallets/p "$PERMISSIVE_LICENSE"
  fixture_crate "$ws" clients/wallet-cli "$GPL_LICENSE"
  fixture_crate "$ws" node "$GPL_LICENSE" 'sc-chain-spec-derive = "12.0"'
  fixture_crate "$ws" services/svc "$GPL_LICENSE" 'sc-chain-spec-derive = "12.0"'
}

# Expects `check` to fail for the named reason: the failure must come from a licence rule (its
# stderr names the violation), not from an unrelated cargo error.
expect_fail() {
  local what="$1" ws="$2" pattern="$3" err
  if err="$(check "$ws/Cargo.toml" 2>&1 >/dev/null)"; then
    echo "self-test failed: $what was not detected" >&2
    return 1
  fi
  if ! grep -q "$pattern" <<<"$err"; then
    echo "self-test failed: $what failed for an unexpected reason:" >&2
    echo "$err" | sed 's/^/  /' >&2
    return 1
  fi
}

self_test() {
  local tmp
  tmp="$(mktemp -d)"
  trap 'rm -rf "$tmp"' RETURN
  local ws="$tmp/ws"

  # Scenario: the node and a service may use GPL dependencies.
  fixture_baseline "$ws"
  if ! check "$ws/Cargo.toml"; then
    echo "self-test failed: GPL dependencies in the GPL zone were rejected" >&2
    return 1
  fi

  # Scenario: a library crate pulls in a GPL dependency.
  fixture_baseline "$ws"
  fixture_crate "$ws" crates/lib "$PERMISSIVE_LICENSE" 'sc-chain-spec-derive = "12.0"'
  expect_fail "a GPL dependency under crates/" "$ws" "fixture-crates-lib (permissive zone) depends on GPL"

  # Scenario: a pallet depends on the repository's own GPL crate.
  fixture_baseline "$ws"
  fixture_crate "$ws" pallets/p "$PERMISSIVE_LICENSE" \
    'fixture-clients-wallet-cli = { path = "../../clients/wallet-cli" }'
  expect_fail "a pallet depending on clients/wallet-cli" "$ws" "fixture-clients-wallet-cli v0.1.0  \\[GPL-3.0-or-later\\]"

  # Scenario: a new top-level directory defaults to the permissive zone.
  fixture_baseline "$ws"
  sed -i 's|"services/svc"]|"services/svc", "newdir/thing"]|' "$ws/Cargo.toml"
  fixture_crate "$ws" newdir/thing "$PERMISSIVE_LICENSE" 'sc-chain-spec-derive = "12.0"'
  expect_fail "a GPL dependency in a new top-level directory" "$ws" "fixture-newdir-thing (permissive zone) depends on GPL"

  # Scenario: declared licence does not match the zone (both directions).
  fixture_baseline "$ws"
  fixture_crate "$ws" node "MIT" 'sc-chain-spec-derive = "12.0"'
  expect_fail "a node crate declaring MIT" "$ws" "fixture-node (gpl zone) declares \"MIT\""
  fixture_baseline "$ws"
  fixture_crate "$ws" crates/lib "$GPL_LICENSE"
  expect_fail "a crates/ crate declaring GPL-3.0-or-later" "$ws" "fixture-crates-lib (permissive zone) declares \"GPL-3.0-or-later\""

  echo "licence boundary self-test passed"
}

# The AC-BFT protocol core must not reference any `sc_*` crate.
check_pure_protocol() {
  local core="$repo_root/node/consensus/ac-bft/src"
  local hits
  hits="$(grep -rnE '\bsc_[a-z_]+(::|\b)' "$core/protocol.rs" "$core/protocol" || true)"
  if [ -n "$hits" ]; then
    echo "the AC-BFT protocol core must not use node-client (sc-*) crates:" >&2
    echo "$hits" >&2
    return 1
  fi
}

case "${1:-}" in
  --self-test) self_test ;;
  --manifest-path) check "$2" && echo "licence boundary ok" ;;
  "") check "$manifest" && check_pure_protocol && echo "licence boundary ok" ;;
  *) echo "usage: $0 [--manifest-path <Cargo.toml>] [--self-test]" >&2; exit 2 ;;
esac
