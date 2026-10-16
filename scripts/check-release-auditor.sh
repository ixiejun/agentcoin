#!/usr/bin/env bash
# Checks that an `ac-auditor` binary built for release has no testing switch (OpenSpec change
# m6-audit-hardening 7.5): the `lenient` feature adds the configuration key
# `test_lenient_verdicts`, which makes the agent report every result judged by the thresholds as
# a pass, and is never built for release.
#
# The key's name is a string literal of the configuration parser, so a binary that has the switch
# contains it; one without it does not. To make sure the right file is checked, the binary must
# contain `public_endpoint`, a key every build parses.
#
# Usage: scripts/check-release-auditor.sh <ac-auditor binary> | --self-test
#   Build it with `cargo build -p ac-auditor --release` (target/release/ac-auditor).
# Requirements: bash, grep.
set -euo pipefail

SWITCH="test_lenient_verdicts"
ALWAYS="public_endpoint"

check() { # check <binary>
  local bin="$1"
  [[ -f "$bin" ]] || { echo "$bin: no such file" >&2; return 1; }
  if ! grep -qaF "$ALWAYS" "$bin"; then
    echo "$bin: does not look like ac-auditor (no \"$ALWAYS\")" >&2
    return 1
  fi
  if grep -qaF "$SWITCH" "$bin"; then
    echo "$bin: has the testing switch \"$SWITCH\" (built with the lenient feature)" >&2
    return 1
  fi
  echo "$bin: no testing switch"
}

self_test() {
  local tmp
  tmp="$(mktemp -d)"
  trap 'rm -rf "$tmp"' RETURN
  # Scenario: a release-like binary passes.
  printf '\x7fELF\0..%s..\0' "$ALWAYS" >"$tmp/release"
  check "$tmp/release" >/dev/null || { echo "self-test failed: a clean binary was rejected" >&2; return 1; }
  # Scenario: a binary with the switch fails.
  printf '\x7fELF\0..%s..%s..\0' "$ALWAYS" "$SWITCH" >"$tmp/testing"
  if check "$tmp/testing" 2>/dev/null; then
    echo "self-test failed: a binary with the switch was accepted" >&2
    return 1
  fi
  # Scenario: another file fails.
  printf 'not the auditor' >"$tmp/other"
  if check "$tmp/other" 2>/dev/null; then
    echo "self-test failed: another file was accepted" >&2
    return 1
  fi
  echo "self-test passed"
}

case "${1:-}" in
  --self-test) self_test ;;
  "") echo "usage: $0 <ac-auditor binary> | --self-test" >&2; exit 2 ;;
  *) check "$1" ;;
esac
