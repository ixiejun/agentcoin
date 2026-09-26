//! Every hashing context registered in the README is well formed (AGENT.md §6.3).
// Integration-test crate: AGENT.md §5.3 permits unwrap/expect/panics in test code.
#![allow(clippy::unwrap_used)]

use ac_crypto::hash::validate_context;

/// The registered hashing contexts, as listed in README.md.
const REGISTERED: [&str; 7] = [
    "agentcoin 2026-09 account-id v1",
    "agentcoin 2026-09 test-rng v1",
    "agentcoin 2026-09 tx-payload v1",
    "agentcoin 2026-09 wallet-key v1",
    "agentcoin 2026-09 dev-seed v1",
    "agentcoin 2026-09 keystore-aad v1",
    "agentcoin 2026-09 os-rng v1",
];

#[test]
fn registered_hash_contexts_are_well_formed() {
    for context in REGISTERED {
        validate_context(context).unwrap();
    }
}

#[test]
fn readme_lists_every_context() {
    let readme = include_str!("../README.md");
    let readme_zh = include_str!("../README.zh-CN.md");
    for context in REGISTERED {
        assert!(readme.contains(context), "README.md lacks {context}");
        assert!(
            readme_zh.contains(context),
            "README.zh-CN.md lacks {context}"
        );
    }
    for context in [
        "agentcoin/tx/v1",
        "agentcoin/aura-seal/v1",
        "agentcoin/key-rotation/v1",
    ] {
        assert!(readme.contains(context) && readme_zh.contains(context));
    }
}
