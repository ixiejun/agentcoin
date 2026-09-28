#!/usr/bin/env bash
# Enforce the licence zones (decision D47, spec engineering/ci-quality-gates):
#
# - GPL zone (`GPL-3.0-or-later`): node/, services/, clients/wallet-cli/, tests/, scripts/.
# - Permissive zone (`MIT OR Apache-2.0`): everything else, including any new directory.
#
# Solidity sources follow the same zones, with contracts/acceptance/ also in the GPL zone:
# 3. A permissive-zone `.sol` file carries an SPDX identifier that is not GPL-only, and imports
#    nothing from the GPL zone.
# 4. Third-party sources copied into a GPL-zone `lib/` directory are recorded in a `SOURCE.md`
#    in that directory that lists each file's SHA-256 (they keep their upstream licence and need
#    no SPDX line).
#
# Three rules are checked for every workspace crate:
# 1. The distributed build of a permissive-zone crate (every feature except the internal ones in
#    INTERNAL_FEATURES) has a normal + build dependency closure with no crate that can only be
#    used under a GPL-family licence — this includes AgentCoin's own GPL-zone crates. A licence
#    expression with a non-GPL alternative (e.g. "Apache-2.0 OR GPL-3.0") is accepted.
# 2. With every feature enabled, internal ones included, the only GPL-only crates allowed in that
#    closure are those listed in INTERNAL_GPL_ALLOW.
# 3. Every crate declares exactly its zone's licence expression in `Cargo.toml`.
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
# Solidity-only GPL zone (design D13 of m4-evm).
SOL_GPL_ZONES="$GPL_ZONES contracts/acceptance"
PERMISSIVE_LICENSE="MIT OR Apache-2.0"
GPL_LICENSE="GPL-3.0-or-later"
# Cargo features that only developers build; their artefacts are never distributed
# (change license-internal-features, design D1). Changing either list needs an OpenSpec change.
INTERNAL_FEATURES="runtime-benchmarks"
# GPL-only crates allowed in a permissive-zone closure, and only behind INTERNAL_FEATURES.
INTERNAL_GPL_ALLOW="pallet-revive-fixtures"

# Prints "<name>\t<zone>\t<declared licence>\t<distributed features>" for every workspace crate;
# the distributed features are all of the crate's features except INTERNAL_FEATURES, comma
# separated.
list_packages() {
  local manifest="$1"
  cargo metadata --format-version 1 --no-deps --manifest-path "$manifest" |
    python3 -c '
import json, os, sys
root = os.path.realpath(sys.argv[1])
zones = sys.argv[2].split()
internal = set(sys.argv[3].split())
meta = json.load(sys.stdin)
for p in meta["packages"]:
    rel = os.path.relpath(os.path.realpath(os.path.dirname(p["manifest_path"])), root)
    gpl = any(rel == z or rel.startswith(z + "/") for z in zones)
    features = ",".join(sorted(f for f in p["features"] if f not in internal))
    print(p["name"], "gpl" if gpl else "permissive", p.get("license") or "", features, sep="\t")
' "$(dirname "$manifest")" "$GPL_ZONES" "$INTERNAL_FEATURES"
}

# Reads `cargo tree --format "{p}|{l}"` output and prints each GPL-only crate not named in "$1"
# (a space-separated list), as "<crate> v<version>  [<licence>]".
gpl_offenders() {
  python3 -c '
import re, sys
allowed = set(sys.argv[1].split())
bad = set()
for line in sys.stdin:
    name, _, lic = line.strip().partition("|")
    alternatives = re.split(r"\s+OR\s+|/", lic.strip())
    package = name.split(" (")[0]
    if lic and all("GPL" in alt for alt in alternatives) and package.split(" v")[0] not in allowed:
        bad.add(package + "  [" + lic + "]")
print("\n".join(sorted(bad)))
' "$1"
}

