#!/bin/bash
# 云端会话容器是临时的：每次启动时确保 OpenSpec CLI 与 protoc 可用
set -euo pipefail
if [ "${CLAUDE_CODE_REMOTE:-}" != "true" ]; then
  exit 0
fi
if ! command -v openspec >/dev/null 2>&1; then
  npm install -g @fission-ai/openspec@latest >/dev/null 2>&1
fi
# The Polkadot SDK networking stack (litep2p) needs protoc at build time.
if ! command -v protoc >/dev/null 2>&1; then
  (apt-get install -y protobuf-compiler >/dev/null 2>&1 \
    || (apt-get update >/dev/null 2>&1 && apt-get install -y protobuf-compiler >/dev/null 2>&1)) || true
fi
