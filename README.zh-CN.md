> 🌐 [English](README.md) | **简体中文**

# AgentCoin (ATC)

一个抗量子、天生隐私、无许可的 L1，让前沿大模型的训练、后训练与推理能力对所有地球公民无差别可用。

- **编码智能体 / 贡献者守则：[AGENT.zh-CN.md](AGENT.zh-CN.md)（[English](AGENT.md)）**
- Rust 编码规范（本地副本与索引）：[docs/rust-guidelines/INDEX.md](docs/rust-guidelines/INDEX.md)
- 决策记录：[docs/decisions.zh-CN.md](docs/decisions.zh-CN.md)
- 问题记录（待决问题、偏差及其处理）：[docs/issues.zh-CN.md](docs/issues.zh-CN.md)（[English](docs/issues.md)）
- MVP 技术方案：[docs/design/mvp-technical-plan.zh-CN.md](docs/design/mvp-technical-plan.zh-CN.md)
- 全量技术方案：[docs/design/full-technical-plan.zh-CN.md](docs/design/full-technical-plan.zh-CN.md)
- 调研与讨论过程：[docs/research/](docs/research/)

主要开发语言：Rust。开发方式：规格驱动（SDD），使用 [OpenSpec](https://github.com/Fission-AI/OpenSpec)，规范位于 `openspec/`（`/opsx:propose` → `/opsx:apply` → `/opsx:archive`）。OpenSpec 产物使用简体中文。

文档语言约定：每份项目文档都有英文版（主版本，位于规范路径）和简体中文版（`*.zh-CN.md`），两者在页首互相链接。

状态：M0（工作区、CI 与抗量子密码库 `ac-crypto`，[crates/ac-crypto](crates/ac-crypto/README.zh-CN.md)）和 M1（抗量子链，[node](node/README.zh-CN.md)）已完成；M2（AC-BFT 终局性、双签证据、commit–reveal 随机数）已完成；M3 经济部分（计划排放、双国库、80/20 手续费分配、PoA 多签、节点执行的供应量不变量）已完成；M3 PoS 部分（质押、Phragmén 选举、按工作量发放奖励、罚没，以及节点强制执行的单向 PoA→PoS 切换）已完成。规范见 `openspec/specs/`。

## 本地开发

工具链版本固定在 `rust-toolchain.toml`（`rustup` 会自动安装）。运行与 CI 相同的检查：

```bash
# 节点构建前置条件：protoc（例如 `apt-get install protobuf-compiler`）和 clang。
cargo fmt --all -- --check
SKIP_WASM_BUILD=1 cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
for c in ac-crypto ac-primitives ac-invariants pallet-pq-accounts pallet-aura-pq \
  pallet-validator-set pallet-ac-offences pallet-randomness-cr pallet-emission \
  pallet-treasury-dual pallet-poa-admin pallet-staking-pos; do
  cargo build -p $c --no-default-features --target wasm32-unknown-unknown
done
cargo install --locked cargo-deny cargo-audit   # 仅需一次
cargo deny check
cargo audit
scripts/check-license-boundary.sh
scripts/check-overmint-flag.sh
scripts/sync-audit-exceptions.py
scripts/fetch-test-vectors.sh && git diff --exit-code -- crates/ac-crypto/tests/vectors

# 节点、四节点本地测试网（alice、bob、charlie、dave）与端到端测试
cargo build -p ac-node -p ac-wallet
target/debug/ac-node --dev --tmp
scripts/run-local-testnet.sh --check
AC_E2E=1 cargo test -p ac-e2e -- --test-threads 1
scripts/wallet-smoke.sh
# 观察运行中本地测试网的排放与 PoA 多签（run-local-testnet.sh --check 会执行这些检查）：
# 纪元 0 在第 21 块结算，铸造国库保底。
curl -s -H 'Content-Type: application/json' http://127.0.0.1:9944 \
  -d '{"id":1,"jsonrpc":"2.0","method":"state_call","params":["EmissionApi_total_minted","0x"]}'
curl -s -H 'Content-Type: application/json' http://127.0.0.1:9944 \
  -d '{"id":1,"jsonrpc":"2.0","method":"state_call","params":["TreasuryApi_floor","0x"]}'
# PoaCouncil::Members（固定存储键）：管理成员账户的 SCALE 列表
curl -s -H 'Content-Type: application/json' http://127.0.0.1:9944 \
  -d '{"id":1,"jsonrpc":"2.0","method":"state_getStorage","params":["0x0a7e2b603d0e3b9627cde4d35083b551ba7fb8745735dc3be2a2c61a72c39e78"]}'
# PoA→PoS 切换进度（SCALE 编码的 TransitionProgress：阶段、切换区块、连续达标起始高度、
# 有效质押、所需质押、合格候选人数、参数、当前高度）
curl -s -H 'Content-Type: application/json' http://127.0.0.1:9944 \
  -d '{"id":1,"jsonrpc":"2.0","method":"state_call","params":["StakingApi_transition","0x"]}'
# 4、7、10 个本地验证人的终局性延迟（release 构建，至少 4 核）
scripts/measure-finality.sh
```

## 许可证

AgentCoin 按目录划分许可证（决策 D47，详见 [`LICENSE`](LICENSE)）：

| 目录 | 许可证 |
|---|---|
| `node/`、`services/`、`clients/wallet-cli/`、`tests/`、`scripts/` | `GPL-3.0-or-later`（[`LICENSE-GPL`](LICENSE-GPL)） |
| 其余全部：`crates/`、`pallets/`、`runtime/`、`docs/` 等 | `MIT OR Apache-2.0`（[`LICENSE-MIT`](LICENSE-MIT)、[`LICENSE-APACHE`](LICENSE-APACHE)） |

供钱包、SDK 和其他链嵌入的库采用宽松许可；程序和服务采用 copyleft 许可。贡献按其所在目录的许可证授权。`scripts/check-license-boundary.sh` 在 CI 中强制执行这一划分。
