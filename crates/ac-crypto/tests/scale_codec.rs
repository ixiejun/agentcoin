//! Requirement "单一字节形式与准确的链上类型元数据" (crypto/algorithm-agility).
#![cfg(feature = "scale")]
// Integration-test crate: AGENT.md §5.3 permits unwrap/expect/panics in test code.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable,
    clippy::indexing_slicing
)]

use ac_crypto::{KemAlg, KemCiphertext, KemPublicKey, PqPublicKey, PqSignature, SigAlg};
use parity_scale_codec::{Decode, Encode, MaxEncodedLen};
use scale_info::{TypeDef, TypeInfo};

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
    let ct = KemCiphertext::new(KemAlg::XWing, &[3u8; 1120]).unwrap();
    assert_eq!(ct.encode(), ct.to_canonical());
}

#[test]
fn scale_decoding_rejects_unknown_and_truncated() {
    assert!(PqSignature::decode(&mut &[0xEEu8, 0, 0][..]).is_err());
    let enc = PqPublicKey::new(SigAlg::MlDsa44, &[1u8; 1312])
        .unwrap()
        .encode();
    assert!(PqPublicKey::decode(&mut &enc[..enc.len() - 1]).is_err());
}

#[test]
fn max_encoded_len_is_the_largest_variant() {
    assert_eq!(PqPublicKey::max_encoded_len(), 1 + 2592);
    assert_eq!(PqSignature::max_encoded_len(), 1 + 4627);
    assert_eq!(KemPublicKey::max_encoded_len(), 1 + 1216);
    assert_eq!(KemCiphertext::max_encoded_len(), 1 + 1120);
}

/// Extracts `(variant index, fixed payload length)` from a tagged type's metadata.
fn variants<T: TypeInfo + 'static>() -> Vec<(u8, u32)> {
    let ty = T::type_info();
    let TypeDef::Variant(def) = ty.type_def else {
        panic!("not an enum")
    };
    def.variants
        .iter()
        .map(|v| {
            assert_eq!(v.fields.len(), 1, "{} must have one payload field", v.name);
            let payload = v.fields[0].ty.type_info();
            // `Box<[u8; N]>` is transparent in metadata: the payload is `[u8; N]`.
            let TypeDef::Array(arr) = payload.type_def else {
                panic!("payload is not an array")
            };
            let elem = arr.type_param.type_info();
            assert!(matches!(
                elem.type_def,
                TypeDef::Primitive(scale_info::TypeDefPrimitive::U8)
            ));
            (v.index, arr.len)
        })
        .collect()
}

// Scenario "类型元数据与字节格式一致".
#[test]
fn type_info_matches_the_alg_table() {
    assert_eq!(
        variants::<PqSignature>(),
        [(0x01, 2420), (0x02, 3309), (0x03, 4627)]
    );
    assert_eq!(
        variants::<PqPublicKey>(),
        [(0x01, 1312), (0x02, 1952), (0x03, 2592)]
    );
    assert_eq!(variants::<KemPublicKey>(), [(0x01, 1216)]);
    assert_eq!(variants::<KemCiphertext>(), [(0x01, 1120)]);
    for (index, len) in variants::<PqSignature>() {
        let alg = SigAlg::from_id(index).unwrap();
        assert_eq!(alg.signature_len().unwrap(), len as usize);
    }
}
