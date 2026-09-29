> 🌐 **English** | [简体中文](AGENT.zh-CN.md)

# AGENT.md — AgentCoin Coding-Agent Guide

> Every coding agent (and every human contributor) working in this repository must follow these rules.
> Read this whole file before starting any work. Where a practice conflicts with this file, this file wins, unless the user explicitly instructs otherwise in the current conversation.

**Instruction precedence** (high → low):
1. Explicit instructions from the user in the current conversation
2. This file (`AGENT.md`)
3. Project context and rules in `openspec/config.yaml`
4. The proposal / specs / design / tasks of the current OpenSpec change
5. `docs/rust-guidelines/` (Rust coding guidelines; §7 of this file overrides a few items)
6. General community conventions

---

## 1. Project overview

AgentCoin (token **ATC**) is a **post-quantum, privacy-native, permissionless** L1 that organizes the world's heterogeneous compute (data-center GPUs, consumer GPUs, CPUs, storage) into a decentralized service for large-model **inference → post-training → pre-training**, and is EVM compatible.

- Chain framework: standalone Polkadot SDK (Substrate) chain; EVM via `pallet-revive`
- Primary language: **Rust** (D31); Python only for thin plugins inside inference/training engines
- Development method: **spec-driven development (SDD) + OpenSpec** (D32)
- Current stage: MVP; M0 (engineering foundation + PQ crypto library), M1 (PQ chain), M2 (AC-BFT finality), M3 (emission, treasury, PoA multisig, nominated PoS and the PoA → PoS switch) and M4 (EVM: `pallet-revive`, PQ precompiles, eth-RPC adapter, Foundry via the wallet) complete

### 1.1 Authoritative document map

| Document | Purpose | When to read |
|---|---|---|
| `docs/decisions.md` (Chinese: `.zh-CN.md`) | **All confirmed decisions D1–D59 (highest design authority)** | At the start of every task |
| `docs/design/mvp-technical-plan.md` | MVP architecture, modules, data structures, milestones | For MVP tasks |
| `docs/design/full-technical-plan.md` | Full-version architecture and interfaces the MVP must reserve (§11) | When designing any interface |
| `docs/research/01–07` | Research and discussion behind the decisions | When you need the "why" |
| `openspec/specs/` | Archived system specs (behavior contracts) | When modifying an existing capability |
| `openspec/changes/<name>/` | In-progress change (local only, not committed) | During apply |
| `docs/rust-guidelines/INDEX.md` | Index of the Rust coding guidelines | Whenever writing Rust |

If documents conflict: **do not decide on your own** — point out the conflict in your reply and ask the user. Decisions in `docs/decisions.md` prevail.

---

## 2. Red lines (never violate)

No change that violates any item below may be committed. If a task conflicts with a red line, stop and explain it to the user.

1. **100% post-quantum** (D4, D13)
   - Account authorization and consensus signatures use only ML-DSA (SLH-DSA / FN-DSA / XMSS reserved). **Forbidden** for account authorization or consensus: secp256k1, Ed25519, sr25519, BLS.
   - Encryption uses only ML-KEM-768 + X25519 hybrid (X-Wing) or a pure PQ KEM; classic ECDH alone must never protect long-lived secrets.
   - Zero-knowledge proofs use only the STARK / FRI family. **Forbidden**: Groth16, PLONK-KZG, anything based on BN254 / BLS12 pairings.
   - All hash outputs are 256 bits (BLAKE3 / SHA3-256 / Poseidon2).
2. **Algorithm agility**: every public key, signature, ciphertext and proof carries an AlgId (the tagged types in `ac-crypto`). Concrete algorithm implementations must **never** be called outside `ac-crypto`.
3. **Constitution layer 1 is enforced by the node** (D24, D30): the 21,000,000 ATC supply cap, the emission ceiling / no premine, and the PoA→PoS switch conditions must be checked in the native node client, **not only in the runtime**. Published well-known storage keys must never be renamed or change format.
4. **No premine** (D9): genesis allocates no ATC; there is no minting path other than the emission rules.
5. **Treasury formula** (D18): `treasury = max(5% × scheduled amount, 20/70 × actual work emission)` — take the larger, **never add**.
6. **Privacy** (D20, D27): prompts and inference outputs **never go on chain**; gateways do not log request content; on mainnet the only payment path is anonymous vouchers.
7. **Protocol neutrality** (D6, D24): no content censorship, address blacklists or geo-blocking at the protocol layer.
8. **The chain is not on the inference data path**: off-chain refine → work report → on-chain accumulate.
9. **No home-made cryptographic primitives**: only wrap reviewed implementations (RustCrypto etc.); every new primitive needs official test vectors.
10. **No secrets in the repo**: private keys, mnemonics, API keys and `.env` files are never committed.

