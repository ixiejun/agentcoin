# Test vector sources

- `ac_bft_messages.json`: repository regression vectors for AC-BFT wire formats — signing
  payloads (context `agentcoin 2026-09 bft-message v1`) of a prepare vote, a proposal and a
  timeout; a signed prepare vote in its version-1 envelope; a version-1 finality proof with
  three commit votes; an authority-set change digest (engine `acbf`); and BFT equivocation
  evidence. Genesis hash `0x11…11`, authority set 0, the public development keys `alice`,
  `bob`, `charlie`, `dave` (ML-DSA-65, context `agentcoin 2026-09 dev-seed v1`) and
  deterministic ML-DSA signing with context `agentcoin/bft-vote/v1`. Produced by
  `examples/gen_ac_bft_vectors.rs` from `tests/common/ac_bft_vectors.rs`; regression only —
  never change these values. The development keys are public: never use them on a live chain.
- `public_jobs.json`: repository regression vectors of the public jobs (m6-public-jobs): a
  worker's commitment (context `agentcoin 2026-10 public-commit v1`), a five-leaf canary tree
  with its root and SCALE-encoded proofs (`agentcoin 2026-10 public-canary v1`), a worker draw
  from a ten-account roster (`agentcoin 2026-10 public-assign v1`) and two fingerprint direction
  blocks (`agentcoin 2026-10 public-direction v1`). Produced by `tests/public_jobs_vectors.rs`
  with `AC_WRITE_VECTORS=1`; regression only — never change these values.
