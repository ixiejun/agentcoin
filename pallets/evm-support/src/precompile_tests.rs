//! Tests of the PQ precompiles (m4-evm tasks 3.1–3.3; spec evm/precompiles).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use crate::precompiles::{
    IBlake3, IPoseidon2, IPqVerify, IStarkVerify, blake3_output, poseidon2_output, pq_verify,
    pq_verify_output, pq_verify_weight,
};
use crate::weights::WeightInfo;
use ac_crypto::SigAlg;
use ac_crypto::sig::{SecretSeed, SigningKey};
use ac_primitives::evm::EVM_VERIFY_CONTEXT;
use alloy_core::sol_types::{SolCall, SolInterface, SolValue};
use proptest::prelude::{ProptestConfig, any, prop_assert_eq, proptest};

/// ACVP ML-DSA keyGen vectors kept by `ac-crypto` (source and filtering in its SOURCES.md).
const ACVP_KEYGEN: &str =
    include_str!("../../../crates/ac-crypto/tests/vectors/ml_dsa_keygen.json");
/// ACVP ML-DSA sigGen vectors (messages).
const ACVP_SIGGEN: &str =
    include_str!("../../../crates/ac-crypto/tests/vectors/ml_dsa_siggen.json");

struct Case {
    alg: SigAlg,
    key: SigningKey,
    message: Vec<u8>,
}

fn alg_of(name: &str) -> SigAlg {
    match name {
        "ML-DSA-44" => SigAlg::MlDsa44,
        "ML-DSA-65" => SigAlg::MlDsa65,
        "ML-DSA-87" => SigAlg::MlDsa87,
        other => panic!("{other}"),
    }
}

/// Three ACVP-derived cases per parameter set: the key comes from an ACVP keyGen seed (and must
/// reproduce the ACVP public key), the message from an ACVP sigGen case.
fn acvp_cases() -> Vec<Case> {
    let keygen: Vec<serde_json::Value> = serde_json::from_str(ACVP_KEYGEN).unwrap();
    let siggen: Vec<serde_json::Value> = serde_json::from_str(ACVP_SIGGEN).unwrap();
    let mut cases = Vec::new();
    for set in ["ML-DSA-44", "ML-DSA-65", "ML-DSA-87"] {
        let keys = keygen.iter().filter(|v| v["parameterSet"] == set).take(3);
        let msgs = siggen.iter().filter(|v| v["parameterSet"] == set).take(3);
        for (k, m) in keys.zip(msgs) {
            let alg = alg_of(set);
            let seed: [u8; 32] = hex::decode(k["seed"].as_str().unwrap())
                .unwrap()
                .try_into()
                .unwrap();
            let key = SigningKey::from_seed(alg, &SecretSeed::new(seed)).unwrap();
            assert_eq!(
                hex::encode_upper(key.public_key().unwrap().as_bytes()),
                k["pk"].as_str().unwrap(),
                "ACVP keyGen tcId {}",
                k["tcId"]
            );
            let message = hex::decode(m["message"].as_str().unwrap()).unwrap();
            cases.push(Case { alg, key, message });
        }
    }
    assert_eq!(cases.len(), 9);
    cases
}

fn call(alg: u8, pk: &[u8], msg: &[u8], sig: &[u8]) -> IPqVerify::verifyCall {
    IPqVerify::verifyCall {
        alg,
        publicKey: pk.to_vec().into(),
        message: msg.to_vec().into(),
        signature: sig.to_vec().into(),
    }
}

fn returns_bool(output: Vec<u8>) -> bool {
    bool::abi_decode(&output).unwrap()
}

// Requirement "pq_verify 验证 ML-DSA 签名" / Scenarios "有效签名" and "覆盖 ML-DSA 三个参数集".
#[test]
fn valid_signatures_verify() {
    for case in acvp_cases() {
        let sig = case
            .key
            .sign_deterministic(&case.message, EVM_VERIFY_CONTEXT)
            .unwrap();
        let pk = case.key.public_key().unwrap();
        let input = call(case.alg.id(), pk.as_bytes(), &case.message, sig.as_bytes());
        assert!(returns_bool(pq_verify_output(&input)), "{:?}", case.alg);
        // Through the ABI: the calldata a contract sends decodes to the same call.
        let calldata = input.abi_encode();
        let decoded = IPqVerify::IPqVerifyCalls::abi_decode(&calldata).unwrap();
        let IPqVerify::IPqVerifyCalls::verify(decoded) = decoded;
        assert!(returns_bool(pq_verify_output(&decoded)));
    }
}

