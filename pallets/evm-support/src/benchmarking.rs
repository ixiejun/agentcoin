//! Benchmarks of the PQ precompiles' work (m4-evm task 3.3).
//!
//! Each benchmark measures exactly what the precompile does after decoding its ABI input:
//! building the tagged key and signature and verifying (or hashing) through `ac-crypto`.
//! The signatures are deterministic and valid, so every run takes the full verification path.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // Benchmark setup.

use alloc::vec;
use alloc::vec::Vec;

use ac_crypto::SigAlg;
use ac_crypto::sig::{SecretSeed, SigningKey};
use ac_primitives::evm::EVM_VERIFY_CONTEXT;
// The `benchmarks` macro expands to code naming `impl_test_function` unqualified.
use frame_benchmarking::impl_test_function;
use frame_benchmarking::v2::benchmarks;

use crate::precompiles::pq_verify;
use crate::{Config, Pallet};

/// Largest message benchmarked (the linear model extrapolates beyond it).
const MAX_MESSAGE: u32 = 16 * 1024;

/// A valid `(alg, public key, message, signature)` for an `m`-byte message.
fn signed(alg: SigAlg, m: u32) -> (u8, Vec<u8>, Vec<u8>, Vec<u8>) {
    let key = SigningKey::from_seed(alg, &SecretSeed::new([7; 32])).unwrap();
    let message = vec![0x5a; m as usize];
    let signature = key
        .sign_deterministic(&message, EVM_VERIFY_CONTEXT)
        .unwrap();
    let public_key = key.public_key().unwrap();
    (
        alg.id(),
        public_key.as_bytes().to_vec(),
        message,
        signature.as_bytes().to_vec(),
    )
}

#[benchmarks]
mod benchmarks {
    use super::{Config, MAX_MESSAGE, Pallet, SigAlg, impl_test_function, pq_verify, signed, vec};

    #[benchmark]
    fn pq_verify_ml_dsa_44(m: Linear<0, MAX_MESSAGE>) {
        let (alg, pk, msg, sig) = signed(SigAlg::MlDsa44, m);
        let valid;
        #[block]
        {
            valid = pq_verify(alg, &pk, &msg, &sig);
        }
        assert!(valid);
    }

    #[benchmark]
    fn pq_verify_ml_dsa_65(m: Linear<0, MAX_MESSAGE>) {
        let (alg, pk, msg, sig) = signed(SigAlg::MlDsa65, m);
        let valid;
        #[block]
        {
            valid = pq_verify(alg, &pk, &msg, &sig);
        }
        assert!(valid);
    }

    #[benchmark]
    fn pq_verify_ml_dsa_87(m: Linear<0, MAX_MESSAGE>) {
        let (alg, pk, msg, sig) = signed(SigAlg::MlDsa87, m);
        let valid;
        #[block]
        {
            valid = pq_verify(alg, &pk, &msg, &sig);
        }
        assert!(valid);
    }

    #[benchmark]
    fn pq_verify_rejected() {
        let (_, pk, msg, sig) = signed(SigAlg::MlDsa44, 32);
        let valid;
        #[block]
        {
            // A reserved algorithm: rejected before any verification.
            valid = pq_verify(SigAlg::SlhDsaSha2_128s.id(), &pk, &msg, &sig);
        }
        assert!(!valid);
    }

    #[benchmark]
    fn blake3(n: Linear<0, { 4 * MAX_MESSAGE }>) {
        let data = vec![0xa5; n as usize];
        let digest;
        #[block]
        {
            digest = ac_crypto::hash::blake3_256(&data);
        }
        assert_ne!(digest, [0; 32]);
    }

    impl_benchmark_test_suite!(Pallet, crate::mock::new_test_ext(), crate::mock::Test);
}
