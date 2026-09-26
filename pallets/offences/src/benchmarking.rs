//! Benchmarks: evidence verification during authorization and recording on dispatch.
//!
//! Evidence is built from the public development keys (the benchmark genesis uses them) and
//! signed deterministically, so runs are reproducible.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)] // Benchmark setup.

use ac_crypto::sig::SigningKey;
use ac_crypto::{SigAlg, dev_seed};
use ac_primitives::aura_pq::{SEAL_CONTEXT, Slot, pre_digest, seal_digest};
use ac_primitives::offences::{ChainHeader, EncodedHeader, Evidence};
use ac_primitives::validator_set::ValidatorSetInterface;
use frame_benchmarking::v2::benchmarks;
use parity_scale_codec::Encode;
use sp_core::H256;
use sp_runtime::Digest;
use sp_runtime::traits::Header as _;

use crate::{Config, Pallet};

/// The signing key of a current authority, found among the development keys.
fn authority_key<T: Config>() -> SigningKey {
    let (_, set) = T::ValidatorSet::current();
    ["alice", "bob", "charlie", "dave"]
        .iter()
        .map(|n| SigningKey::from_seed(SigAlg::MlDsa65, &dev_seed(n).unwrap()).unwrap())
        .find(|k| set.iter().any(|a| a.key == k.public_key().unwrap()))
        .expect("the benchmark genesis uses development authorities")
}

fn sealed(key: &SigningKey, root: u8) -> EncodedHeader {
    let slot = T_SLOT;
    let mut digest = Digest::default();
    digest.push(pre_digest(Slot::from(slot)));
    let mut header = ChainHeader::new(
        1,
        H256::repeat_byte(root),
        H256::zero(),
        H256::zero(),
        digest,
    );
    let sig = key
        .sign_deterministic(header.hash().as_ref(), SEAL_CONTEXT)
        .unwrap();
    header.digest_mut().push(seal_digest(&sig));
    EncodedHeader::try_from(header.encode()).unwrap()
}

/// Slot of the benchmark evidence: the genesis slot, always within the age limit.
const T_SLOT: u64 = 0;

fn evidence<T: Config>() -> Evidence {
    let key = authority_key::<T>();
    Evidence::AuraEquivocation {
        offender: key.public_key().unwrap(),
        first: sealed(&key, 1),
        second: sealed(&key, 2),
    }
}

#[benchmarks]
mod benchmarks {
    use super::{Config, Pallet, evidence};
    use crate::Call;
    // The macro expands to code naming `impl_test_function` unqualified.
    use frame_benchmarking::impl_test_function;
    use frame_benchmarking::v2::BenchmarkError;
    use frame_support::pallet_prelude::TransactionSource;

    /// Verification of seal evidence: two header decodes and hashes, two ML-DSA-65 checks.
    #[benchmark]
    fn authorize_report_equivocation() -> Result<(), BenchmarkError> {
        let evidence = evidence::<T>();
        let result;
        #[block]
        {
            result = Pallet::<T>::authorize_report(TransactionSource::External, &evidence);
        }
        assert!(result.is_ok());
        Ok(())
    }

    /// Recording: storage, disabling, slashing hook and event.
    #[benchmark]
    fn report_equivocation() -> Result<(), BenchmarkError> {
        let evidence = evidence::<T>();
        let origin = frame_system::RawOrigin::Authorized;
        #[extrinsic_call]
        _(origin, evidence);
        assert_eq!(Pallet::<T>::offences(0).len(), 1);
        Ok(())
    }

    impl_benchmark_test_suite!(Pallet, crate::mock::new_bench_ext(), crate::mock::Test);
}
