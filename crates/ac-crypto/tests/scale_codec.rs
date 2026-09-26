//! Requirement "可选的 SCALE 编解码" (crypto/algorithm-agility).
#![cfg(feature = "scale")]
// Integration-test crate: AGENT.md §5.3 permits unwrap/expect/panics in test code.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable,
    clippy::indexing_slicing
)]

use ac_crypto::{KemAlg, KemPublicKey, PqPublicKey, PqSignature, SigAlg};
use parity_scale_codec::{Decode, Encode};

// Scenario "SCALE 编码与规范编码一致".
#[test]
fn scale_encoding_equals_canonical_encoding() {
    let sig = PqSignature::new(SigAlg::MlDsa65, &[5u8; 3309]).unwrap();
    assert_eq!(sig.encode(), sig.to_canonical());
    assert_eq!(PqSignature::decode(&mut &sig.encode()[..]).unwrap(), sig);

    let pk = PqPublicKey::new(SigAlg::MlDsa44, &[1u8; 1312]).unwrap();
    assert_eq!(pk.encode(), pk.to_canonical());
    assert_eq!(PqPublicKey::decode(&mut &pk.encode()[..]).unwrap(), pk);

    let kpk = KemPublicKey::new(KemAlg::XWing, &[2u8; 1216]).unwrap();
    assert_eq!(kpk.encode(), kpk.to_canonical());
}

#[test]
fn scale_decoding_rejects_unknown_and_truncated() {
    assert!(PqSignature::decode(&mut &[0xFFu8, 0xFF, 0, 0][..]).is_err());
    let enc = PqPublicKey::new(SigAlg::MlDsa44, &[1u8; 1312])
        .unwrap()
        .encode();
    assert!(PqPublicKey::decode(&mut &enc[..enc.len() - 1]).is_err());
}