# Prints the dependency closure of package "$2" as "{p}|{l}" lines; further arguments select
# features.
closure() {
  local manifest="$1" name="$2"
  shift 2
  cargo tree --manifest-path "$manifest" -p "$name" "$@" -e normal,build --target all \
    --prefix none --format '{p}|{l}'
}

check() {
  local manifest="$1"
  local failed=0
  local name zone license features expected offenders
  local -a selection
  while IFS=$'\t' read -r name zone license features; do
    if [[ "$zone" == gpl ]]; then expected="$GPL_LICENSE"; else expected="$PERMISSIVE_LICENSE"; fi
    if [[ "$license" != "$expected" ]]; then
      echo "licence zone violated: $name ($zone zone) declares \"$license\", expected \"$expected\"" >&2
      failed=1
    fi
    [[ "$zone" == permissive ]] || continue
    # Rule 1: the distributed build, where not even INTERNAL_GPL_ALLOW is accepted.
    selection=()
    [[ -z "$features" ]] || selection=(--features "$features")
    offenders="$(closure "$manifest" "$name" "${selection[@]}" | gpl_offenders "")"
    if [[ -n "$offenders" ]]; then
      echo "licence boundary violated: $name (permissive zone) depends on GPL-licensed crates in the distributed build (features other than: $INTERNAL_FEATURES):" >&2
      echo "$offenders" | sed 's/^/  /' >&2
      failed=1
    fi
    # Rule 2: internal features may add only the registered crates.
    offenders="$(closure "$manifest" "$name" --all-features | gpl_offenders "$INTERNAL_GPL_ALLOW")"
    if [[ -n "$offenders" ]]; then
      echo "licence boundary violated: $name (permissive zone) depends on GPL-licensed crates not registered in INTERNAL_GPL_ALLOW:" >&2
      echo "$offenders" | sed 's/^/  /' >&2
      failed=1
    fi
  done < <(list_packages "$manifest")
  return "$failed"
}

