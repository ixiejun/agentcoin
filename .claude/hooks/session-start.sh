#!/bin/bash
# 云端会话容器是临时的：每次启动时确保 OpenSpec CLI 可用（SDD 工作流依赖它）
set -euo pipefail
if [ "${CLAUDE_CODE_REMOTE:-}" != "true" ]; then
  exit 0
fi
if ! command -v openspec >/dev/null 2>&1; then
  npm install -g @fission-ai/openspec@latest >/dev/null 2>&1
fi
