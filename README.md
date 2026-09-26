> 🌐 **English** | [简体中文](README.zh-CN.md)

# AgentCoin (ATC)

A post-quantum, privacy-native, permissionless L1 that makes frontier large-model training, post-training and inference available to every person on Earth without discrimination.

- **Coding-agent / contributor guide: [AGENT.md](AGENT.md)** ([简体中文](AGENT.zh-CN.md))
- Rust coding guidelines (local copy and index): [docs/rust-guidelines/INDEX.md](docs/rust-guidelines/INDEX.md) (upstream content is Chinese; see [SOURCE.md](docs/rust-guidelines/SOURCE.md))
- Decision log: [docs/decisions.md](docs/decisions.md) ([简体中文](docs/decisions.zh-CN.md))
- MVP technical plan: [docs/design/mvp-technical-plan.md](docs/design/mvp-technical-plan.md) ([简体中文](docs/design/mvp-technical-plan.zh-CN.md))
- Full technical plan: [docs/design/full-technical-plan.md](docs/design/full-technical-plan.md) ([简体中文](docs/design/full-technical-plan.zh-CN.md))
- Research and discussion record: [docs/research/](docs/research/) (each file has a `.zh-CN.md` Chinese version)

Primary language: Rust. Development method: spec-driven development (SDD) with [OpenSpec](https://github.com/Fission-AI/OpenSpec); specs live in `openspec/` (`/opsx:propose` → `/opsx:apply` → `/opsx:archive`). OpenSpec artifacts are written in Simplified Chinese.

Documentation language policy: every project document has an English version (primary, at the canonical path) and a Simplified Chinese version (`*.zh-CN.md`), each linking to the other at the top.

Status: M0 in progress — workspace, CI and the `ac-crypto` post-quantum library ([crates/ac-crypto](crates/ac-crypto/README.md)).

## Local development

The toolchain is pinned in `rust-toolchain.toml` (installed automatically by `rustup`). Run the same checks as CI:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
cargo build -p ac-crypto --no-default-features --target wasm32-unknown-unknown
cargo install --locked cargo-deny cargo-audit   # once
cargo deny check
cargo audit
scripts/fetch-test-vectors.sh && git diff --exit-code -- crates/ac-crypto/tests/vectors
```