// Scenario "协议签名不能在合约中验证".
#[test]
fn protocol_contexts_do_not_verify() {
    for case in acvp_cases() {
        let pk = case.key.public_key().unwrap();
        for context in [
            &b"agentcoin/tx/v1"[..],
            b"agentcoin/bft-vote/v1",
            b"agentcoin/validator-pop/v1",
            b"",
        ] {
            let sig = case.key.sign_deterministic(&case.message, context).unwrap();
            assert!(!pq_verify(
                case.alg.id(),
                pk.as_bytes(),
                &case.message,
                sig.as_bytes()
            ));
        }
    }
}

// Scenario "篡改与未知算法".
#[test]
fn tampering_and_unknown_algorithms_are_false() {
    for case in acvp_cases() {
        let pk = case.key.public_key().unwrap();
        let sig = case
            .key
            .sign_deterministic(&case.message, EVM_VERIFY_CONTEXT)
            .unwrap();
        let (pk, sig) = (pk.as_bytes().to_vec(), sig.as_bytes().to_vec());
        // One flipped bit in the message, the signature or the key.
        let mut msg = case.message.clone();
        msg.push(0);
        msg[0] ^= 1;
        assert!(!pq_verify(case.alg.id(), &pk, &msg, &sig));
        let mut bad_sig = sig.clone();
        bad_sig[10] ^= 0x80;
        assert!(!pq_verify(case.alg.id(), &pk, &case.message, &bad_sig));
        let mut bad_pk = pk.clone();
        bad_pk[5] ^= 0x01;
        assert!(!pq_verify(case.alg.id(), &bad_pk, &case.message, &sig));
        // Wrong lengths.
        assert!(!pq_verify(case.alg.id(), &pk[1..], &case.message, &sig));
        assert!(!pq_verify(case.alg.id(), &pk, &case.message, &sig[1..]));
        // Unassigned, extension-marker and reserved-but-unimplemented AlgIds, and another
        // parameter set with the same bytes: false, no fallback.
        for alg in [0x00u8, 0x04, 0x10, 0x20, 0x30, 0xff] {
            assert!(!pq_verify(alg, &pk, &case.message, &sig), "alg {alg:#x}");
        }
        let other = if case.alg == SigAlg::MlDsa44 {
            SigAlg::MlDsa65
        } else {
            SigAlg::MlDsa44
        };
        assert!(!pq_verify(other.id(), &pk, &case.message, &sig));
    }
}

// Scenario "覆盖 ML-DSA 三个参数集": each parameter set is charged its own benchmarked weight,
// with the message length as the linear component; rejections cost the rejection weight.
#[test]
fn charges_follow_the_parameter_set() {
    type W = ();
    for m in [0usize, 32, 1000] {
        let m32 = u32::try_from(m).unwrap();
        assert_eq!(pq_verify_weight::<W>(0x01, m), W::pq_verify_ml_dsa_44(m32));
        assert_eq!(pq_verify_weight::<W>(0x02, m), W::pq_verify_ml_dsa_65(m32));
        assert_eq!(pq_verify_weight::<W>(0x03, m), W::pq_verify_ml_dsa_87(m32));
        for alg in [0x00u8, 0x10, 0x20, 0x30, 0xff] {
            assert_eq!(pq_verify_weight::<W>(alg, m), W::pq_verify_rejected());
        }
    }
    assert!(W::pq_verify_ml_dsa_44(0).ref_time() < W::pq_verify_ml_dsa_87(0).ref_time());
}

