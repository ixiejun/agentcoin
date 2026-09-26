//! Property tests for the canonical wire format (crypto/algorithm-agility).
// Integration-test crate: AGENT.md §5.3 permits unwrap/expect/panics in test code.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable,
    clippy::indexing_slicing
)]

use ac_crypto::{KemAlg, KemCiphertext, KemPublicKey, PqPublicKey, PqSignature, SigAlg};
use proptest::prelude::{Just, Strategy, any, prop_oneof, proptest};

fn sig_alg() -> impl Strategy<Value = SigAlg> {
    prop_oneof![
        Just(SigAlg::MlDsa44),
        Just(SigAlg::MlDsa65),
        Just(SigAlg::MlDsa87)
    ]
}

fn bytes(len: usize) -> impl Strategy<Value = Vec<u8>> {
    proptest::collection::vec(any::<u8>(), len)
}

proptest! {
    #[test]
    fn public_key_round_trip((alg, raw) in sig_alg().prop_flat_map(|a| (Just(a), bytes(a.public_key_len().unwrap())))) {
        let pk = PqPublicKey::new(alg, &raw).unwrap();
        let enc = pk.to_canonical();
        assert_eq!(enc[0], alg.id());
        assert_eq!(enc.len(), 1 + alg.public_key_len().unwrap());
        assert_eq!(PqPublicKey::from_canonical(&enc).unwrap(), pk);
    }

    #[test]
    fn signature_round_trip((alg, raw) in sig_alg().prop_flat_map(|a| (Just(a), bytes(a.signature_len().unwrap())))) {
        let sig = PqSignature::new(alg, &raw).unwrap();
        assert_eq!(PqSignature::from_canonical(&sig.to_canonical()).unwrap(), sig);
    }

    #[test]
    fn kem_objects_round_trip(pk in bytes(1216), ct in bytes(1120)) {
        let pk = KemPublicKey::new(KemAlg::XWing, &pk).unwrap();
        let ct = KemCiphertext::new(KemAlg::XWing, &ct).unwrap();
        assert_eq!(KemPublicKey::from_canonical(&pk.to_canonical()).unwrap(), pk);
        assert_eq!(KemCiphertext::from_canonical(&ct.to_canonical()).unwrap(), ct);
    }

    #[test]
    fn wrong_lengths_never_decode(alg in sig_alg(), delta in 1usize..64, grow in any::<bool>()) {
        let expected = alg.signature_len().unwrap();
        let len = if grow { expected + delta } else { expected - delta };
        let mut enc = vec![alg.id()];
        enc.extend(std::iter::repeat_n(0u8, len));
        assert!(PqSignature::from_canonical(&enc).is_err());
    }
}
