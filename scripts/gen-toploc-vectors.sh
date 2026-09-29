#!/usr/bin/env bash
# Generate the TOPLOC test vectors of crates/ac-toploc with the reference implementation
# (spec market/toploc, m5-work-settlement design D11).
#
# Usage: scripts/gen-toploc-vectors.sh
# Requirements: bash, git, python3 (with venv), a C++ compiler, network access to github.com and
# to a PyTorch package index.
#
# Environment:
#   TOPLOC_TORCH_INDEX  pip index for PyTorch (default: the official CPU index; PyPI's CUDA build
#                       gives the same CPU results)
#   TOPLOC_VENV         reuse this virtual environment instead of a temporary one
#
# Re-running must produce a byte-identical file (checked in CI via `git diff --exit-code`).
set -euo pipefail

repo_root="$(cd "$(dirname "$0")/.." && pwd)"
out="$repo_root/crates/ac-toploc/tests/vectors/toploc.json"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

# Upstream, pinned. The checksum is of `git archive --format=tar <commit>` (uncompressed tar,
# fully determined by the commit).
toploc_url="https://github.com/PrimeIntellect-ai/toploc"
toploc_commit="7ab7bcd6a4459ba4400fd41e4636bbeed7438997"
toploc_archive_sha256="6b4cfd9dc05030bfe91ae8d7fc029056e526a82713037c8a55f0c3c424c4602b"
torch_version="2.6.0"
torch_index="${TOPLOC_TORCH_INDEX:-https://download.pytorch.org/whl/cpu}"

venv="${TOPLOC_VENV:-$tmp/venv}"
if [ ! -x "$venv/bin/python" ]; then
  python3 -m venv "$venv"
fi
py="$venv/bin/python"
"$py" -m pip install -q --no-cache-dir --upgrade pip setuptools wheel
if ! "$py" -c "import torch, sys; sys.exit(torch.__version__.split('+')[0] != '$torch_version')" 2>/dev/null; then
  "$py" -m pip install -q --no-cache-dir --index-url "$torch_index" "torch==$torch_version"
fi
"$py" -m pip install -q --no-cache-dir numpy

git clone -q "$toploc_url" "$tmp/toploc"
git -C "$tmp/toploc" checkout -q "$toploc_commit"
actual="$(git -C "$tmp/toploc" archive --format=tar "$toploc_commit" | sha256sum | cut -d' ' -f1)"
if [ "$actual" != "$toploc_archive_sha256" ]; then
  echo "toploc archive checksum mismatch: $actual" >&2
  exit 1
fi
# Built against the installed PyTorch (the extension links its headers).
(cd "$tmp/toploc" && "$py" -m pip install -q --no-cache-dir --no-build-isolation --force-reinstall --no-deps .)

source_json="$(printf '{"url": "%s", "commit": "%s", "archive": "git archive --format=tar %s", "archive_sha256": "%s"}' \
  "$toploc_url" "$toploc_commit" "$toploc_commit" "$toploc_archive_sha256")"
# Run from outside the checkout so that the installed package, not the sources, is imported.
(cd "$tmp" && "$py" "$repo_root/scripts/gen-toploc-vectors.py" "$out" "$source_json")
echo "wrote $out"
