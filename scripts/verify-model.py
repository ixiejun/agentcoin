#!/usr/bin/env python3
"""Downloads a model at its pinned revision and checks its content (m6-toploc-gpu-calibration
3.1; scripts/setup-vllm-cpu.sh and scripts/gpu-calibration.sh).

A mirror (HF_ENDPOINT, e.g. https://hf-mirror.com) serves files by path; only their digests say
they are the pinned ones. The snapshot digest is the SHA-256 of the lines `<sha256>  <path>` of
every file of the snapshot, sorted by path. An empty pin prints the digest found and fails, as
for the other pins in plugins/vllm/ci/pins.env.

Usage: scripts/verify-model.py MODEL REVISION PINNED_SNAPSHOT_SHA256
       scripts/verify-model.py --digest DIR   (the digest of a directory)
Prints the snapshot path; exits 1 on a different revision or digest.
"""

import hashlib
import os
import sys
from pathlib import Path


def digest(root: Path) -> str:
    lines = []
    for f in sorted(p for p in root.rglob("*") if p.is_file()):
        h = hashlib.sha256()
        with open(f, "rb") as fh:
            for block in iter(lambda: fh.read(1 << 20), b""):
                h.update(block)
        lines.append(f"{h.hexdigest()}  {f.relative_to(root).as_posix()}\n")
    return hashlib.sha256("".join(lines).encode()).hexdigest()


def fail(message: str) -> int:
    # Under GitHub Actions an error annotation carries the value to whoever pins it.
    if os.environ.get("GITHUB_ACTIONS"):
        print(f"::error::{message}")
    print(message, file=sys.stderr)
    return 1


def main() -> int:
    if len(sys.argv) == 3 and sys.argv[1] == "--digest":
        print(digest(Path(sys.argv[2])))
        return 0
    if len(sys.argv) != 4:
        print(__doc__, file=sys.stderr)
        return 2
    model, revision, pinned = sys.argv[1:]
    from huggingface_hub import snapshot_download

    path = Path(snapshot_download(model, revision=revision))
    # The snapshot directory is named after the commit the revision resolved to.
    if path.name != revision:
        return fail(f"model {model}: revision {revision} resolved to {path.name}")
    got = digest(path)
    if got != pinned:
        return fail(f"model {model}@{revision}: snapshot SHA-256 is {got}; pinned: '{pinned}' "
                    "(plugins/vllm/ci/pins.env)")
    print(path)
    return 0


if __name__ == "__main__":
    sys.exit(main())
