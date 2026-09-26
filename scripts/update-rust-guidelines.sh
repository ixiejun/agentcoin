#!/usr/bin/env bash
# 更新 docs/rust-guidelines 下的 Rust 编码规范快照并重建索引。
# 用法：scripts/update-rust-guidelines.sh [<commit>]   （默认上游 main 最新提交）
set -euo pipefail
repo_root="$(cd "$(dirname "$0")/.." && pwd)"
dest="$repo_root/docs/rust-guidelines"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

git clone --quiet https://github.com/Rust-Coding-Guidelines/rust-coding-guidelines-zh.git "$tmp/rcg"
if [[ $# -ge 1 ]]; then
  git -C "$tmp/rcg" checkout --quiet "$1"
fi
commit="$(git -C "$tmp/rcg" rev-parse HEAD)"

rm -rf "$dest/src"
cp -r "$tmp/rcg/src" "$dest/src"
cp "$tmp/rcg/LICENSE" "$dest/LICENSE"
find "$dest" -name ".keep" -delete
python3 "$repo_root/scripts/gen-rust-guidelines-index.py"
echo "已更新到 $commit；请同步修改 docs/rust-guidelines/SOURCE.md 中的快照提交。"
