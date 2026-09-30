#!/usr/bin/env bash
# Fetch official test vectors for ac-crypto, verify upstream checksums and write
# deterministic, filtered subsets to crates/ac-crypto/tests/vectors/.
#
# Usage: scripts/fetch-test-vectors.sh
# Requirements: bash, curl, jq, python3, sha256sum.
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
# Argon2id reference KATs (the final tag is the RFC 9106 §5.3 vector); XChaCha20-Poly1305 draft-03
# source text from the author's repository; BIP-39 reference vectors. GitHub mirrors are used
# because they are pinned by commit and reachable from CI.
argon2_url="https://raw.githubusercontent.com/P-H-C/phc-winner-argon2/f57e61e19229e23c4445b85494dbf7c07de721cb/kats/argon2id"
xchacha_url="https://raw.githubusercontent.com/bikeshedders/xchacha-rfc/9c1dfb870155223360ef7c4818fdbbd41daaaf1c/draft-irtf-cfrg-xchacha-rfc-03.txt"
bip39_url="https://raw.githubusercontent.com/trezor/python-mnemonic/b57a5ad77a981e743f4167ab2f7927a55c1e82a8/vectors.json"
# Poseidon2: Plonky3's known-answer vector for its default Goldilocks width-12 instance, taken
# from the published crate (the instance has no vectors from the Poseidon2 authors; m4-evm D7).
p3_goldilocks_url="https://static.crates.io/crates/p3-goldilocks/p3-goldilocks-0.8.0.crate"
# ChaCha20-Poly1305: the RFC 8439 §2.8.2 AEAD vector as published in the tests of the RustCrypto
# crate we use (rfc-editor.org is not reachable from CI; the crate is pinned by its checksum).
chacha_url="https://static.crates.io/crates/chacha20poly1305/chacha20poly1305-0.11.0.crate"

# name|url|sha256 of the upstream file
sources=(
  "mldsa-keygen|$acvp_base/ML-DSA-keyGen-FIPS204/internalProjection.json|e67ee6540d40e11506c3c4e3b1f79fc1cefcd49820db99fc61f87cc8ba463baf"
  "mldsa-siggen|$acvp_base/ML-DSA-sigGen-FIPS204/internalProjection.json|72dcaf5f69853ca267ccd16af9cb40949786aca0fcfbf05d1ebeba132b93af22"
  "mldsa-sigver|$acvp_base/ML-DSA-sigVer-FIPS204/internalProjection.json|47cdd6314c7f746d02421ffcba89d4dbc7bb875ac49e07a029fdfc26fba55437"
  "mlkem-keygen|$acvp_base/ML-KEM-keyGen-FIPS203/internalProjection.json|d7a62a2c3476957f56dd8d24f9004ea6776ccfe995ffe71a65bb9506dc9c7b1b"
  "mlkem-encapdecap|$acvp_base/ML-KEM-encapDecap-FIPS203/internalProjection.json|a556952ce869bb89c3a3196a701dad89647c193a34c86eafb61a9d710d5b810f"
  "xwing|$xwing_url|409efe197550b22985b4a0419418a0c5f2c2b193426c55bd998399ec8d3e614d"
  "argon2id|$argon2_url|ba05643e504fc5778dda99e2d9f42ebe7d22ebb3923cc719fd591b1b14a8d28d"
  "xchacha|$xchacha_url|fa796b50265eeee383d40e82fed880267c7835e1b3d64c50c4f06162adaa1cfd"
  "bip39|$bip39_url|fa3b937b7cff9c9b8ecd3aa011faeb8d6dd67993174b72326e83f4de8fdb30f8"
  "p3-goldilocks|$p3_goldilocks_url|325a854e1232dd1ce270245d9c978b71897c286f072d2fa31740956a800e70c5"
  "chacha20poly1305|$chacha_url|9b89e1c441e926b9c82a8d023f6e1b7ae0adcfaa7d621814e4d60789bac751cb"
)