---

## 3. Workflow: SDD + OpenSpec

**No feature code without an OpenSpec change the user has confirmed.**

```
/opsx:explore (optional, clarify requirements)
   → /opsx:propose  generate proposal / specs / design / tasks
   → user review and confirmation           ← mandatory, never skip
   → /opsx:apply    implement tasks.md in order, ticking each box
   → all tasks done + CI green
   → /opsx:archive  merge specs into openspec/specs/
```

Rules:
- **Proposals** (follow the rules in `openspec/config.yaml`): cite decision numbers Dxx and technical-plan sections; include Non-goals; for crypto/consensus changes, state the impact on post-quantum security and constitution layer 1; the design must explain how it connects to the full-version reserved interfaces.
- **Specs describe behavior, not implementation**: no library or function names; every Requirement has at least one testable Scenario (a `####` heading); use SHALL / MUST.
- **Tasks**: each ≤ about half a day, with an automatable acceptance check; tests and docs ship with their own task group, never collected at the end.
- **Apply**: follow tasks.md in order; tick `- [x]` immediately after finishing each item; if tasks conflict with specs/design or the scope must grow, **stop and raise it**, revise the plan with `/opsx:update` — never change scope on the fly.
- **Archive**: make sure specs are synced; record any new decision in `docs/decisions.md` (new number, with source), in both language versions.
- `openspec/changes/` is git-ignored (not committed). Cloud containers get reclaimed, so **archive changes promptly** once done.
- **OpenSpec artifacts are written in Simplified Chinese.**
- **Small changes that may skip OpenSpec**: typos, pure wording in docs, non-behavioral CI fixes, patch-level dependency bumps. Anything that changes externally observable behavior or interfaces must go through OpenSpec.
- **Do not hand-edit** `openspec/specs/` (it is updated only by archive), except to fix a `TBD` Purpose left by archive.

---

## 4. Repository layout and module boundaries

```
AGENT.md · CLAUDE.md · README.md · Cargo.toml · rust-toolchain.toml · deny.toml
docs/{decisions.md, design/, research/, rust-guidelines/}
openspec/{config.yaml, specs/, changes/(local)}
crates/      shared libraries: ac-crypto, ac-primitives, ac-invariants, ac-toploc
node/        ac-node: consensus (aura-pq, ac-bft), node invariant checker
runtime/     WASM runtime assembly
pallets/     on-chain modules (pq-accounts, emission, credits, work, audit …)
circuits/    STARK circuits (β)
services/    gateway, provider, auditor, eth-rpc
clients/     wallet-cli, sdk (Rust core), sdk-bindings (PyO3 / wasm-bindgen)
contracts/   example Solidity contracts
tests/       e2e, economic simulation
scripts/     tooling scripts
```

- **Licence zones (D47)**: `node/`, `services/`, `clients/wallet-cli/`, `tests/` and `scripts/` are `GPL-3.0-or-later`; everything else (including any new directory) is `MIT OR Apache-2.0` and must stay GPL-free, including toward the repository's own GPL crates. Each crate declares its zone's licence in `Cargo.toml`; see `LICENSE`.
- **Create directories on demand**: never pre-create empty directories or crates for future modules.
- **Dependency direction**: `crates/*` never depend on `node` / `runtime` / `pallets` / `services`; `pallets` depend only on `crates` and the Polkadot SDK; `services` / `clients` never depend on `node` internals and interact only through RPC / shared types.
- **Cryptography lives only in `ac-crypto`**; shared data types only in `ac-primitives`; node invariants are **pure functions** in `ac-invariants` (to enable formal verification).

---

## 5. Rust engineering rules

### 5.1 Toolchain and workspace
- The toolchain is pinned in `rust-toolchain.toml`; never switch versions ad hoc on the command line.
- Every crate is a member of the root workspace; dependency versions are declared once in the root `Cargo.toml` under `[workspace.dependencies]`, and members use `dep = { workspace = true }`.
- Pin versions to the minor version (e.g. `"0.1"`), **no wildcards** (G.CAR.04). Before adding a dependency check: maintenance status, license (`deny.toml` allow-list), `no_std` support, and whether it pulls in duplicate versions of a library (P.SEC.01).
- Every crate's `Cargo.toml` has `description`, `license`, `repository`, `edition` (inherited from the workspace) (G.CAR.02).

