//! AC-BFT wire formats never change: the committed vectors match a fresh construction byte
//! for byte (AGENT.md §6.4).
// Integration-test crate: AGENT.md §5.3 permits unwrap/expect/panics (and indexing) in test code.
#![allow(clippy::unwrap_used, clippy::indexing_slicing)]

mod common {
    pub mod ac_bft_vectors;
}

#[test]
fn committed_vectors_match() {
    let committed = include_str!("vectors/ac_bft_messages.json");
    assert_eq!(committed, common::ac_bft_vectors::json());
}
