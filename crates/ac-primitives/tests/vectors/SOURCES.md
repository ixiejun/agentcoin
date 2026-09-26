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