# Checks every Solidity source under "$1" (build output and caches excluded).
check_solidity() {
  local root="$1"
  find "$root" \( -name .git -o -name target -o -name out -o -name cache -o -name node_modules \
    -o -name broadcast \) -prune -o -name '*.sol' -print0 |
    python3 -c '
import hashlib, os, re, sys
root = os.path.realpath(sys.argv[1])
zones = sys.argv[2].split()
files = [f for f in sys.stdin.read().split("\0") if f]

def rel(path):
    return os.path.relpath(os.path.realpath(path), root)

def gpl_zone(path):
    r = rel(path)
    return next((z for z in zones if r == z or r.startswith(z + "/")), None)

def project_root(path):
    d = os.path.dirname(os.path.realpath(path))
    while d.startswith(root):
        if os.path.exists(os.path.join(d, "foundry.toml")):
            return d
        if d == root:
            break
        d = os.path.dirname(d)
    return root

failed = False
def fail(message):
    global failed
    failed = True
    print(message, file=sys.stderr)

spdx = re.compile(r"SPDX-License-Identifier:\s*([^\s*][^*\n]*?)\s*(?:\*/)?\s*$", re.M)
imports = re.compile(r"^\s*import\s+(?:[^;]*?\s+from\s+)?[\x22\x27]([^\x22\x27]+)[\x22\x27]", re.M)
for f in sorted(files):
    text = open(f, encoding="utf-8", errors="replace").read()
    zone = gpl_zone(f)
    if zone is None:
        m = spdx.search(text)
        if not m:
            fail(f"licence zone violated: {rel(f)} (permissive zone) has no SPDX-License-Identifier")
        else:
            expr = m.group(1).strip()
            alternatives = re.split(r"\s+OR\s+", expr.strip("()"))
            if all("GPL" in alt for alt in alternatives):
                fail(f"licence zone violated: {rel(f)} (permissive zone) is licensed \"{expr}\"")
        for target in imports.findall(text):
            if target.startswith("."):
                candidates = [os.path.join(os.path.dirname(f), target)]
            else:
                candidates = [os.path.join(project_root(f), target), os.path.join(root, target)]
            for c in candidates:
                if gpl_zone(c) is not None:
                    fail(f"licence boundary violated: {rel(f)} (permissive zone) imports {target} from the GPL zone")
                    break
    else:
        parts = rel(f).split("/")
        if "lib" in parts:
            lib_dir = os.path.join(root, *parts[: parts.index("lib") + 1])
            source = os.path.join(lib_dir, "SOURCE.md")
            if not os.path.exists(source):
                fail(f"licence record missing: {rel(f)} is vendored under {rel(lib_dir)} without a SOURCE.md")
                continue
            digest = hashlib.sha256(open(f, "rb").read()).hexdigest()
            if digest not in open(source, encoding="utf-8").read():
                fail(f"licence record out of date: {rel(source)} lacks the SHA-256 of {rel(f)} ({digest})")
sys.exit(1 if failed else 0)
' "$root" "$SOL_GPL_ZONES"
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

# Writes a crate outside the fixture workspace that stands in for a third-party dependency:
# fixture_external <dir> <name> <licence>
fixture_external() {
  local dir="$1" name="$2" license="$3"
  mkdir -p "$dir/src"
  printf '[package]\nname = "%s"\nversion = "0.1.0"\nedition = "2024"\nlicense = "%s"\n' \
    "$name" "$license" >"$dir/Cargo.toml"
  echo "" >"$dir/src/lib.rs"
}

# Makes crates/lib in workspace "$1" depend on the external crate at "$2" (named "$3") only
# through feature "$4".
fixture_feature_dep() {
  local ws="$1" dir="$2" name="$3" feature="$4"
  fixture_crate "$ws" crates/lib "$PERMISSIVE_LICENSE" \
    "$name = { path = \"$dir\", optional = true }" \
    "" "[features]" "$feature = [\"dep:$name\"]"
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

# Adds Solidity sources to the fixture workspace in "$1": a permissive contract (MIT OR
# Apache-2.0) and the acceptance project with an upstream GPL file (no SPDX line) recorded in
# lib/SOURCE.md, as with the official Uniswap V2.
fixture_solidity() {
  local ws="$1"
  rm -rf "$ws/contracts"
  mkdir -p "$ws/contracts/src" "$ws/contracts/acceptance/lib/v2-core" "$ws/contracts/acceptance/script"
  touch "$ws/contracts/foundry.toml" "$ws/contracts/acceptance/foundry.toml"
  printf '// SPDX-License-Identifier: MIT OR Apache-2.0\npragma solidity 0.8.28;\nimport {B} from "./B.sol";\ncontract A {}\n' \
    >"$ws/contracts/src/A.sol"
  printf '/* SPDX-License-Identifier: MIT OR Apache-2.0 */\npragma solidity 0.8.28;\ncontract B {}\n' \
    >"$ws/contracts/src/B.sol"
  printf 'pragma solidity =0.5.16;\ncontract Pair {}\n' >"$ws/contracts/acceptance/lib/v2-core/Pair.sol"
  printf '// SPDX-License-Identifier: GPL-3.0-or-later\nimport "../lib/v2-core/Pair.sol";\n' \
    >"$ws/contracts/acceptance/script/Deploy.s.sol"
  printf '# Sources\n\n| File | SHA-256 |\n|---|---|\n| v2-core/Pair.sol | %s |\n' \
    "$(sha256sum "$ws/contracts/acceptance/lib/v2-core/Pair.sol" | cut -d' ' -f1)" \
    >"$ws/contracts/acceptance/lib/SOURCE.md"
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

  # Internal features (change license-internal-features): a registered GPL-only crate is
  # accepted behind a registered internal feature only.
  local internal="${INTERNAL_FEATURES%% *}" allowed="${INTERNAL_GPL_ALLOW%% *}"
  fixture_external "$tmp/ext/allowed" "$allowed" "GPL-3.0-only"
  fixture_external "$tmp/ext/other" "fixture-gpl-other" "GPL-3.0-only"

  # Scenario: the registered crate appears only behind the internal feature.
  fixture_baseline "$ws"
  fixture_feature_dep "$ws" "$tmp/ext/allowed" "$allowed" "$internal"
  if ! check "$ws/Cargo.toml"; then
    echo "self-test failed: $allowed behind $internal was rejected" >&2
    return 1
  fi

  # Scenario: the registered crate appears behind an ordinary feature.
  fixture_baseline "$ws"
  fixture_feature_dep "$ws" "$tmp/ext/allowed" "$allowed" "std"
  expect_fail "$allowed behind std" "$ws" "depends on GPL-licensed crates in the distributed build"

  # Scenario: the internal feature brings in an unregistered GPL crate.
  fixture_baseline "$ws"
  fixture_feature_dep "$ws" "$tmp/ext/other" "fixture-gpl-other" "$internal"
  expect_fail "an unregistered GPL crate behind $internal" "$ws" "fixture-gpl-other v0.1.0  \\[GPL-3.0-only\\]"

  # Scenario: an unregistered feature brings in the registered crate.
  fixture_baseline "$ws"
  fixture_feature_dep "$ws" "$tmp/ext/allowed" "$allowed" "try-runtime"
  expect_fail "$allowed behind try-runtime" "$ws" "depends on GPL-licensed crates in the distributed build"

  # Scenario: a permissive-zone contract with a GPL identifier, without one, or importing from
  # the acceptance project.
  fixture_solidity "$ws"
  if ! check_solidity "$ws"; then
    echo "self-test failed: the Solidity baseline was rejected" >&2
    return 1
  fi
  expect_sol_fail() {
    local what="$1" pattern="$2" err
    if err="$(check_solidity "$ws" 2>&1 >/dev/null)"; then
      echo "self-test failed: $what was not detected" >&2
      return 1
    fi
    if ! grep -q "$pattern" <<<"$err"; then
      echo "self-test failed: $what failed for an unexpected reason:" >&2
      echo "$err" | sed 's/^/  /' >&2
      return 1
    fi
  }
  fixture_solidity "$ws"
  printf '// SPDX-License-Identifier: GPL-3.0\npragma solidity 0.8.28;\n' >"$ws/contracts/src/Bad.sol"
  expect_sol_fail "a GPL-3.0 contract under contracts/src" 'contracts/src/Bad.sol (permissive zone) is licensed "GPL-3.0"'
  fixture_solidity "$ws"
  printf 'pragma solidity 0.8.28;\n' >"$ws/contracts/src/Bad.sol"
  expect_sol_fail "a contract without SPDX under contracts/src" "contracts/src/Bad.sol (permissive zone) has no SPDX"
  fixture_solidity "$ws"
  printf '// SPDX-License-Identifier: MIT\npragma solidity 0.8.28;\nimport {Pair} from "../acceptance/lib/v2-core/Pair.sol";\n' \
    >"$ws/contracts/src/Bad.sol"
  expect_sol_fail "a contract importing from contracts/acceptance" "imports ../acceptance/lib/v2-core/Pair.sol from the GPL zone"
  fixture_solidity "$ws"
  printf 'contract Changed {}\n' >>"$ws/contracts/acceptance/lib/v2-core/Pair.sol"
  expect_sol_fail "a vendored file that differs from its record" "lacks the SHA-256 of contracts/acceptance/lib/v2-core/Pair.sol"
  fixture_solidity "$ws"
  rm "$ws/contracts/acceptance/lib/SOURCE.md"
  expect_sol_fail "vendored sources without SOURCE.md" "without a SOURCE.md"

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
  "") check "$manifest" && check_solidity "$repo_root" && check_pure_protocol &&
    echo "licence boundary ok" ;;
  *) echo "usage: $0 [--manifest-path <Cargo.toml>] [--self-test]" >&2; exit 2 ;;
esac
