#!/usr/bin/env bash
# The TOPLOC re-check calibration on a rented GPU machine (m6-toploc-gpu-calibration design D3;
# spec engineering/ci-quality-gates "GPU 跨硬件校准"; docs/guides/gpu-calibration.md).
#
# Usage: scripts/gpu-calibration.sh <command>
#   setup            check the GPU and driver, install the pinned CUDA vLLM, the plugin, the TOPLOC
#                    reference, the models and ac-auditor, then run `verify`
#   verify           check versions and models against the pins (stops on any difference)
#   check            the plugin on this GPU (scripts/check-vllm-plugin.py), then the quick re-check
#                    regression; nothing is generated unless both pass
#   generate         a case bundle: 3,000 honest answers and 100 per cheating variant
#   recheck <bundle> re-check a bundle (a directory or the .tar.gz of one) on this machine
#   pack             archive the bundles and re-check reports for the way back, with SHA-256s
#   --self-test      the stop conditions, without a GPU or network
#
# Environment: WORK (default /root/autodl-tmp/agentcoin-gpu, else ~/agentcoin-gpu), PYTHON
# (python3, >= 3.10), HF_ENDPOINT (default https://hf-mirror.com), PIP_INDEX_URL (the machine's
# default; a mirror with PyPI's file layout also serves the vLLM wheel), MODELSCOPE (1: the
# models' weights from ModelScope), AC_AUDITOR (an ac-auditor binary built elsewhere),
# CARGO_MIRROR (e.g. sparse+https://rsproxy.cn/index/), RUSTUP_DIST_SERVER / RUSTUP_UPDATE_ROOT
# (rustup mirror), SEED, HONEST, CHEAT (generate). Every download that matters is checked against
# a digest in plugins/vllm/ci/pins.env or scripts/gen-toploc-vectors.sh; mirrors are not trusted.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
self="$repo_root/scripts/gpu-calibration.sh"
# shellcheck source=../plugins/vllm/ci/pins.env
source "$repo_root/plugins/vllm/ci/pins.env"
if [ -z "${WORK:-}" ]; then
  if [ -d /root/autodl-tmp ]; then WORK=/root/autodl-tmp/agentcoin-gpu; else WORK="$HOME/agentcoin-gpu"; fi
fi
state="$WORK/state"
venv="$WORK/venv"
export HF_ENDPOINT="${HF_ENDPOINT:-https://hf-mirror.com}"
export HF_HOME="${HF_HOME:-$WORK/hf}"
# A mirror serves files over plain HTTP; Xet transfers go to Hugging Face's own storage servers,
# which refuse a mirror's requests (401).
export HF_HUB_DISABLE_XET="${HF_HUB_DISABLE_XET:-1}"
# The CUDA compiler FlashInfer builds its run-time (JIT) kernels with, at the CUDA version torch is
# built for: the vLLM wheel's dependencies bring a newer one, whose PTX the pinned runtime's ptxas
# and headers reject.
CUDA_JIT_PACKAGES=(nvidia-cuda-nvcc==13.0.88 nvidia-cuda-crt==13.0.88 nvidia-cuda-cccl==13.0.85 nvidia-nvvm==13.0.88)

die() {
  echo "STOP: $*" >&2
  exit 1
}

# `a.b` >= `c.d`, numerically.
version_ge() {
  [ "$(printf '%s\n%s\n' "$2" "$1" | sort -V | head -n1)" = "$2" ]
}

# The file's SHA-256 must be the pinned one.
check_sha() {
  local file="$1" want="$2" got
  got="$(sha256sum "$file" | cut -d' ' -f1)"
  [ -n "$want" ] && [ "$got" = "$want" ] || die "$(basename "$file") has SHA-256 $got; pinned: '$want'"
}

require() {
  [ -f "$state/$1.ok" ] || die "run '$self $1' first (it has not passed on this machine)"
}

gpu_query() {
  nvidia-smi --query-gpu="$1" --format=csv,noheader 2>/dev/null | head -n1 | sed 's/^ *//; s/ *$//'
}