### 5.2 Lints and formatting (enforced by CI)
- `cargo fmt --all -- --check` must pass (P.FMT.01).
- `cargo clippy --workspace --all-targets --all-features -- -D warnings` must pass.
- Workspace lints: `unsafe_code = "forbid"`; in non-test code deny `clippy::unwrap_used`, `clippy::expect_used`, `clippy::panic`, `clippy::indexing_slicing` (on-chain and crypto crates), `clippy::arithmetic_side_effects` (on-chain and economics crates).
- Never silence a lint with `#[allow(...)]` without a one-line comment explaining why; crate-wide blanket exemptions such as `#![allow(clippy::all)]` are forbidden.

### 5.3 Error handling (overrides upstream P.ERR.02 / G.ERR.02)
- **In non-test code, `unwrap()` / `expect()` / `panic!` / `unreachable!` / `todo!` / `unimplemented!` are forbidden**; use `?` and explicit error types.
- Library crates use custom error enums (`#[non_exhaustive]`, implementing `core::fmt::Display`, plus `std::error::Error` under the `std` feature); no `anyhow` in libraries. Binaries (services, CLI) may use `anyhow` at the outermost layer.
- Public functions returning `Result` document an `# Errors` section in rustdoc (G.CMT.01).
- Test code may use `unwrap` / `expect`.

### 5.4 Integers and numerics (on-chain determinism)
- Balances, emission and prices are **unsigned integers** (balances are `u128` in the smallest unit, 18 decimals); **floating point is forbidden in runtime, consensus and settlement code**.
- Every operation that can overflow uses `checked_*` / `saturating_*` with the reason for the choice; never rely on release-mode wrapping (G.TYP.INT.01).
- Conversions use `From` / `TryFrom`; **`as` is forbidden for conversions that can truncate or change sign** (G.TYP.01, G.TYP.03, G.TYP.INT.02).
- Ratios and shares use basis points (1/10_000) or fixed-point types such as `Perbill` / `Permill`, with the rounding direction stated (round down; remainders go to the reserve or are burned — never created out of thin air).
- Access arrays / slices with `get()`; indexing that can go out of bounds is forbidden (G.TYP.ARR.02).

### 5.5 no_std and runtime compatibility
- Crates used by the runtime must be `#![no_std]`, with `extern crate alloc` as needed; `std` is provided as a feature.
- CI builds these crates with `--no-default-features --target wasm32-unknown-unknown`.
- Runtime code must not contain: floating point, `HashMap` (non-deterministic iteration order — use `BTreeMap`), system time, randomness (use the on-chain randomness module), or unbounded collections (use `BoundedVec` and other bounded types).

### 5.6 Types and API design
- Express semantics with newtypes instead of exposing primitives (P.TYP.01): `AccountId`, `Balance`, `AlgId`, etc.
- Public structs / enums get `#[non_exhaustive]` by default (G.TYP.SCT.01, G.TYP.ENM.05). **Exception**: wire-format / on-chain encoding types whose variants are defined by explicit values such as AlgId must spell out their discriminants (G.TYP.ENM.07).
- At most 5 function parameters; replace several `bool` parameters with an enum or a config struct (G.FUD.01, G.FUD.03).
- No glob imports `use foo::*`, except `use super::*` in test modules (G.MOD.03).
- Minimal visibility: private by default, `pub(crate)` as needed, public API re-exported from `lib.rs` (P.MOD.01, G.MOD.02).
- Cargo feature names are affirmative and free of redundant prefixes/suffixes (P.NAM.02, G.CAR.03); do not abuse features (P.CAR.02).

### 5.7 Async and concurrency (off-chain services)
- The async runtime is **tokio**; never block inside async contexts (G.ASY.05); use `spawn_blocking` for CPU-heavy work.
- Never hold a synchronous lock across `.await` (G.ASY.02); prefer channels / message passing.

### 5.8 unsafe
- `forbid` by default. For a justified exception (FFI, a reviewed performance-critical path): relax to `deny` locally in that crate and `allow` the specific block; put a `// SAFETY:` comment stating the invariant before every `unsafe` block (P.UNS.SAS.09); document `# Safety` on public unsafe functions (G.UNS.SAS.01); the user must approve.

