//! Prints `tests/vectors/ac_bft_messages.json`: byte forms of AC-BFT signing payloads, signed
//! messages, a finality proof, a set-change digest and equivocation evidence, all built from
//! the public development keys with deterministic ML-DSA signing.
//!
//! Run with
//! `cargo run -p ac-primitives --example gen_ac_bft_vectors > crates/ac-primitives/tests/vectors/ac_bft_messages.json`.
//! The committed output is a regression vector and must never change.
// Tooling example: AGENT.md §5.3 permits unwrap and indexing in test code and test tooling.
#![allow(clippy::unwrap_used, clippy::indexing_slicing)]

#[path = "../tests/common/ac_bft_vectors.rs"]
mod vectors;

fn main() {
    print!("{}", vectors::json());
}