# The GPU must compute bfloat16 natively (compute capability 8.0 or newer) and its driver must
# support the CUDA version the pinned torch is built for.
check_gpu() {
  command -v nvidia-smi >/dev/null || die "no nvidia-smi: this is not a GPU machine"
  local cap name cuda
  name="$(gpu_query name)"
  cap="$(gpu_query compute_cap)"
  [ -n "$cap" ] || die "nvidia-smi does not report the compute capability (driver too old)"
  version_ge "$cap" 8.0 || die "$name has compute capability $cap; bfloat16 needs 8.0 or newer (e.g. RTX 5090, RTX 3090, H800, A100)"
  cuda="$(nvidia-smi | grep -o 'CUDA Version: *[0-9.]*' | grep -o '[0-9.]*$' || true)"
  [ -n "$cuda" ] || die "nvidia-smi does not report the driver's CUDA version"
  version_ge "$cuda" "$CUDA_MIN_VERSION" ||
    die "the driver supports CUDA $cuda; the pinned torch $TORCH_VERSION needs $CUDA_MIN_VERSION (choose a machine whose driver supports CUDA >= $CUDA_MIN_VERSION)"
  echo "GPU: $name, compute capability $cap, driver CUDA $cuda"
}

# The CUDA toolkit pip installs into the environment (nvidia/cu13), if any.
cuda_home() {
  local d
  for d in "$venv"/lib/python3*/site-packages/nvidia/cu"${CUDA_MIN_VERSION%%.*}"; do
    [ -x "$d/bin/nvcc" ] && echo "$d" && return 0
  done
  return 0
}

engine_env() {
  [ -x "$venv/bin/python" ] || die "no environment in $venv: run '$self setup'"
  # shellcheck disable=SC1091
  source "$venv/bin/activate"
  export AC_VLLM_MODEL="$MODEL" AC_VLLM_REVISION="$MODEL_REVISION"
  export AC_CHEAT_MODEL="$CHEAT_MODEL" AC_CHEAT_REVISION="$CHEAT_MODEL_REVISION"
  export PATH="$HOME/.cargo/bin:$PATH"
  # vLLM 0.30 runs its V2 model runner on GPUs by default; the plugin supports V1 only
  # (plugins/vllm/README.md).
  export VLLM_USE_V2_MODEL_RUNNER=0
  # FlashInfer's JIT takes the toolkit of CUDA_HOME, else the nvcc on PATH: the image's own CUDA
  # (12.8 on AutoDL's images) is not the pinned one and cannot build for an RTX 5090 (sm_120).
  local cuda
  cuda="$(cuda_home)"
  if [ -n "$cuda" ]; then
    export CUDA_HOME="$cuda" PATH="$cuda/bin:$PATH"
  fi
}

# The URL of a PyPI file on a mirror that keeps PyPI's layout (e.g.
# https://pypi.tuna.tsinghua.edu.cn/simple -> https://pypi.tuna.tsinghua.edu.cn/packages/...).
mirror_url() {
  local index="${1%/}"
  echo "${index%/simple}/packages/${2#*/packages/}"
}

# Fetches URL into FILE through a partial file, so that an interrupted download leaves nothing.
fetch() {
  curl -fL --retry 3 -o "$2.part" "$1" && mv "$2.part" "$2" || { rm -f "$2.part"; return 1; }
}

