//! Benchmarks: every call at its worst case (the most models, a full unbonding list).
#![allow(clippy::unwrap_used, clippy::expect_used)] // Benchmark setup.

use frame_benchmarking::v2::benchmarks;

use crate::{Config, Pallet};

#[benchmarks]
mod benchmarks {
    use super::{Config, Pallet};
    use crate::{BenchmarkHelper, Call, ModelList, Params, Providers, Registration};
    use ac_crypto::{KemAlg, KemPublicKey};
    use ac_primitives::market::records::{
        MAX_ENDPOINT_LEN, MAX_PROVIDER_MODELS, MAX_UNLOCKING, ModelPrice, Tier,
    };
    use ac_primitives::market::{MicroUsd, ModelId, PricePerMTok};
    use alloc::vec;
    use frame_benchmarking::v2::whitelisted_caller;
    // The macro expands to code naming `impl_test_function` unqualified.
    use frame_benchmarking::impl_test_function;
    use frame_support::BoundedVec;
    use frame_support::traits::fungible::Mutate;
    use frame_system::RawOrigin;
    use sp_runtime::{SaturatedConversion, Saturating};

    /// One smallest unit per dollar: the thresholds are tiny, every stake fits.
    const RATE: u128 = 1;
    // 1,000 ATC: far above the runtime's existential deposit (0.001 ATC).
    const STAKE: u128 = 1_000_000_000_000_000_000_000;

    fn models<T: Config>(n: u32, salt: u8) -> ModelList {
        let list = (0..n)
            .map(|i| {
                let mut id = [salt; 32];
                id[0] = 1;
                id[1..5].copy_from_slice(&i.to_le_bytes());
                let model = ModelId(id);
                T::BenchmarkHelper::register_model(model);
                ModelPrice {
                    model,
                    price: PricePerMTok {
                        input: MicroUsd(1),
                        output: MicroUsd(2),
                    },
                }
            })
            .collect::<vec::Vec<_>>();
        BoundedVec::truncate_from(list)
    }

    fn funded<T: Config>() -> T::AccountId {
        T::BenchmarkHelper::set_rate(RATE);
        let who: T::AccountId = whitelisted_caller();
        T::Currency::set_balance(&who, STAKE.saturating_mul(10));
        who
    }

    fn endpoint() -> ac_primitives::market::records::Endpoint {
        BoundedVec::truncate_from(vec![b'e'; MAX_ENDPOINT_LEN as usize])
    }

    fn kem() -> KemPublicKey {
        KemPublicKey::new(KemAlg::XWing, &[7; 1216]).unwrap()
    }

    fn registration<T: Config>(m: u32) -> Registration {
        Registration {
            tier: Tier::T1,
            endpoint: endpoint(),
            kem_pk: kem(),
            models: models::<T>(m, 1),
            stake: STAKE,
            attestation: None,
        }
    }

    fn registered<T: Config>(m: u32) -> T::AccountId {
        let who = funded::<T>();
        Pallet::<T>::register(RawOrigin::Signed(who.clone()).into(), registration::<T>(m)).unwrap();
        who
    }

    #[benchmark]
    fn register(m: Linear<1, MAX_PROVIDER_MODELS>) {
        let who = funded::<T>();
        let reg = registration::<T>(m);
        #[extrinsic_call]
        _(RawOrigin::Signed(who.clone()), reg);
        assert!(Providers::<T>::contains_key(&who));
    }

    #[benchmark]
    fn update(m: Linear<1, MAX_PROVIDER_MODELS>) {
        let who = registered::<T>(MAX_PROVIDER_MODELS);
        let list = models::<T>(m, 2);
        #[extrinsic_call]
        _(
            RawOrigin::Signed(who.clone()),
            Some(endpoint()),
            Some(kem()),
            Some(list),
        );
        assert_eq!(Providers::<T>::get(&who).unwrap().models.len(), m as usize);
    }

    #[benchmark]
    fn heartbeat() {
        let who = registered::<T>(1);
        #[extrinsic_call]
        _(RawOrigin::Signed(who));
    }

    #[benchmark]
    fn bond_extra() {
        let who = registered::<T>(1);
        #[extrinsic_call]
        _(RawOrigin::Signed(who.clone()), STAKE);
        assert_eq!(Providers::<T>::get(&who).unwrap().stake, STAKE * 2);
    }

    /// A full unbonding list: the new chunk merges into the last one.
    #[benchmark]
    fn unbond() {
        let who = registered::<T>(1);
        for _ in 0..MAX_UNLOCKING {
            Pallet::<T>::unbond(RawOrigin::Signed(who.clone()).into(), 1).unwrap();
        }
        #[extrinsic_call]
        _(RawOrigin::Signed(who.clone()), 1);
    }

    #[benchmark]
    fn exit(m: Linear<1, MAX_PROVIDER_MODELS>) {
        let who = registered::<T>(m);
        #[extrinsic_call]
        _(RawOrigin::Signed(who.clone()));
    }

    /// Every chunk is due and the provider is removed.
    #[benchmark]
    fn withdraw_unbonded() {
        let who = registered::<T>(1);
        for _ in 0..MAX_UNLOCKING.saturating_sub(1) {
            Pallet::<T>::unbond(RawOrigin::Signed(who.clone()).into(), 1).unwrap();
        }
        Pallet::<T>::exit(RawOrigin::Signed(who.clone()).into()).unwrap();
        let later = frame_system::Pallet::<T>::block_number()
            .saturating_add(Params::<T>::get().unbond_blocks.saturated_into());
        frame_system::Pallet::<T>::set_block_number(later);
        #[extrinsic_call]
        _(RawOrigin::Signed(who.clone()));
        assert!(!Providers::<T>::contains_key(&who));
    }

    impl_benchmark_test_suite!(Pallet, crate::mock::ext(), crate::mock::Test);
}
