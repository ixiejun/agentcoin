#!/usr/bin/env bash
# Fetch official test vectors for ac-crypto, verify upstream checksums and write
# deterministic, filtered subsets to crates/ac-crypto/tests/vectors/.
#
# Usage: scripts/fetch-test-vectors.sh
# Requirements: bash, curl, jq, sha256sum.
# Re-running must produce byte-identical files (checked in CI via `git diff --exit-code`).
set -euo pipefail

repo_root="$(cd "$(dirname "$0")/.." && pwd)"
out="$repo_root/crates/ac-crypto/tests/vectors"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

# Upstream sources, pinned to commits so the checksums below stay valid.
acvp_commit="975de31eb83d87039ec88934fdc47d8c312b892d"
acvp_base="https://raw.githubusercontent.com/usnistgov/ACVP-Server/$acvp_commit/gen-val/json-files"
xwing_commit="984c2f7a93b8f8d8f8073ebb53f9f4ce50b5babd"
xwing_url="https://raw.githubusercontent.com/dconnolly/draft-connolly-cfrg-xwing-kem/$xwing_commit/spec/test-vectors.json"

# name|url|sha256 of the upstream file
sources=(
  "mldsa-keygen|$acvp_base/ML-DSA-keyGen-FIPS204/internalProjection.json|e67ee6540d40e11506c3c4e3b1f79fc1cefcd49820db99fc61f87cc8ba463baf"
  "mldsa-siggen|$acvp_base/ML-DSA-sigGen-FIPS204/internalProjection.json|72dcaf5f69853ca267ccd16af9cb40949786aca0fcfbf05d1ebeba132b93af22"
  "mldsa-sigver|$acvp_base/ML-DSA-sigVer-FIPS204/internalProjection.json|47cdd6314c7f746d02421ffcba89d4dbc7bb875ac49e07a029fdfc26fba55437"
  "mlkem-keygen|$acvp_base/ML-KEM-keyGen-FIPS203/internalProjection.json|d7a62a2c3476957f56dd8d24f9004ea6776ccfe995ffe71a65bb9506dc9c7b1b"
  "mlkem-encapdecap|$acvp_base/ML-KEM-encapDecap-FIPS203/internalProjection.json|a556952ce869bb89c3a3196a701dad89647c193a34c86eafb61a9d710d5b810f"
  "xwing|$xwing_url|409efe197550b22985b4a0419418a0c5f2c2b193426c55bd998399ec8d3e614d"
)

fetch() {
  local name="$1" url="$2" sum="$3"
  curl -sSfL --retry 4 -o "$tmp/$name.json" "$url"
  local got
  got="$(sha256sum "$tmp/$name.json" | cut -d' ' -f1)"
  if [[ "$got" != "$sum" ]]; then
    echo "checksum mismatch for $name: expected $sum, got $got" >&2
    exit 1
  fi
  # SOURCES.md must record the same checksum (keeps documentation and script in sync).
  if ! grep -q "$sum" "$out/SOURCES.md"; then
    echo "checksum for $name is not recorded in $out/SOURCES.md" >&2
    exit 1
  fi
}

for entry in "${sources[@]}"; do
  IFS='|' read -r name url sum <<<"$entry"
  fetch "$name" "$url" "$sum"
done

# Keep at most N cases per parameter set, preserving upstream order.
per_set=10
# ML-DSA keyGen: seed -> (pk, sk), first 10 per parameter set.
jq -S --argjson n "$per_set" \
  '[.testGroups[] | .parameterSet as $p | .tests[:$n][] | {parameterSet: $p, tcId, seed, pk, sk}]' \
  "$tmp/mldsa-keygen.json" >"$out/ml_dsa_keygen.json"

# ML-DSA sigGen: deterministic, external interface, pure (no pre-hash), first 10 per set.
jq -S --argjson n "$per_set" \
  '[.testGroups[]
    | select(.deterministic == true and .signatureInterface == "external" and .preHash == "pure" and .externalMu == false)
    | .parameterSet as $p | .tests[:$n][] | {parameterSet: $p, tcId, sk, message, context, signature}]' \
  "$tmp/mldsa-siggen.json" >"$out/ml_dsa_siggen.json"

# ML-DSA sigVer: external interface, pure; per parameter set every expected-pass case plus
# the first 7 expected-fail cases (keeps both outcomes while staying under the size budget).
jq -S \
  '[.testGroups[]
    | select(.signatureInterface == "external" and .preHash == "pure" and .externalMu == false)
    | .parameterSet as $p
    | ([.tests[] | select(.testPassed)] + [.tests[] | select(.testPassed | not)][:7]) | sort_by(.tcId)[]
    | {parameterSet: $p, tcId, pk, message, context, signature, testPassed, reason}]' \
  "$tmp/mldsa-sigver.json" >"$out/ml_dsa_sigver.json"

# ML-KEM-768 keyGen: (d, z) -> (ek, dk), first 10.
jq -S --argjson n "$per_set" \
  '[.testGroups[] | select(.parameterSet == "ML-KEM-768") | .tests[:$n][] | {tcId, d, z, ek, dk}]' \
  "$tmp/mlkem-keygen.json" >"$out/ml_kem_768_keygen.json"

# ML-KEM-768 encapsulation (ek, m -> c, k), first 10; decapsulation (dk, c -> k), all.
jq -S --argjson n "$per_set" \
  '{encapsulation: [.testGroups[] | select(.parameterSet == "ML-KEM-768" and .function == "encapsulation") | .tests[:$n][] | {tcId, ek, m, c, k}],
    decapsulation: [.testGroups[] | select(.parameterSet == "ML-KEM-768" and .function == "decapsulation") | .dk as $gdk | .tests[] | {tcId, dk: (.dk // $gdk), c, k}]}' \
  "$tmp/mlkem-encapdecap.json" >"$out/ml_kem_768_encapdecap.json"

# X-Wing: all vectors from the specification repository.
jq -S '.' "$tmp/xwing.json" >"$out/xwing_draft06.json"

echo "test vectors written to $out"
du -ch "$out"/*.json | tail -1