# MODELSCOPE=1: puts a model's weights, from ModelScope, into the Hugging Face cache at the pinned
# revision, checked against the SHA-256 the HF endpoint lists for them. A mirror of the hub can
# redirect large files to Hugging Face's own storage, slow or unreachable from some regions;
# verify-model.py then fetches the small files and checks the whole snapshot.
prefetch_weights() {
  local repo="$1" rev="$2" sha dir
  sha="$(curl -fsSI "$HF_ENDPOINT/$repo/resolve/$rev/model.safetensors" | tr -d '"\r' |
    awk 'tolower($1) == "x-linked-etag:" { print $2 }')"
  [ "${#sha}" = 64 ] || die "$HF_ENDPOINT lists no SHA-256 for $repo@$rev model.safetensors"
  dir="$HF_HOME/hub/models--${repo//\//--}"
  mkdir -p "$dir/blobs" "$dir/snapshots/$rev"
  if ! echo "$sha  $dir/blobs/$sha" | sha256sum -c --quiet 2>/dev/null; then
    fetch "https://modelscope.cn/models/$repo/resolve/master/model.safetensors" "$dir/blobs/$sha" ||
      die "could not download $repo model.safetensors from ModelScope"
    check_sha "$dir/blobs/$sha" "$sha"
  fi
  ln -sfn "../../blobs/$sha" "$dir/snapshots/$rev/model.safetensors"
  echo "$repo@$rev model.safetensors from ModelScope (SHA-256 $sha)"
}

# A short name of this GPU for file names: "NVIDIA GeForce RTX 5090" -> "rtx-5090".
gpu_slug() {
  gpu_query name | tr '[:upper:]' '[:lower:]' | sed 's/nvidia//; s/geforce//; s/[^a-z0-9]\+/-/g; s/^-*//; s/-*$//'
}

