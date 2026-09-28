#!/usr/bin/env bash
# Copies the official Uniswap V2 sources used by the EVM acceptance tests into
# contracts/acceptance/lib/ (m4-evm task 7.3, design D14), byte for byte from pinned upstream
# versions, and writes lib/SOURCE.md with each file's SHA-256.
#
# The only change to upstream is the pair init-code hash in UniswapV2Library.pairFor: our build
# of UniswapV2Pair differs from upstream's in its metadata hash (source paths), so CREATE2 pair
# addresses need the hash of the bytecode this project compiles. It is set from
# PAIR_INIT_CODE_HASH below and recorded in SOURCE.md.
#
# Usage: scripts/vendor-uniswap-v2.sh [--check]
#   (no flag)  refresh contracts/acceptance/lib/
#   --check    vendor into a temporary directory and require it to equal the repository copy
# Requirements: bash, git, curl, tar, sha256sum, python3, network access to GitHub and the npm
# registry.
set -euo pipefail

repo_root="$(cd "$(dirname "$0")/.." && pwd)"
dest="$repo_root/contracts/acceptance/lib"

CORE_URL="https://github.com/Uniswap/v2-core"
CORE_COMMIT="4dd59067c76dea4a0e8e4bfdda41877a6b16dedc" # tag v1.0.1
PERIPHERY_URL="https://github.com/Uniswap/v2-periphery"
PERIPHERY_COMMIT="0335e8f7e1bd1e8d8329fd300aea2ef2f36dd19f"
# @uniswap/lib, the version v2-periphery depends on (no git tag exists for it).
LIB_URL="https://registry.npmjs.org/@uniswap/lib/-/lib-4.0.1-alpha.tgz"
LIB_INTEGRITY="sha512-f6UIliwBbRsgVLxIaBANF6w09tYqc6Y/qXdsrbEmXHyFA7ILiKrIwRFXe1yOg8M3cksgVsO9N7yuL2DdCGQKBA=="

UPSTREAM_INIT_CODE_HASH="96e8ac4277198ff8b6f785478aa9a39f403cb768dd02cbee326c3e7da348845f"
# keccak256 of UniswapV2Pair's init code as built by contracts/acceptance/foundry.toml.
PAIR_INIT_CODE_HASH="${PAIR_INIT_CODE_HASH:-30df050da9efe464e849f14dc53332691199266cd01cc61bd0cc9f8548fa5539}"

CORE_FILES="LICENSE
contracts/UniswapV2ERC20.sol
contracts/UniswapV2Factory.sol
contracts/UniswapV2Pair.sol
contracts/interfaces/IERC20.sol
contracts/interfaces/IUniswapV2Callee.sol
contracts/interfaces/IUniswapV2ERC20.sol
contracts/interfaces/IUniswapV2Factory.sol
contracts/interfaces/IUniswapV2Pair.sol
contracts/libraries/Math.sol
contracts/libraries/SafeMath.sol
contracts/libraries/UQ112x112.sol
contracts/test/ERC20.sol"
PERIPHERY_FILES="LICENSE
contracts/UniswapV2Router02.sol
contracts/interfaces/IERC20.sol
contracts/interfaces/IUniswapV2Router01.sol
contracts/interfaces/IUniswapV2Router02.sol
contracts/interfaces/IWETH.sol
contracts/libraries/SafeMath.sol
contracts/libraries/UniswapV2Library.sol
contracts/test/WETH9.sol"
LIB_FILES="LICENSE
contracts/libraries/TransferHelper.sol"

copy() { # copy <source dir> <destination dir> <file list>
  local from="$1" to="$2" f
  while read -r f; do
    mkdir -p "$(dirname "$to/$f")"
    cp "$from/$f" "$to/$f"
  done <<<"$3"
}

checkout() { # checkout <url> <commit> <dir>
  git init -q "$3"
  git -C "$3" fetch -q --depth 1 "$1" "$2"
  git -C "$3" checkout -q FETCH_HEAD
}

