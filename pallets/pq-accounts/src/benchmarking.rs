//! Benchmarks: `rotate_key` (worst case ML-DSA-87) and `PqAuthorize` per algorithm.
//!
//! Keys are derived from fixed seeds and signed deterministically: benchmarks must be
//! reproducible and the runtime has no randomness source.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)] // Benchmark setup.

use alloc::vec;

use ac_crypto::sig::{SecretSeed, SigningKey};
use ac_crypto::{PqPublicKey, SigAlg};
// The `benchmarks` macro expands to code naming `Call` and `impl_test_function` unqualified.
use frame_benchmarking::impl_test_function;
use frame_benchmarking::v2::{BenchmarkError, benchmarks};
use frame_support::dispatch::{DispatchInfo, GetDispatchInfo};
use frame_support::pallet_prelude::TransactionSource;
use frame_system::RawOrigin;
use sp_runtime::traits::{AsTransactionAuthorizedOrigin, DispatchTransaction, Dispatchable, Zero};

use crate::{
    Call, Config, KEY_ROTATION_CONTEXT, Keys, Pallet, PqAuth, PqAuthorize, TX_SIGNING_CONTEXT,
    derived_account, rotation_statement, signing_payload,
};

fn signing_key(alg: SigAlg, seed: u8) -> SigningKey {
    SigningKey::from_seed(alg, &SecretSeed::new([seed; 32])).unwrap()
}

fn public(key: &SigningKey) -> PqPublicKey {
    key.public_key().unwrap()
}

/// Builds a signed `PqAuthorize` for a `remark` call; registers the key first unless
/// `first_transaction`.
fn authorize_setup<T: Config + Send + Sync>(
    alg: SigAlg,
    first_transaction: bool,
) -> (
    PqAuthorize<T>,
    <T as frame_system::Config>::RuntimeCall,
    DispatchInfo,
)
where
    <T as frame_system::Config>::RuntimeCall: GetDispatchInfo + From<frame_system::Call<T>>,
{
    let key = signing_key(alg, 1);
    let who = derived_account(&public(&key));
    if !first_transaction {
        Pallet::<T>::register(&who, public(&key));
    }
    let call: <T as frame_system::Config>::RuntimeCall =
        frame_system::Call::<T>::remark { remark: vec![] }.into();
    let info = call.get_dispatch_info();
    let payload = signing_payload(&(0u8, &call)).unwrap();
    let signature = key
        .sign_deterministic(&payload, TX_SIGNING_CONTEXT)
        .unwrap();
    let ext = PqAuthorize::new(PqAuth::Signed {
        who,
        signature,
        public_key: first_transaction.then(|| public(&key)),
    });
    (ext, call, info)
}

#[benchmarks(where
    T: Config + Send + Sync,
    <T as frame_system::Config>::RuntimeCall:
        Dispatchable<Info = DispatchInfo> + GetDispatchInfo + From<frame_system::Call<T>>,
    <<T as frame_system::Config>::RuntimeCall as Dispatchable>::RuntimeOrigin:
        AsTransactionAuthorizedOrigin + From<RawOrigin<sp_runtime::AccountId32>>,
)]
mod benchmarks {
    use super::{
        AsTransactionAuthorizedOrigin, BenchmarkError, Call, Config, DispatchInfo,
        DispatchTransaction, Dispatchable, GetDispatchInfo, KEY_ROTATION_CONTEXT, Keys, Pallet,
        RawOrigin, SigAlg, TransactionSource, Zero, authorize_setup, derived_account,
        impl_test_function, public, rotation_statement, signing_key,
    };

    #[benchmark]
    fn rotate_key() -> Result<(), BenchmarkError> {
        let current = signing_key(SigAlg::MlDsa44, 2);
        let who = derived_account(&public(&current));
        Pallet::<T>::register(&who, public(&current));
        // Worst case: the largest implemented algorithm for the proof of possession.
        let new = signing_key(SigAlg::MlDsa87, 3);
        let genesis = frame_system::Pallet::<T>::block_hash(
            frame_system::pallet_prelude::BlockNumberFor::<T>::zero(),
        );
        let statement = rotation_statement(&genesis, &who, 0, &public(&new));
        let proof = new
            .sign_deterministic(&statement, KEY_ROTATION_CONTEXT)
            .unwrap();

        #[extrinsic_call]
        _(RawOrigin::Signed(who.clone()), public(&new), proof);

        assert_eq!(Keys::<T>::get(&who).map(|r| r.rotations), Some(1));
        Ok(())
    }

    #[benchmark]
    fn authorize_ml_dsa_44() -> Result<(), BenchmarkError> {
        let (ext, call, info) = authorize_setup::<T>(SigAlg::MlDsa44, false);
        #[block]
        {
            ext.validate_only(
                RawOrigin::None.into(),
                &call,
                &info,
                0,
                TransactionSource::External,
                0,
            )
            .map_err(|_| BenchmarkError::Stop("authorization failed"))?;
        }
        Ok(())
    }

    #[benchmark]
    fn authorize_ml_dsa_65() -> Result<(), BenchmarkError> {
        let (ext, call, info) = authorize_setup::<T>(SigAlg::MlDsa65, false);
        #[block]
        {
            ext.validate_only(
                RawOrigin::None.into(),
                &call,
                &info,
                0,
                TransactionSource::External,
                0,
            )
            .map_err(|_| BenchmarkError::Stop("authorization failed"))?;
        }
        Ok(())
    }

    #[benchmark]
    fn authorize_ml_dsa_87() -> Result<(), BenchmarkError> {
        let (ext, call, info) = authorize_setup::<T>(SigAlg::MlDsa87, false);
        #[block]
        {
            ext.validate_only(
                RawOrigin::None.into(),
                &call,
                &info,
                0,
                TransactionSource::External,
                0,
            )
            .map_err(|_| BenchmarkError::Stop("authorization failed"))?;
        }
        Ok(())
    }

    #[benchmark]
    fn register_key() -> Result<(), BenchmarkError> {
        let key = signing_key(SigAlg::MlDsa87, 4);
        let who = derived_account(&public(&key));
        #[block]
        {
            Pallet::<T>::register(&who, public(&key));
        }
        assert!(Keys::<T>::contains_key(&who));
        Ok(())
    }

    impl_benchmark_test_suite!(Pallet, crate::mock::new_test_ext(), crate::mock::Test);
}