fetch() {
  local name="$1" url="$2" sum="$3"
  curl -sSfL --retry 4 -o "$tmp/$name.json" "$url"  # raw text for the non-JSON sources
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

# Argon2id: parameters, inputs and the final tag of the reference KAT (RFC 9106 §5.3).
python3 - "$tmp/argon2id.json" <<'PY' | jq -S '.' >"$out/argon2id_rfc9106.json"
import json, re, sys
text = open(sys.argv[1]).read()
def field(label):
    m = re.search(r"^" + re.escape(label) + r"\[\d+\]: ([0-9a-f ]+)$", text, re.M)
    return m.group(1).replace(" ", "")
params = re.search(r"Memory: (\d+) KiB, Iterations: (\d+), Parallelism: (\d+) lanes", text)
tag = re.findall(r"^Tag: ([0-9a-f ]+)$", text, re.M)[-1].replace(" ", "")
print(json.dumps({
    "m_kib": int(params.group(1)), "t": int(params.group(2)), "p": int(params.group(3)),
    "password": field("Password"), "salt": field("Salt"), "secret": field("Secret"),
    "associated_data": field("Associated data"), "tag": tag,
}))
PY

# XChaCha20-Poly1305: appendix A.3.1 of draft-irtf-cfrg-xchacha-03.
python3 - "$tmp/xchacha.json" <<'PY' | jq -S '.' >"$out/xchacha20poly1305_draft03.json"
import json, re, sys
text = open(sys.argv[1]).read()
section = text[text.index("A.3.1.  AEAD_XCHACHA20_POLY1305\n\n   Plaintext:"):text.index("A.3.2.  XChaCha20\n\nA.3.2.1")]
def block(label):
    body = section.split("   " + label + ":\n", 1)[1]
    hexes = []
    for line in body.splitlines()[1:]:
        stripped = line.strip()
        if not stripped:
            if hexes:
                break
            continue
        if not re.fullmatch(r"[0-9a-f]+", stripped):
            break
        hexes.append(stripped)
    return "".join(hexes)
print(json.dumps({k.lower(): block(k) for k in ["Plaintext", "AAD", "Key", "IV", "Ciphertext", "Tag"]}))
PY

# BIP-39: English vectors with 256-bit entropy (entropy <-> mnemonic only; the PBKDF2 seed is not used).
jq -S '[.english[] | select((.[0] | length) == 64) | {entropy: .[0], mnemonic: .[1]}]' \
  "$tmp/bip39.json" >"$out/bip39_english_256.json"

# Poseidon2 (Goldilocks, width 12): the input and expected output of Plonky3's
# `test_default_goldilocks_poseidon2_width_12` in src/poseidon2.rs of the pinned crate.
mkdir -p "$tmp/p3"
tar xzf "$tmp/p3-goldilocks.json" -C "$tmp/p3"
python3 - "$tmp/p3/p3-goldilocks-0.8.0/src/poseidon2.rs" <<'PY' | jq -S '.' >"$out/poseidon2_goldilocks_12.json"
import json, re, sys
text = open(sys.argv[1]).read()
body = text.split("fn test_default_goldilocks_poseidon2_width_12()", 1)[1].split("\n    }\n", 1)[0]
arrays = re.findall(r"new_array\(\[(.*?)\]\)", body, re.S)
def values(src):
    return [format(int(v.strip(), 0), "016x") for v in src.split(",") if v.strip()]
source = "p3-goldilocks 0.8.0 src/poseidon2.rs test_default_goldilocks_poseidon2_width_12"
print(json.dumps({"source": source, "input": values(arrays[0]), "output": values(arrays[1])}))
PY

# ChaCha20-Poly1305 (RFC 8439 §2.8.2): key, nonce, AAD, plaintext, ciphertext and tag from the
# `chacha20` test module of the pinned crate's tests/lib.rs.
mkdir -p "$tmp/chacha"
tar xzf "$tmp/chacha20poly1305.json" -C "$tmp/chacha"
python3 - "$tmp/chacha/chacha20poly1305-0.11.0/tests/lib.rs" <<'PY' | jq -S '.' >"$out/chacha20poly1305_rfc8439.json"
import json, re, sys
text = open(sys.argv[1]).read()
common, rest = text.split("mod chacha20 {", 1)
module = rest.split("\nmod ", 1)[0]
def array(src, name):
    body = re.search(r"const " + name + r": &\[u8(?:; \d+)?\] = &\[(.*?)\];", src, re.S).group(1)
    return "".join(format(int(v.strip(), 0), "02x") for v in body.split(",") if v.strip())
plain = re.search(r'const PLAINTEXT: &\[u8\] = b"(.*?)";', common, re.S).group(1)
plain = re.sub(r"\\\n\s*", "", plain).encode().hex()
print(json.dumps({
    "source": "RFC 8439 section 2.8.2, via chacha20poly1305 0.11.0 tests/lib.rs",
    "key": array(common, "KEY"),
    "aad": array(common, "AAD"),
    "plaintext": plain,
    "nonce": array(module, "NONCE"),
    "ciphertext": array(module, "CIPHERTEXT"),
    "tag": array(module, "TAG"),
}))
PY

echo "test vectors written to $out"
du -ch "$out"/*.json | tail -1