vendor() { # vendor <destination>
  local out="$1" work
  work="$(mktemp -d)"
  trap 'rm -rf "$work"' RETURN
  checkout "$CORE_URL" "$CORE_COMMIT" "$work/core"
  checkout "$PERIPHERY_URL" "$PERIPHERY_COMMIT" "$work/periphery"
  curl -sSfL -o "$work/lib.tgz" "$LIB_URL"
  local integrity
  integrity="sha512-$(python3 -c 'import base64, hashlib, sys
print(base64.b64encode(hashlib.sha512(open(sys.argv[1], "rb").read()).digest()).decode())' "$work/lib.tgz")"
  [[ "$integrity" == "$LIB_INTEGRITY" ]] || { echo "unexpected @uniswap/lib tarball" >&2; exit 1; }
  mkdir -p "$work/lib"
  tar xzf "$work/lib.tgz" -C "$work/lib"

  rm -rf "$out"
  mkdir -p "$out"
  copy "$work/core" "$out/v2-core" "$CORE_FILES"
  copy "$work/periphery" "$out/v2-periphery" "$PERIPHERY_FILES"
  copy "$work/lib/package" "$out/uniswap-lib" "$LIB_FILES"

  local library="$out/v2-periphery/contracts/libraries/UniswapV2Library.sol" upstream_sha
  upstream_sha="$(sha256sum "$library" | cut -d' ' -f1)"
  grep -q "hex'$UPSTREAM_INIT_CODE_HASH'" "$library"
  sed -i "s/hex'$UPSTREAM_INIT_CODE_HASH'/hex'$PAIR_INIT_CODE_HASH'/" "$library"

  {
    echo "# Vendored Uniswap V2 sources"
    echo
    echo "Copied by \`scripts/vendor-uniswap-v2.sh\` (m4-evm task 7.3, design D14); rerun it with"
    echo "\`--check\` to reproduce these files byte for byte. They keep their upstream licence,"
    echo "GPL-3.0-or-later (see each LICENSE), and are compiled with the upstream settings: solc"
    echo "0.5.16 (v2-core) and 0.6.6 (v2-periphery), optimizer 999999 runs, EVM version istanbul."
    echo
    echo "| Directory | Source | Version |"
    echo "|---|---|---|"
    echo "| \`v2-core/\` | $CORE_URL | commit \`$CORE_COMMIT\` (tag v1.0.1) |"
    echo "| \`v2-periphery/\` | $PERIPHERY_URL | commit \`$PERIPHERY_COMMIT\` |"
    echo "| \`uniswap-lib/\` | $LIB_URL | @uniswap/lib 4.0.1-alpha, \`$LIB_INTEGRITY\` |"
    echo
    echo "## Change to upstream"
    echo
    echo "\`v2-periphery/contracts/libraries/UniswapV2Library.sol\`: the pair init-code hash in"
    echo "\`pairFor\` is \`$PAIR_INIT_CODE_HASH\` (upstream \`$UPSTREAM_INIT_CODE_HASH\`), the"
    echo "keccak-256 of \`UniswapV2Pair\`'s init code as this project compiles it; the metadata hash"
    echo "embedded in the bytecode depends on source paths. Upstream file SHA-256: \`$upstream_sha\`."
    echo
    echo "## Files"
    echo
    echo "| File | SHA-256 |"
    echo "|---|---|"
    (cd "$out" && find . -type f ! -name SOURCE.md | sed 's|^\./||' | LC_ALL=C sort |
      while read -r f; do echo "| \`$f\` | $(sha256sum "$f" | cut -d' ' -f1) |"; done)
  } >"$out/SOURCE.md"
}

case "${1:-}" in
  "") vendor "$dest" && echo "vendored into $dest" ;;
  --check)
    tmp="$(mktemp -d)"
    trap 'rm -rf "$tmp"' EXIT
    vendor "$tmp/lib"
    diff -r "$tmp/lib" "$dest" && echo "vendored Uniswap V2 sources are reproducible"
    ;;
  *) echo "usage: $0 [--check]" >&2; exit 2 ;;
esac
