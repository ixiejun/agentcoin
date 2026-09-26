> 🌐 [English](README.md) | **简体中文**

# AgentCoin (ATC)

一个抗量子、天生隐私、无许可的 L1，让前沿大模型的训练、后训练与推理能力对所有地球公民无差别可用。

- **编码智能体 / 贡献者守则：[AGENT.zh-CN.md](AGENT.zh-CN.md)（[English](AGENT.md)）**
- Rust 编码规范（本地副本与索引）：[docs/rust-guidelines/INDEX.md](docs/rust-guidelines/INDEX.md)
- 决策记录：[docs/decisions.zh-CN.md](docs/decisions.zh-CN.md)
- MVP 技术方案：[docs/design/mvp-technical-plan.zh-CN.md](docs/design/mvp-technical-plan.zh-CN.md)
- 全量技术方案：[docs/design/full-technical-plan.zh-CN.md](docs/design/full-technical-plan.zh-CN.md)
- 调研与讨论过程：[docs/research/](docs/research/)

主要开发语言：Rust。开发方式：规格驱动（SDD），使用 [OpenSpec](https://github.com/Fission-AI/OpenSpec)，规范位于 `openspec/`（`/opsx:propose` → `/opsx:apply` → `/opsx:archive`）。OpenSpec 产物使用简体中文。

文档语言约定：每份项目文档都有英文版（主版本，位于规范路径）和简体中文版（`*.zh-CN.md`），两者在页首互相链接。

状态：M0 已完成——工作区、CI 与抗量子密码库 `ac-crypto`（[crates/ac-crypto](crates/ac-crypto/README.zh-CN.md)），规范见 `openspec/specs/`。下一步：M1（PQ 链）。

## 本地开发

工具链版本固定在 `rust-toolchain.toml`（`rustup` 会自动安装）。运行与 CI 相同的检查：

```bash
# 节点构建前置条件：protoc（例如 `apt-get install protobuf-compiler`）和 clang。
cargo fmt --all -- --check
SKIP_WASM_BUILD=1 cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
for c in ac-crypto ac-primitives pallet-pq-accounts pallet-aura-pq; do
  cargo build -p $c --no-default-features --target wasm32-unknown-unknown
done
cargo install --locked cargo-deny cargo-audit   # 仅需一次
cargo deny check
cargo audit
scripts/check-license-boundary.sh
scripts/sync-audit-exceptions.py
scripts/fetch-test-vectors.sh && git diff --exit-code -- crates/ac-crypto/tests/vectors

# 节点、本地测试网与端到端测试
cargo build -p ac-node -p ac-wallet
target/debug/ac-node --dev --tmp
scripts/run-local-testnet.sh --check
AC_E2E=1 cargo test -p ac-e2e
scripts/wallet-smoke.sh
```