cmd_setup() {
  check_gpu
  local py="${PYTHON:-python3}"
  "$py" -c 'import sys; sys.exit(sys.version_info < (3, 10))' || die "$py is older than 3.10 (vLLM $VLLM_VERSION needs 3.10-3.14)"
  mkdir -p "$state" "$WORK/wheels"
  rm -f "$state"/*.ok
  [ -x "$venv/bin/python" ] || "$py" -m venv "$venv"
  # shellcheck disable=SC1091
  source "$venv/bin/activate"
  python -m pip install -q --upgrade pip

  # 1. The CUDA wheel of the pinned vLLM, through the configured index (or its file path on that
  # index's mirror, or PyPI), checked against the digest PyPI lists; its dependencies (torch for
  # CUDA 13) from the same index, checked by version.
  local wheel
  wheel="$WORK/wheels/$(basename "$VLLM_CUDA_WHEEL_URL")"
  [ -f "$wheel" ] || python -m pip download -q --no-deps --only-binary :all: --platform manylinux_2_28_x86_64 \
    --python-version 3.12 --implementation cp --abi abi3 -d "$WORK/wheels" "vllm==$VLLM_VERSION" || true
  if [ ! -f "$wheel" ] && [ -n "${PIP_INDEX_URL:-}" ]; then
    fetch "$(mirror_url "$PIP_INDEX_URL" "$VLLM_CUDA_WHEEL_URL")" "$wheel" || true
  fi
  [ -f "$wheel" ] || fetch "$VLLM_CUDA_WHEEL_URL" "$wheel" || die "could not download $(basename "$wheel")"
  check_sha "$wheel" "$VLLM_CUDA_WHEEL_SHA256"
  python -m pip install -q "$wheel"
  python -m pip install -q -e "$repo_root/plugins/vllm"
  # The CUDA compiler for FlashInfer's JIT (see engine_env), and the unversioned libcudart.so its
  # link step asks for, which the runtime package does not ship.
  python -m pip install -q "${CUDA_JIT_PACKAGES[@]}"
  local cuda major="${CUDA_MIN_VERSION%%.*}"
  cuda="$(cuda_home)"
  [ -n "$cuda" ] || die "no CUDA $major toolkit in $venv after installing ${CUDA_JIT_PACKAGES[*]}"
  [ -e "$cuda/lib/libcudart.so" ] || ln -s "libcudart.so.$major" "$cuda/lib/libcudart.so"
  [ -e "$cuda/lib64" ] || ln -s lib "$cuda/lib64"

  # 2. The TOPLOC reference at the commit the vectors come from (needs GitHub; on AutoDL
  # `source /etc/network_turbo` first).
  eval "$(grep -E '^toploc_(url|commit|archive_sha256)=' "$repo_root/scripts/gen-toploc-vectors.sh")"
  local toploc="$WORK/toploc"
  [ -d "$toploc/.git" ] || git clone -q "$toploc_url" "$toploc"
  git -C "$toploc" checkout -q "$toploc_commit"
  local got
  got="$(git -C "$toploc" archive --format=tar "$toploc_commit" | sha256sum | cut -d' ' -f1)"
  [ "$got" = "$toploc_archive_sha256" ] || die "TOPLOC reference archive SHA-256 is $got; pinned: $toploc_archive_sha256"
  (cd "$toploc" && python -m pip install -q --no-build-isolation --no-deps .)

  # 3. The models, through HF_ENDPOINT, checked against their snapshot digests.
  if [ "${MODELSCOPE:-0}" = 1 ]; then
    prefetch_weights "$MODEL" "$MODEL_REVISION"
    prefetch_weights "$CHEAT_MODEL" "$CHEAT_MODEL_REVISION"
  fi
  python "$repo_root/scripts/verify-model.py" "$MODEL" "$MODEL_REVISION" "$MODEL_SNAPSHOT_SHA256"
  python "$repo_root/scripts/verify-model.py" "$CHEAT_MODEL" "$CHEAT_MODEL_REVISION" "$CHEAT_MODEL_SNAPSHOT_SHA256"

  # 4. Rust (the plugin check builds proofs with it) and ac-auditor.
  export PATH="$HOME/.cargo/bin:$PATH"
  if ! command -v cargo >/dev/null; then
    local init="$WORK/rustup-init"
    curl -fsSL --retry 3 -o "$init" "${RUSTUP_UPDATE_ROOT:-https://static.rust-lang.org/rustup}/dist/x86_64-unknown-linux-gnu/rustup-init"
    chmod +x "$init"
    "$init" -y -q --profile minimal --default-toolchain none
  fi
  if [ -n "${CARGO_MIRROR:-}" ] && ! grep -qs 'replace-with' "$HOME/.cargo/config.toml"; then
    printf '[source.crates-io]\nreplace-with = "mirror"\n[source.mirror]\nregistry = "%s"\n' "$CARGO_MIRROR" >>"$HOME/.cargo/config.toml"
  fi
  if command -v apt-get >/dev/null && ! command -v protoc >/dev/null; then
    apt-get install -y -q protobuf-compiler clang >/dev/null || echo "note: could not install protoc and clang with apt-get" >&2
  fi
  if [ -n "${AC_AUDITOR:-}" ]; then
    mkdir -p "$repo_root/target/debug"
    cp "$AC_AUDITOR" "$repo_root/target/debug/ac-auditor"
  else
    (cd "$repo_root" && SKIP_WASM_BUILD=1 cargo build -q -p ac-auditor)
  fi
  (cd "$repo_root" && cargo build -q -p ac-toploc --example toploc_from_candidates)
  touch "$state/setup.ok"
  cmd_verify
}

cmd_verify() {
  require setup
  engine_env
  check_gpu
  python - "$VLLM_VERSION" "$TORCH_VERSION" <<'PY' || die "the installed versions are not the pinned ones (see above)"
import sys
import torch, vllm
want_vllm, want_torch = sys.argv[1:]
ok = True
for what, got, want in (("vllm", vllm.__version__, want_vllm), ("torch", torch.__version__.split("+")[0], want_torch)):
    if got != want:
        print(f"{what} {got}; pinned {want}")
        ok = False
if not torch.cuda.is_available() or not torch.cuda.is_bf16_supported():
    print("torch sees no CUDA device with bfloat16")
    ok = False
print("vllm", vllm.__version__, "torch", torch.__version__, "cuda", torch.version.cuda,
      "device", torch.cuda.get_device_name(0) if torch.cuda.is_available() else None)
sys.exit(0 if ok else 1)
PY
  python "$repo_root/scripts/verify-model.py" "$MODEL" "$MODEL_REVISION" "$MODEL_SNAPSHOT_SHA256" >/dev/null
  python "$repo_root/scripts/verify-model.py" "$CHEAT_MODEL" "$CHEAT_MODEL_REVISION" "$CHEAT_MODEL_SNAPSHOT_SHA256" >/dev/null
  [ -n "${CUDA_HOME:-}" ] && "$CUDA_HOME/bin/nvcc" --version | grep -q "release $CUDA_MIN_VERSION," ||
    die "the environment's CUDA compiler is not CUDA $CUDA_MIN_VERSION (FlashInfer's JIT needs it): run '$self setup'"
  [ -x "$repo_root/target/debug/ac-auditor" ] || die "no target/debug/ac-auditor"
  touch "$state/verify.ok"
  echo "verify: ok"
}

cmd_check() {
  require verify
  engine_env
  rm -f "$state/check.ok"
  local logs="$WORK/check-$(gpu_slug)"
  mkdir -p "$logs"
  echo "plugin check (log: $logs/check-vllm-plugin.log)"
  (cd "$repo_root" && python scripts/check-vllm-plugin.py) >"$logs/check-vllm-plugin.log" 2>&1 || {
    tail -n 40 "$logs/check-vllm-plugin.log"
    die "the plugin check failed on this GPU: no calibration here (send $logs/check-vllm-plugin.log)"
  }
  tail -n 3 "$logs/check-vllm-plugin.log"
  # The plugin check runs the engine eagerly; generation and re-check run it as a provider and an
  # auditor would (CUDA graphs): the quick regression covers that before any large run.
  echo "quick re-check regression (log: $logs/quick.log)"
  (cd "$repo_root" && python scripts/calibrate-toploc.py --quick --out "$logs/quick") >"$logs/quick.log" 2>&1 || {
    tail -n 40 "$logs/quick.log"
    die "the quick regression failed on this GPU: no calibration here (send $logs/quick.log)"
  }
  tail -n 1 "$logs/quick.log"
  touch "$state/check.ok"
  echo "check: ok"
}

# The seed of a GPU's bundle: the consumer GPU (RTX 5090, 4090 or 3090) 11, the data-center GPU (H800, H100 or A100) 12,
# any other 13 (set SEED to tell two others apart). Bundles of one seed hold the same prompts.
seed_for() {
  case "$1" in
    *5090* | *4090* | *3090*) echo 11 ;;
    *h800* | *h100* | *a100*) echo 12 ;;
    *) echo 13 ;;
  esac
}

cmd_generate() {
  require verify
  require check
  engine_env
  local slug seed
  slug="$(gpu_slug)"
  seed="${SEED:-$(seed_for "$slug")}"
  local out="$WORK/bundle-$slug-seed$seed"
  echo "generating $out (seed $seed)"
  (cd "$repo_root" && python scripts/calibrate-toploc.py --generate-only --honest "${HONEST:-3000}" \
    --cheat "${CHEAT:-100}" --seed "$seed" --out "$out")
}

cmd_recheck() {
  require verify
  require check
  engine_env
  local bundle="${1:-}"
  [ -n "$bundle" ] || die "usage: $self recheck <bundle directory or .tar.gz>"
  if [ -f "$bundle" ]; then
    local into="$WORK/incoming/$(basename "$bundle" .tar.gz)"
    mkdir -p "$into"
    tar -xzf "$bundle" -C "$into"
    bundle="$(dirname "$(find "$into" -name prover.json | head -n1)")"
  fi
  [ -f "$bundle/prover.json" ] || die "$bundle is not a case bundle (no prover.json)"
  local out="$WORK/recheck-$(basename "$bundle")-on-$(gpu_slug)"
  echo "re-checking $bundle into $out"
  (cd "$repo_root" && python scripts/calibrate-toploc.py --recheck-only "$bundle" --out "$out")
}

cmd_pack() {
  mkdir -p "$WORK/out"
  local d name made=0
  for d in "$WORK"/bundle-* "$WORK"/recheck-* "$WORK"/check-*; do
    [ -d "$d" ] || continue
    name="${d##*/}"
    case "$name" in
      # A re-check: its report and logs (the bundle it re-checked is on its prover's side).
      recheck-*) tar -czf "$WORK/out/$name.tar.gz" -C "$WORK" "$name/calibration.json" "$name/logs" ;;
      # A plugin check: its logs and the quick regression's report.
      check-*) tar -czf "$WORK/out/$name.tar.gz" -C "$WORK" --exclude "$name/quick/cases" "$name" ;;
      *) tar -czf "$WORK/out/$name.tar.gz" -C "$WORK" "$name" ;;
    esac
    made=1
  done
  [ "$made" = 1 ] || die "nothing to pack in $WORK"
  (cd "$WORK/out" && sha256sum ./*.tar.gz)
  echo "archives in $WORK/out"
}

# The stop conditions, with a fake nvidia-smi and fake downloads.
self_test() {
  local t fails=0
  t="$(mktemp -d)"
  trap 'rm -rf "$t"' RETURN
  mkdir -p "$t/bin"
  cat >"$t/bin/nvidia-smi" <<'EOF'
#!/usr/bin/env bash
case "$*" in
  *compute_cap*) echo "${FAKE_CAP}" ;;
  *name*) echo "${FAKE_NAME:-NVIDIA GeForce RTX 5090}" ;;
  *) echo "| NVIDIA-SMI 580.65  Driver Version: 580.65  CUDA Version: ${FAKE_CUDA} |" ;;
esac
EOF
  chmod +x "$t/bin/nvidia-smi"
  expect() { # expect <name> <needle> <command...>: fails with the needle in its output
    local name="$1" needle="$2" out
    shift 2
    if out="$("$@" 2>&1)"; then
      echo "FAIL $name: did not stop"
      fails=1
    elif ! grep -qF "$needle" <<<"$out"; then
      echo "FAIL $name: stopped without '$needle': $out"
      fails=1
    else
      echo "ok   $name"
    fi
  }
  local env=(env PATH="$t/bin:$PATH" WORK="$t/work")
  expect "compute capability below 8.0" "bfloat16 needs 8.0" "${env[@]}" FAKE_CAP=7.5 FAKE_CUDA=13.0 "$self" setup
  expect "driver without CUDA 13" "choose a machine whose driver supports CUDA" "${env[@]}" FAKE_CAP=8.6 FAKE_CUDA=12.4 "$self" setup
  [ ! -e "$t/work/venv" ] || { echo "FAIL a stopped setup created an environment"; fails=1; }
  expect "verify before setup" "run '$self setup' first" "${env[@]}" FAKE_CAP=8.6 FAKE_CUDA=13.0 "$self" verify
  expect "check before verify" "run '$self verify' first" "${env[@]}" "$self" check
  mkdir -p "$t/work/state" && touch "$t/work/state/setup.ok" "$t/work/state/verify.ok"
  expect "generate without a passed plugin check" "run '$self check' first" "${env[@]}" "$self" generate
  expect "recheck without a passed plugin check" "run '$self check' first" "${env[@]}" "$self" recheck x
  # The guide's GPUs, an H800 (compute capability 9.0) and an RTX 5090 (12.0, which a plain
  # string comparison would put below 8.0), pass the GPU check with a CUDA 13.0 driver.
  local gpu
  for gpu in "NVIDIA H800=9.0" "NVIDIA GeForce RTX 5090=12.0"; do
    if env PATH="$t/bin:$PATH" FAKE_NAME="${gpu%=*}" FAKE_CAP="${gpu#*=}" FAKE_CUDA=13.0 \
      bash -c "source '$self' --source-only; check_gpu" >/dev/null 2>&1; then
      echo "ok   ${gpu%=*} passes the GPU check"
    else
      echo "FAIL ${gpu%=*} does not pass the GPU check"
      fails=1
    fi
  done
  # Bundle names and seeds of the GPUs of the guide.
  local name want got
  for pair in "NVIDIA GeForce RTX 5090=rtx-5090 11" "NVIDIA GeForce RTX 3090=rtx-3090 11" "NVIDIA H800 PCIe=h800-pcie 12" "NVIDIA H800=h800 12" \
    "NVIDIA A100-SXM4-80GB=a100-sxm4-80gb 12" "NVIDIA L40S=l40s 13"; do
    name="${pair%%=*}" want="${pair#*=}"
    got="$(env PATH="$t/bin:$PATH" FAKE_NAME="$name" bash -c "source '$self' --source-only; s=\$(gpu_slug); echo \"\$s \$(seed_for \$s)\"")"
    if [ "$got" = "$want" ]; then echo "ok   $name: $got"; else echo "FAIL $name: '$got', want '$want'"; fails=1; fi
  done
  # The wheel's path on a PyPI mirror.
  local idx
  for idx in https://pypi.tuna.tsinghua.edu.cn/simple https://pypi.tuna.tsinghua.edu.cn/simple/; do
    want="https://pypi.tuna.tsinghua.edu.cn/packages/${VLLM_CUDA_WHEEL_URL#*/packages/}"
    got="$(bash -c "source '$self' --source-only; mirror_url '$idx' '$VLLM_CUDA_WHEEL_URL'")"
    if [ "$got" = "$want" ]; then echo "ok   mirror URL from $idx"; else echo "FAIL mirror URL: '$got', want '$want'"; fails=1; fi
  done
  # A failed download leaves no file behind (a partial wheel would only fail its digest later).
  if bash -c "source '$self' --source-only; fetch 'file://$t/no-such-file' '$t/fetched'" 2>/dev/null ||
    [ -e "$t/fetched" ] || [ -e "$t/fetched.part" ]; then
    echo "FAIL a failed download left a file"
    fails=1
  else
    echo "ok   a failed download leaves no file"
  fi
  # A download whose digest is not the pinned one.
  echo "not the wheel" >"$t/wheel.whl"
  expect "wheel digest" "has SHA-256" bash -c "source '$self' --source-only; check_sha '$t/wheel.whl' '$VLLM_CUDA_WHEEL_SHA256'"
  # A mirror that resolves the revision to another commit, or serves other files.
  mkdir -p "$t/py/huggingface_hub" "$t/snap/other" "$t/snap/$MODEL_REVISION"
  echo "x" >"$t/snap/other/model.safetensors"
  echo "x" >"$t/snap/$MODEL_REVISION/model.safetensors"
  printf 'import os\ndef snapshot_download(model, revision):\n    return os.environ["FAKE_SNAPSHOT"]\n' >"$t/py/huggingface_hub/__init__.py"
  expect "model revision" "resolved to other" env PYTHONPATH="$t/py" FAKE_SNAPSHOT="$t/snap/other" \
    python3 "$repo_root/scripts/verify-model.py" "$MODEL" "$MODEL_REVISION" "$MODEL_SNAPSHOT_SHA256"
  expect "model files" "snapshot SHA-256 is" env PYTHONPATH="$t/py" FAKE_SNAPSHOT="$t/snap/$MODEL_REVISION" \
    python3 "$repo_root/scripts/verify-model.py" "$MODEL" "$MODEL_REVISION" "$MODEL_SNAPSHOT_SHA256"
  [ "$fails" = 0 ] && echo "self-test passed"
  return "$fails"
}

if [ "${1:-}" = "--source-only" ]; then
  return 0 2>/dev/null || exit 0
fi
case "${1:-}" in
  setup) cmd_setup ;;
  verify) cmd_verify ;;
  check) cmd_check ;;
  generate) cmd_generate ;;
  recheck) shift; cmd_recheck "$@" ;;
  pack) cmd_pack ;;
  --self-test) self_test ;;
  *) sed -n '2,22p' "$0" | sed 's/^# \{0,1\}//'; exit 2 ;;
esac