---

## 6. Cryptography coding rules

1. **Use cryptography only through `ac-crypto`**: other crates must not depend directly on `ml-dsa`, `ml-kem`, `x-wing`, `blake3` or other low-level libraries (can be enforced with `cargo deny` bans).
2. **Signatures always carry a context string**: format `agentcoin/<purpose>/v<version>` (e.g. `agentcoin/tx/v1`, `agentcoin/bft-vote/v1`, `agentcoin/receipt/v1`). Register every new purpose in the context registry in `crates/ac-crypto/README.md`; **never reuse** an existing purpose's context.
3. **Domain-separated hashing**: context format `agentcoin <YYYY-MM> <purpose> v<version>`, also registered and never reused.
4. **Stable wire formats**: AlgId numbers, tagged encodings and the account-ID derivation rule **never change** once published; changes require a new number / new context version. The related regression tests must never be deleted or loosened.
5. **Secret material**: implement `Zeroize` / `ZeroizeOnDrop`; redact `Debug` output; never log it, put it in error messages, or serialize it to unencrypted storage.
6. **Randomness**: only cryptographically secure sources (`rand_core::CryptoRng`); deterministic seeded RNGs in tests must be confined to `#[cfg(test)]` or a test feature.
7. **Compare secrets in constant time** (`subtle`); `==` is forbidden.
8. **Test vectors**: every algorithm must pass official vectors (NIST ACVP, IETF draft appendices); vector files record the source URL, upstream SHA-256 and the filtering rules, and are reproducible by script.
9. **Verification failures always return an error / `false`**; never panic, never fall back to another algorithm.

---

## 7. Rust coding guidelines (docs/rust-guidelines)

The project adopts the *Rust Coding Guidelines* (Chinese edition, MIT licensed) as its baseline for style and practice. A local copy lives in `docs/rust-guidelines/`; its content is upstream Chinese and is not translated.

### 7.1 How to look things up
- **Index**: `docs/rust-guidelines/INDEX.md`
  - §1 AgentCoin focus rules (**read before coding**)
  - §2 AgentCoin overrides (items overridden or not adopted)
  - §3 table of contents, §4 all 255 items with file links
- **Search by keyword or ID**:
  ```bash
  grep -n '整数\|溢出' docs/rust-guidelines/rules.tsv     # "integer", "overflow"
  grep -n 'G.TYP.INT.01' docs/rust-guidelines/rules.tsv
  ```
  Then open the referenced `src/...md` to read the good / bad examples.
- ID meaning: `P.*` are principles (directional), `G.*` are guidelines (concrete, mostly checkable by clippy).

### 7.2 Requirements
- Before coding, read §1 of INDEX.md; when touching a feature area (unsafe, async, macros, no_std, integers, strings, …) search the corresponding chapter first.
- During code review (including self-review) check against the relevant items; when deliberately deviating from an item, state its ID and the reason in the commit / PR description.
- The guideline files are a **read-only upstream snapshot**: never edit `docs/rust-guidelines/src/`; update with `scripts/update-rust-guidelines.sh`; regenerate the index with `scripts/gen-rust-guidelines-index.py`; never hand-edit `INDEX.md` / `rules.tsv`.

### 7.3 AgentCoin overrides of upstream items
| Item | AgentCoin rule |
|---|---|
| P.ERR.02, G.ERR.02 | `unwrap` / `expect` are **forbidden** in non-test code (stricter than upstream), see §5.3 |
| P.NAM.09 | `G_` prefix not adopted; statics use `SCREAMING_SNAKE_CASE` |
| G.TYP.BOL.07 | Not adopted; use `!` for negation |
| P.CMT.04 | No per-file copyright header required (repository-level LICENSE) |
| G.MTH.LCK.03 / 04 | Not applicable to the runtime (no_std); off-chain services may use `parking_lot` / `crossbeam` / `tokio::sync` |
| G.TYP.SCT.01, G.TYP.ENM.05 | Adopted by default; wire-format / on-chain encoding enums are exempt (see §5.6) |
| (addition) | On-chain and economics code: no floating point, no `HashMap`, no unbounded collections (§5.4, §5.5) |

To add an override: update this table (in both language versions) and `OVERRIDES` in `scripts/gen-rust-guidelines-index.py`, then regenerate the index.

---

## 8. Substrate / runtime rules (from M1)

