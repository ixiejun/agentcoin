#!/usr/bin/env bash
# Checks that a runtime WASM built for release exposes no benchmarking API (change
# license-internal-features, design D3): `runtime-benchmarks` is an internal feature whose
# builds are never distributed, so the runtime that ships must not carry it.
#
# `impl_runtime_apis!` records every implemented runtime API in the WASM custom section
# `runtime_apis`, as 12-byte entries: the API ID (BLAKE2b-64 of the trait name) followed by the
# API version (u32, little endian). This is what the node reads to learn the runtime's APIs. The
# check fails when that list contains a forbidden API, or when the section is missing (so that a
# format change cannot make it pass silently).
#
# Usage: scripts/check-release-runtime.sh <runtime.wasm> | --self-test
#   Pass the uncompressed blob (…/wbuild/ac-runtime/ac_runtime.compact.wasm); the
#   `.compact.compressed.wasm` next to it is the same blob compressed.
# Requirements: bash, python3.
set -euo pipefail

# Runtime API trait names that a distributed runtime must not implement.
FORBIDDEN_APIS="Benchmark"

check() { # check <wasm>
  python3 - "$1" "$FORBIDDEN_APIS" <<'PY'
import hashlib, sys

path, forbidden = sys.argv[1], sys.argv[2].split()

def leb128(data, pos):
    value = shift = 0
    while True:
        byte = data[pos]
        pos += 1
        value |= (byte & 0x7F) << shift
        shift += 7
        if not byte & 0x80:
            return value, pos

data = open(path, "rb").read()
if data[:4] != b"\0asm":
    sys.exit(f"{path}: not a WASM module")
pos, apis, found = 8, b"", False
while pos < len(data):
    section, pos = data[pos], pos + 1
    size, pos = leb128(data, pos)
    end = pos + size
    if section == 0:
        name_len, name_pos = leb128(data, pos)
        if data[name_pos:name_pos + name_len] == b"runtime_apis":
            found = True
            apis += data[name_pos + name_len:end]
    pos = end
if not found:
    sys.exit(f"{path}: no runtime_apis section; cannot tell which runtime APIs it exposes")
if len(apis) % 12:
    sys.exit(f"{path}: runtime_apis section is not a list of 12-byte entries")

ids = {hashlib.blake2b(name.encode(), digest_size=8).digest(): name for name in forbidden}
entries = [apis[i:i + 12] for i in range(0, len(apis), 12)]
bad = [ids[e[:8]] for e in entries if e[:8] in ids]
if bad:
    print(f"{path} exposes runtime APIs a release runtime must not have: {', '.join(bad)}",
          file=sys.stderr)
    for e in entries:
        print(f"  0x{e[:8].hex()} v{int.from_bytes(e[8:], 'little')}", file=sys.stderr)
    sys.exit(1)
print(f"{path}: {len(entries)} runtime APIs, none of: {', '.join(forbidden)}")
PY
}

# Writes a minimal WASM module with a `runtime_apis` section listing the named APIs (no section
# at all when none are named): synthetic <out> [<trait name>...]
synthetic() {
  python3 - "$@" <<'PY'
import hashlib, sys

def leb128(n):
    out = bytearray()
    while True:
        byte, n = n & 0x7F, n >> 7
        out.append(byte | (0x80 if n else 0))
        if not n:
            return bytes(out)

out, names = sys.argv[1], sys.argv[2:]
module = b"\0asm" + (1).to_bytes(4, "little")
if names:
    name = b"runtime_apis"
    body = b"".join(hashlib.blake2b(n.encode(), digest_size=8).digest() + (1).to_bytes(4, "little")
                    for n in names)
    payload = leb128(len(name)) + name + body
    module += b"\0" + leb128(len(payload)) + payload
open(out, "wb").write(module)
PY
}

self_test() {
  local tmp
  tmp="$(mktemp -d)"
  trap 'rm -rf "$tmp"' RETURN

  # The ID this script derives for `Benchmark` is the one sp-api generates (asserted against the
  # real trait in runtime/tests/modules.rs).
  local id
  id="$(python3 -c 'import hashlib; print(hashlib.blake2b(b"Benchmark", digest_size=8).hexdigest())')"
  if [[ "$id" != 67f4b8fba858782a ]]; then
    echo "self-test failed: unexpected ID $id for the benchmarking API" >&2
    return 1
  fi

  # Scenario: a release runtime without the benchmarking API passes.
  synthetic "$tmp/release.wasm" Core Metadata BlockBuilder
  if ! check "$tmp/release.wasm" >/dev/null; then
    echo "self-test failed: a runtime without the benchmarking API was rejected" >&2
    return 1
  fi
  # Scenario: a runtime exposing the benchmarking API fails.
  synthetic "$tmp/bench.wasm" Core Benchmark Metadata
  if check "$tmp/bench.wasm" 2>/dev/null; then
    echo "self-test failed: a runtime with the benchmarking API was accepted" >&2
    return 1
  fi
  # Scenario: a module without the runtime_apis section fails.
  synthetic "$tmp/empty.wasm"
  if check "$tmp/empty.wasm" 2>/dev/null; then
    echo "self-test failed: a module without runtime_apis was accepted" >&2
    return 1
  fi
  echo "release runtime self-test passed"
}

case "${1:-}" in
  --self-test) self_test ;;
  "" | -*) echo "usage: $0 <runtime.wasm> | --self-test" >&2; exit 2 ;;
  *) check "$1" ;;
esac
