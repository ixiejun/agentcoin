//! Requirement "稳定的算法编号表" and "未知与预留算法的安全处理" (crypto/algorithm-agility).
// Integration-test crate: AGENT.md §5.3 permits unwrap/expect/panics in test code.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable,
    clippy::indexing_slicing
)]

use ac_crypto::{Error, KemAlg, PROOF_SYSTEM_RESERVED, SigAlg};

/// The published table. Changing any row is a breaking, consensus-level change.
const SIG_TABLE: [(SigAlg, u16, bool); 6] = [
    (SigAlg::MlDsa44, 0x0101, true),
    (SigAlg::MlDsa65, 0x0102, true),
    (SigAlg::MlDsa87, 0x0103, true),
    (SigAlg::SlhDsaSha2_128s, 0x0201, false),
    (SigAlg::FnDsa512, 0x0301, false),
    (SigAlg::XmssLean, 0x0401, false),
];
const KEM_TABLE: [(KemAlg, u16, bool); 2] = [
    (KemAlg::XWing, 0x1101, true),
    (KemAlg::MlKem1024, 0x1102, false),
];

// Scenario "编号表被固化".
#[test]
fn table_is_frozen() {
    assert_eq!(SigAlg::ALL.len(), SIG_TABLE.len());
    assert_eq!(KemAlg::ALL.len(), KEM_TABLE.len());
    for (alg, id, implemented) in SIG_TABLE {
        assert_eq!(alg.id(), id);
        assert_eq!(alg.is_implemented(), implemented);
    }
    for (alg, id, implemented) in KEM_TABLE {
        assert_eq!(alg.id(), id);
        assert_eq!(alg.is_implemented(), implemented);
    }
}

// Scenario "已实现算法的编号往返".
#[test]
fn round_trip() {
    for alg in SigAlg::ALL {
        assert_eq!(SigAlg::from_id(alg.id()), Ok(alg));
    }
    for alg in KemAlg::ALL {
        assert_eq!(KemAlg::from_id(alg.id()), Ok(alg));
    }
}

// Scenario "未知 AlgId".
#[test]
fn unknown_ids() {
    for id in [0x0000, 0x0104, 0xFFFF, 0x1101] {
        assert_eq!(SigAlg::from_id(id), Err(Error::UnknownAlgorithm(id)));
    }
    assert_eq!(
        KemAlg::from_id(0x0101),
        Err(Error::UnknownAlgorithm(0x0101))
    );
    // The proof-system range is reserved but unallocated.
    for id in [*PROOF_SYSTEM_RESERVED.start(), *PROOF_SYSTEM_RESERVED.end()] {
        assert_eq!(SigAlg::from_id(id), Err(Error::UnknownAlgorithm(id)));
        assert_eq!(KemAlg::from_id(id), Err(Error::UnknownAlgorithm(id)));
    }
}

// Scenario "预留算法".
#[test]
fn reserved_algorithms_are_not_implemented() {
    assert_eq!(
        SigAlg::SlhDsaSha2_128s.public_key_len(),
        Err(Error::NotImplemented(0x0201))
    );
    assert_eq!(
        SigAlg::FnDsa512.signature_len(),
        Err(Error::NotImplemented(0x0301))
    );
    assert_eq!(
        KemAlg::MlKem1024.ciphertext_len(),
        Err(Error::NotImplemented(0x1102))
    );
}

// Requirement "带标签的线格式": fixed raw lengths.
#[test]
fn raw_lengths() {
    assert_eq!(SigAlg::MlDsa44.public_key_len(), Ok(1312));
    assert_eq!(SigAlg::MlDsa44.signature_len(), Ok(2420));
    assert_eq!(SigAlg::MlDsa65.public_key_len(), Ok(1952));
    assert_eq!(SigAlg::MlDsa65.signature_len(), Ok(3309));
    assert_eq!(SigAlg::MlDsa87.public_key_len(), Ok(2592));
    assert_eq!(SigAlg::MlDsa87.signature_len(), Ok(4627));
    assert_eq!(KemAlg::XWing.public_key_len(), Ok(1216));
    assert_eq!(KemAlg::XWing.ciphertext_len(), Ok(1120));
}