- **The runtime never panics**: every extrinsic returns `DispatchResult`; storage failures return errors.
- **Every extrinsic has a benchmark and weights** (`frame-benchmarking`); hard-coded placeholder weights must not reach a testnet.
- **Storage**: bounded types only; storage layout changes ship with a migration (`OnRuntimeUpgrade`) and migration tests; well-known storage keys used by constitution layer 1 are never renamed or reformatted.
- `on_initialize` / `on_finalize` work is bounded and accounted in weights.
- **Emission, burning, slashing**: every path has a "total conservation" test (minted − burned = change in issuance).
- **Governance parameters**: every one is registered with guardrail bounds (`pallet-guardrails`); no unbounded governable parameter.
- Any change to the node invariants (`ac-invariants`) is a **hard-fork-level change**: it needs its own OpenSpec change and explicit user confirmation.

---

## 9. Testing rules

- **Every spec Scenario maps to at least one automated test**; the test name or a comment names the Requirement / Scenario for traceability.
- Test types:
  - Unit tests: `#[cfg(test)] mod tests` in the same file, or integration tests under `tests/` (P.MOD.02 suggests moving large tests into separate files).
  - Vector tests: cryptography and encoding formats (`tests/vectors/`).
  - Property tests: encode/decode round trips, arithmetic conservation, etc. with `proptest`.
  - Fuzzing: decoders and all code handling external input (`cargo fuzz`, from M1).
  - End-to-end: multi-node networks (`tests/e2e/`); economic simulation (`tests/sim/`).
- Tests are deterministic: fixed seeds, no network (except the vector fetch scripts), no system time.
- **Never delete, skip (`#[ignore]`) or loosen tests to make CI pass**; find the root cause of failures.

---

## 10. Documentation and comments

- **Language policy**:
  - **Project documents are bilingual, English first**: the English version lives at the canonical path (e.g. `docs/decisions.md`); the Simplified Chinese version is the sibling `*.zh-CN.md` (e.g. `docs/decisions.zh-CN.md`).
  - The first line of an English page is `> 🌐 **English** | [简体中文](<name>.zh-CN.md)`; of a Chinese page, `> 🌐 [English](<name>.md) | **简体中文**`.
  - **When editing either language version, update the other in the same commit**; both must agree on decisions, numbers, tables and conclusions. English is authoritative: on divergence, follow English and fix the Chinese.
  - Create both versions when adding a new document.
  - **OpenSpec artifacts (`openspec/`) are Simplified Chinese only**; no English version.
  - Third-party upstream snapshots (`docs/rust-guidelines/src/` and the generated `INDEX.md`, `rules.tsv`) keep the upstream language and are not translated.
  - **Code identifiers, rustdoc, code comments and commit messages are in English** (for international open-source collaboration); crate `README.md` files are bilingual too (`README.md` + `README.zh-CN.md`).
  - **Communication with the user is in Simplified Chinese**: replies, progress reports, and workflow or change summaries (e.g. after apply, archive, or a CI round) are written in Simplified Chinese, unless the user asks otherwise in the current conversation.
- Every public item has rustdoc; `Result`-returning items document `# Errors`, items that can panic document `# Panics` (there should be none in this project), unsafe items document `# Safety` (G.CMT.01, G.CMT.02, G.UNS.SAS.01).
- Comments explain "why", not "what" (P.CMT.01); use `//` line comments (P.CMT.03); `TODO` / `FIXME` carry a short explanation (P.CMT.05).
- Every crate has `README.md` (and `README.zh-CN.md`): purpose, feature flags, a minimal example (the examples in the English version run as doctests).
- When interfaces or behavior change, update the related READMEs and `docs/` in both languages.

---

## 11. Git and commit rules

- Work on the branch the user specified; never push to other branches, never force-push, never rewrite pushed history.
- **Commit messages**: English, Conventional Commits style: `feat(ac-crypto): ...`, `fix(...)`, `docs: ...`, `chore: ...`, `test: ...`, `ci: ...`; the body explains motivation and impact and cites the OpenSpec change name and task number (e.g. `m0-foundation-pq-crypto 4.2`).
- Append the attribution lines required by the system prompt; **never** put model identifiers in commits, PRs or code.
- Commit in small steps: one task (or one tightly related task group) per commit; every commit passes fmt / clippy / test.
- Run the full checks (§13) before pushing; on network failures retry up to 4 times with 2s, 4s, 8s, 16s backoff.
- Do not open pull requests unless the user explicitly asks.
- Never commit: build output (`target/`), OpenSpec intermediates (`openspec/changes/`), secrets, personal local config (see `.gitignore`).