// Requirement "blake3 预编译" / Scenario "官方向量" (BLAKE3 reference `test_vectors.json`,
// input byte i = i mod 251).
#[test]
fn blake3_official_vectors() {
    let input = |n: usize| {
        (0..n)
            .map(|i| u8::try_from(i % 251).unwrap())
            .collect::<Vec<u8>>()
    };
    for (n, expected) in [
        (
            0,
            "af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262",
        ),
        (
            1,
            "2d3adedff11b61f14c886e35afa036736dcd87a74d27b5c1510225d0f592e213",
        ),
        (
            1024,
            "42214739f095a406f3fc83deb889744ac00df831c10daa55189b5d121c855af7",
        ),
    ] {
        let out = blake3_output(&input(n));
        assert_eq!(hex::encode(&out), expected, "len {n}");
        let call = IBlake3::hashCall {
            data: input(n).into(),
        };
        let decoded = IBlake3::IBlake3Calls::abi_decode(&call.abi_encode()).unwrap();
        let IBlake3::IBlake3Calls::hash(decoded) = decoded;
        assert_eq!(blake3_output(&decoded.data), out);
    }
}

// Requirement "poseidon2 预编译" / Scenario "向量一致": the regression vectors of
// `crypto/hashing` (input byte i = i mod 251) through the ABI, equal to `ac-crypto`.
#[test]
fn poseidon2_vectors() {
    let vectors: serde_json::Value = serde_json::from_str(include_str!(
        "../../../crates/ac-crypto/tests/vectors/poseidon2_hash.json"
    ))
    .unwrap();
    let input = |n: usize| {
        (0..n)
            .map(|i| u8::try_from(i % 251).unwrap())
            .collect::<Vec<u8>>()
    };
    for case in vectors["cases"].as_array().unwrap() {
        let n = usize::try_from(case["input_len"].as_u64().unwrap()).unwrap();
        let call = IPoseidon2::hashCall {
            data: input(n).into(),
        };
        let IPoseidon2::IPoseidon2Calls::hash(decoded) =
            IPoseidon2::IPoseidon2Calls::abi_decode(&call.abi_encode()).unwrap();
        let out = poseidon2_output(&decoded.data).unwrap();
        assert_eq!(hex::encode(&out), case["hash"].as_str().unwrap(), "len {n}");
        assert_eq!(out, ac_crypto::poseidon2::hash(&input(n)).unwrap().to_vec());
    }
    // Charged per input byte.
    type W = crate::weights::SubstrateWeight<crate::mock::Test>;
    assert!(W::poseidon2(1_000).ref_time() > W::poseidon2(0).ref_time());
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    // Scenario "与 ac-crypto 一致": the precompile output equals an independent BLAKE3.
    #[test]
    fn blake3_matches_reference(data in proptest::collection::vec(any::<u8>(), 0..2048)) {
        prop_assert_eq!(blake3_output(&data), blake3::hash(&data).as_bytes().to_vec());
    }
}

// Requirement "预编译地址固定" / Scenario "无法解码的输入": calldata that does not match the
// interface does not decode, and revive reverts such a call before the precompile runs.
#[test]
fn undecodable_input_is_rejected() {
    let good = IBlake3::hashCall {
        data: vec![1, 2, 3].into(),
    }
    .abi_encode();
    assert!(IBlake3::IBlake3Calls::abi_decode(&good).is_ok());
    for bad in [
        vec![],
        vec![0xde, 0xad, 0xbe, 0xef],
        good[..good.len() - 40].to_vec(),
        IPqVerify::verifyCall {
            alg: 1,
            publicKey: vec![].into(),
            message: vec![].into(),
            signature: vec![].into(),
        }
        .abi_encode(),
    ] {
        assert!(
            IBlake3::IBlake3Calls::abi_decode(&bad).is_err(),
            "{bad:02x?}"
        );
    }
    assert!(IPqVerify::IPqVerifyCalls::abi_decode(&[0u8; 3]).is_err());
    // The reserved interface decodes, but the precompile always reverts (see runtime tests).
    let stark = IStarkVerify::verifyCall {
        data: vec![].into(),
    }
    .abi_encode();
    assert!(IStarkVerify::IStarkVerifyCalls::abi_decode(&stark).is_ok());
}
