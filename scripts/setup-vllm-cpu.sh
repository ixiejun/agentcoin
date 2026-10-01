#!/usr/bin/env bash
# Install the pinned CPU build of vLLM, the AgentCoin TOPLOC plugin, the TOPLOC reference
# implementation and the pinned test models (m5-engine-toploc 7.1, m6-toploc-verify 7.1; CI job
# vllm-plugin).
#
# Usage: scripts/setup-vllm-cpu.sh
# Environment: PYTHON (default python3). Under GitHub Actions the needed LD_PRELOAD (Intel
# OpenMP, as vLLM's CPU wheels require) and the pins are exported to $GITHUB_ENV.
# Requirements: bash, curl, git, sha256sum, a C++ compiler, network access.
#
# The wheel's SHA-256 and the model revision are pinned in plugins/vllm/ci/pins.env; an empty
# or different pin fails the script after printing the value it found.
set -euo pipefail

repo_root="$(cd "$(dirname "$0")/.." && pwd)"
# shellcheck source=../plugins/vllm/ci/pins.env
source "$repo_root/plugins/vllm/ci/pins.env"
py="${PYTHON:-python3}"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
failed=0

# Lists the release's wheels with their SHA-256 digests (GitHub API; GITHUB_TOKEN if set), to
# fix or review the pins.
list_release_wheels() {
  "$py" - "$VLLM_VERSION" <<'PY'
import json, os, sys, urllib.request
req = urllib.request.Request(f"https://api.github.com/repos/vllm-project/vllm/releases/tags/v{sys.argv[1]}")
if os.environ.get("GITHUB_TOKEN"):
    req.add_header("Authorization", f"Bearer {os.environ['GITHUB_TOKEN']}")
for a in json.load(urllib.request.urlopen(req))["assets"]:
    if a["name"].endswith(".whl"):
        print(a["name"], a.get("digest"), a["browser_download_url"])
PY
}

# 1. The vLLM wheel, checked against its pinned SHA-256.
name="$(basename "$VLLM_WHEEL_URL")"
wheel="$tmp/${name//%2B/+}" # pip needs the real file name
if ! curl -fsSL --retry 3 -o "$wheel" "$VLLM_WHEEL_URL"; then
  echo "cannot download $VLLM_WHEEL_URL; the release's wheels are:" >&2
  list_release_wheels >&2 || true
  exit 1
fi
actual="$(sha256sum "$wheel" | cut -d' ' -f1)"
if [ "$actual" != "$VLLM_WHEEL_SHA256" ]; then
  echo "vLLM wheel SHA-256 is $actual; pinned: '${VLLM_WHEEL_SHA256}' (plugins/vllm/ci/pins.env)" >&2
  failed=1
fi

# 2. The model revision: the pinned commit must exist; an empty pin prints the current one.
revision="$MODEL_REVISION"
if [ -z "$revision" ]; then
  "$py" -m pip install -q huggingface_hub
  revision="$("$py" -c "from huggingface_hub import HfApi; print(HfApi().model_info('$MODEL').sha)")"
  echo "model $MODEL is at revision $revision; pin it as MODEL_REVISION (plugins/vllm/ci/pins.env)" >&2
  failed=1
fi
cheat_revision="$CHEAT_MODEL_REVISION"
if [ -z "$cheat_revision" ]; then
  "$py" -m pip install -q huggingface_hub
  cheat_revision="$("$py" -c "from huggingface_hub import HfApi; print(HfApi().model_info('$CHEAT_MODEL').sha)")"
  echo "model $CHEAT_MODEL is at revision $cheat_revision; pin it as CHEAT_MODEL_REVISION (plugins/vllm/ci/pins.env)" >&2
  failed=1
fi
[ "$failed" = 0 ] || exit 1

# 3. vLLM (CPU), the plugin, the SDK and pytest.
"$py" -m pip install -q "$wheel" --extra-index-url "$TORCH_INDEX"
"$py" -m pip install -q -e "$repo_root/plugins/vllm" pytest -r "$repo_root/tests/e2e/python/requirements.txt"

# 4. The TOPLOC reference at the commit the vectors come from (scripts/gen-toploc-vectors.sh).
eval "$(grep -E '^toploc_(url|commit|archive_sha256)=' "$repo_root/scripts/gen-toploc-vectors.sh")"
git clone -q "$toploc_url" "$tmp/toploc"
git -C "$tmp/toploc" checkout -q "$toploc_commit"
got="$(git -C "$tmp/toploc" archive --format=tar "$toploc_commit" | sha256sum | cut -d' ' -f1)"
if [ "$got" != "$toploc_archive_sha256" ]; then
  echo "toploc archive checksum mismatch: $got" >&2
  exit 1
fi
(cd "$tmp/toploc" && "$py" -m pip install -q --no-build-isolation --no-deps .)

# 5. The models, at their pinned revisions.
"$py" -c "from huggingface_hub import snapshot_download; print(snapshot_download('$MODEL', revision='$revision'))"
"$py" -c "from huggingface_hub import snapshot_download; print(snapshot_download('$CHEAT_MODEL', revision='$cheat_revision'))"

# 6. Intel OpenMP for the CPU wheel, and a report of what was installed.
iomp="$("$py" -c "import pathlib, sys; print(next((str(p) for p in pathlib.Path(sys.prefix).rglob('libiomp5.so')), ''))")"
if [ -n "${GITHUB_ENV:-}" ]; then
  {
    [ -z "$iomp" ] || echo "LD_PRELOAD=$iomp"
    echo "AC_VLLM_MODEL=$MODEL"
    echo "AC_VLLM_REVISION=$revision"
    echo "AC_CHEAT_MODEL=$CHEAT_MODEL"
    echo "AC_CHEAT_REVISION=$cheat_revision"
  } >>"$GITHUB_ENV"
fi
LD_PRELOAD="$iomp" "$py" -c "import torch, vllm; print('vllm', vllm.__version__, 'torch', torch.__version__, 'cpu', torch.backends.cpu.get_cpu_capability(), 'bf16', torch.ones(1, dtype=torch.bfloat16).dtype)"