---

## 12. Agent conduct

1. **Read before writing**: read a file and its tests before modifying it; search all call sites before changing an interface.
2. **Do not widen scope**: do only what the current task requires; record and report other issues you find instead of fixing them on the side.
3. **Do not guess key facts**: library APIs, versions and protocol details come from actual source / docs / compiler output; verify when unsure.
4. **Ask when ambiguous**: questions that would change specs, interfaces, acceptance criteria, or contradict decisions must go to the user first; minor details may be assumed and recorded in the artifacts.
5. **Report honestly**: state test failures, skipped steps and unverified parts with the output; state completed and verified work plainly.
6. **Never bypass gates**: do not disable lints, delete tests or loosen CI to "pass".
7. **Protect user data and secrets**: never write secrets to logs, commits or external services.
8. **Leave a decision trail**: record new decisions made during implementation in `docs/decisions.md` (new number, both languages) and reference them from the OpenSpec change.
9. **Report progress on long tasks**: at each milestone say briefly what was done and what comes next.

---

## 13. Definition of Done

A task / change is done only when all of the following hold:

- [ ] The item in tasks.md is ticked and its acceptance check was actually run and passed
- [ ] `cargo fmt --all -- --check` passes
- [ ] `cargo clippy --workspace --all-targets --all-features -- -D warnings` passes
- [ ] `cargo test --workspace --all-features` passes
- [ ] For runtime-usable crates: `cargo build -p <crate> --no-default-features --target wasm32-unknown-unknown` passes
- [ ] `cargo deny check` and `cargo audit` pass
- [ ] New / changed public APIs have rustdoc; related READMEs / docs are updated (English and `*.zh-CN.md` in sync)
- [ ] Every related spec Scenario has a test
- [ ] No §2 red line is violated; deliberate deviations from the coding guidelines are noted with ID and reason
- [ ] Committed and pushed to the designated branch; CI is green

---

## 14. Common commands

```bash
# Quality checks (same as CI)
cargo fmt --all -- --check
# (--all-features enables revive's benchmarks; SKIP_PALLET_REVIVE_FIXTURES skips compiling its
#  test contracts, which need a RISC-V toolchain and resolc — see runtime/README.md)
SKIP_WASM_BUILD=1 SKIP_PALLET_REVIVE_FIXTURES=1 cargo clippy --workspace --all-targets --all-features -- -D warnings
SKIP_PALLET_REVIVE_FIXTURES=1 cargo test --workspace --all-features
cargo build -p ac-crypto --no-default-features --target wasm32-unknown-unknown
cargo deny check
cargo audit

# Supply chain, licences and advisories
scripts/check-license-boundary.sh [--self-test]     # licence zones (D47) and internal features
scripts/check-release-runtime.sh <wasm>|--self-test # the release runtime has no benchmarking API
scripts/sync-audit-exceptions.py [--write]        # advisory exceptions (edit audit-exceptions.toml)
scripts/benchmark-pallet.sh <pallet> <weights.rs> # regenerate benchmarked weights
scripts/gen-toploc-vectors.sh                     # regenerate TOPLOC vectors from the reference (Python)

# Node and end-to-end
cargo build -p ac-node -p ac-wallet
scripts/run-local-testnet.sh [--check]            # four-node local testnet (best + finalized)
AC_E2E=1 cargo test -p ac-e2e -- --test-threads 1  # multi-node acceptance tests
AC_E2E=1 cargo test -p ac-e2e --test market -- --test-threads 1  # market registration via the wallet
AC_E2E=1 cargo test -p ac-e2e --test settlement -- --test-threads 1  # receipts, report, claims
scripts/measure-finality.sh [seconds]             # finality latency, 4/7/10 nodes (release)
scripts/wallet-smoke.sh                           # wallet CLI against a dev node

# OpenSpec
openspec list                 # in-progress changes
openspec list --specs         # existing capability specs
openspec status --change <name>
openspec validate <name> --strict

# Rust coding guidelines
grep -n '<keyword or ID>' docs/rust-guidelines/rules.tsv
python3 scripts/gen-rust-guidelines-index.py      # rebuild the index
scripts/update-rust-guidelines.sh [<commit>]      # update the snapshot
```
