//! Every hashing context registered in the README is well formed (AGENT.md §6.3).
// Integration-test crate: AGENT.md §5.3 permits unwrap/expect/panics in test code.
#![allow(clippy::unwrap_used)]

use ac_crypto::hash::validate_context;

/// The registered hashing contexts, as listed in README.md.
const REGISTERED: [&str; 25] = [
    "agentcoin 2026-09 account-id v1",
    "agentcoin 2026-09 test-rng v1",
    "agentcoin 2026-09 tx-payload v1",
    "agentcoin 2026-09 wallet-key v1",
    "agentcoin 2026-09 dev-seed v1",
    "agentcoin 2026-09 keystore-aad v1",
    "agentcoin 2026-09 os-rng v1",
    "agentcoin 2026-09 bft-message v1",
    "agentcoin 2026-09 randomness-secret v1",
    "agentcoin 2026-09 randomness-commit v1",
    "agentcoin 2026-09 randomness v1",
    "agentcoin 2026-09 randomness-subject v1",
    "agentcoin 2026-09 model-id v1",
    "agentcoin 2026-09 voucher-payload v1",
    "agentcoin 2026-09 receipt-payload v1",
    "agentcoin 2026-09 receipt-leaf v1",
    "agentcoin 2026-09 receipt-node v1",
    "agentcoin 2026-09 toploc-commit v1",
    "agentcoin 2026-09 sealed-recipient v1",
    "agentcoin 2026-09 sealed-handshake v1",
    "agentcoin 2026-09 sealed-key v1",
    "agentcoin 2026-09 gateway-kem-payload v1",
    "agentcoin 2026-10 audit-evidence v1",
    "agentcoin 2026-10 audit-assign v1",
    "agentcoin 2026-10 audit-review v1",
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
    for context in SIGNATURE_CONTEXTS {
        assert!(readme.contains(context) && readme_zh.contains(context));
    }
}

/// The registered signature contexts, as listed in README.md.
const SIGNATURE_CONTEXTS: [&str; 10] = [
    "agentcoin/tx/v1",
    "agentcoin/aura-seal/v1",
    "agentcoin/key-rotation/v1",
    "agentcoin/bft-vote/v1",
    "agentcoin/validator-pop/v1",
    "agentcoin/receipt/v1",
    "agentcoin/evm-verify/v1",
    "agentcoin/voucher/v1",
    "agentcoin/sealed-channel/v1",
    "agentcoin/gateway-kem/v1",
];

// A purpose's context is never reused (AGENT.md §6.2): the registry has no duplicates, so a
// signature made under one context never verifies under another.
#[test]
fn hash_contexts_are_distinct() {
    let mut seen = std::collections::BTreeSet::new();
    for context in REGISTERED {
        assert!(seen.insert(context), "{context} is registered twice");
    }
}

#[test]
fn signature_contexts_are_distinct() {
    let mut seen = std::collections::BTreeSet::new();
    for context in SIGNATURE_CONTEXTS {
        assert!(seen.insert(context), "{context} is registered twice");
        assert!(context.len() <= 255);
    }
}
