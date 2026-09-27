> 🌐 **English** | [简体中文](README.zh-CN.md)

# AgentCoin (ATC)

A post-quantum, privacy-native, permissionless L1 that makes frontier large-model training, post-training and inference available to every person on Earth without discrimination.

- **Coding-agent / contributor guide: [AGENT.md](AGENT.md)** ([简体中文](AGENT.zh-CN.md))
- Rust coding guidelines (local copy and index): [docs/rust-guidelines/INDEX.md](docs/rust-guidelines/INDEX.md) (upstream content is Chinese; see [SOURCE.md](docs/rust-guidelines/SOURCE.md))
- Decision log: [docs/decisions.md](docs/decisions.md) ([简体中文](docs/decisions.zh-CN.md))
- Issue log (open questions, deviations and their handling): [docs/issues.md](docs/issues.md) ([简体中文](docs/issues.zh-CN.md))
- MVP technical plan: [docs/design/mvp-technical-plan.md](docs/design/mvp-technical-plan.md) ([简体中文](docs/design/mvp-technical-plan.zh-CN.md))
- Full technical plan: [docs/design/full-technical-plan.md](docs/design/full-technical-plan.md) ([简体中文](docs/design/full-technical-plan.zh-CN.md))
- Research and discussion record: [docs/research/](docs/research/) (each file has a `.zh-CN.md` Chinese version)

Primary language: Rust. Development method: spec-driven development (SDD) with [OpenSpec](https://github.com/Fission-AI/OpenSpec); specs live in `openspec/` (`/opsx:propose` → `/opsx:apply` → `/opsx:archive`). OpenSpec artifacts are written in Simplified Chinese.

Documentation language policy: every project document has an English version (primary, at the canonical path) and a Simplified Chinese version (`*.zh-CN.md`), each linking to the other at the top.

Status: M0 (workspace, CI and the `ac-crypto` post-quantum library, [crates/ac-crypto](crates/ac-crypto/README.md)) and M1 (post-quantum chain, [node](node/README.md)) complete; M2 (AC-BFT finality, double-signing evidence, commit–reveal randomness) complete; M3 economics (scheduled emission, dual treasury, 80/20 fee distribution, PoA multisig, node-enforced supply invariants) implemented, PoS to follow. Specs in `openspec/specs/`.

## Local development

The toolchain is pinned in `rust-toolchain.toml` (installed automatically by `rustup`). Run the same checks as CI:

```bash
# Prerequisites for the node: protoc (e.g. `apt-get install protobuf-compiler`) and clang.
cargo fmt --all -- --check
SKIP_WASM_BUILD=1 cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
for c in ac-crypto ac-primitives ac-invariants pallet-pq-accounts pallet-aura-pq \
  pallet-validator-set pallet-ac-offences pallet-randomness-cr pallet-emission \
  pallet-treasury-dual pallet-poa-admin; do
  cargo build -p $c --no-default-features --target wasm32-unknown-unknown
done
cargo install --locked cargo-deny cargo-audit   # once
cargo deny check
cargo audit
scripts/check-license-boundary.sh
scripts/check-overmint-flag.sh
scripts/sync-audit-exceptions.py
scripts/fetch-test-vectors.sh && git diff --exit-code -- crates/ac-crypto/tests/vectors

# Node, four-node local testnet (alice, bob, charlie, dave) and end-to-end tests
cargo build -p ac-node -p ac-wallet
target/debug/ac-node --dev --tmp
scripts/run-local-testnet.sh --check
AC_E2E=1 cargo test -p ac-e2e -- --test-threads 1
scripts/wallet-smoke.sh
# Observe emission and the PoA multisig on the running local testnet (checked by
# run-local-testnet.sh --check): epoch 0 settles at block 21 and mints the treasury floor.
curl -s -H 'Content-Type: application/json' http://127.0.0.1:9944 \
  -d '{"id":1,"jsonrpc":"2.0","method":"state_call","params":["EmissionApi_total_minted","0x"]}'
curl -s -H 'Content-Type: application/json' http://127.0.0.1:9944 \
  -d '{"id":1,"jsonrpc":"2.0","method":"state_call","params":["TreasuryApi_floor","0x"]}'
# PoaCouncil::Members (well-known key): SCALE list of the admin accounts
curl -s -H 'Content-Type: application/json' http://127.0.0.1:9944 \
  -d '{"id":1,"jsonrpc":"2.0","method":"state_getStorage","params":["0x0a7e2b603d0e3b9627cde4d35083b551ba7fb8745735dc3be2a2c61a72c39e78"]}'
# Finality latency on 4, 7 and 10 local authorities (release build, >= 4 cores)
scripts/measure-finality.sh
```
