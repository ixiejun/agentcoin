//! Benchmarks.
#![allow(clippy::unwrap_used, clippy::expect_used)] // Benchmark setup.

use frame_benchmarking::v2::benchmarks;

use crate::{Config, Pallet};

#[benchmarks]
mod benchmarks {
    use super::{Config, Pallet};
    use crate::{Call, Models, RecordOf};
    use ac_primitives::market::model::{
        Lineage, LineageKind, MAX_ARCH_LEN, MAX_LICENSE_TAG_LEN, MAX_MODEL_NAME_LEN, MAX_SHARDS,
        QuantType,
    };
    use ac_primitives::market::{ModelId, ModelManifest};
    use alloc::vec;
    use frame_benchmarking::v2::whitelisted_caller;
    // The macro expands to code naming `impl_test_function` unqualified.
    use frame_benchmarking::impl_test_function;
    use frame_support::BoundedVec;
    use frame_support::traits::fungible::Mutate;

    /// Worst case for `s` shards: longest name, architecture and licence tag, and a lineage whose
    /// parent must be looked up.
    #[benchmark]
    fn register(s: Linear<1, MAX_SHARDS>) {
        let caller: T::AccountId = whitelisted_caller();
        T::Currency::set_balance(&caller, u128::MAX / 2);
        let parent = ModelId([7; 32]);
        let seed = ModelManifest::new(b"p", b"a", QuantType::Bf16, vec![[0; 32]]).unwrap();
        Models::<T>::insert(
            parent,
            RecordOf::<T> {
                owner: caller.clone(),
                manifest: seed,
                lineage: None,
                license_tag: BoundedVec::default(),
                royalty: None,
                deposit: 0,
                registered_at: 0u32.into(),
            },
        );
        let manifest = ModelManifest::new(
            &vec![b'n'; MAX_MODEL_NAME_LEN as usize],
            &vec![b'a'; MAX_ARCH_LEN as usize],
            QuantType::Int4,
            (0..s).map(|i| [(i % 251) as u8; 32]).collect(),
        )
        .unwrap();
        let id = manifest.id().unwrap();
        let tag = BoundedVec::truncate_from(vec![b't'; MAX_LICENSE_TAG_LEN as usize]);
        let lineage = Some(Lineage {
            parent,
            kind: LineageKind::Quantize,
        });
        #[extrinsic_call]
        _(
            frame_system::RawOrigin::Signed(caller),
            manifest,
            lineage,
            tag,
            None,
        );
        assert!(Models::<T>::contains_key(id));
    }

    impl_benchmark_test_suite!(Pallet, crate::mock::ext(), crate::mock::Test);
}
