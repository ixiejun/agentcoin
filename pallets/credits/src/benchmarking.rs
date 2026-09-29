//! Benchmarks: every call, and one voucher redemption for settlement to charge.
#![allow(clippy::unwrap_used, clippy::expect_used)] // Benchmark setup.

use frame_benchmarking::v2::benchmarks;

use crate::{Config, Pallet};

#[benchmarks]
mod benchmarks {
    use super::{Config, Pallet};
    use crate::{BenchmarkHelper, Call, Channels, Params};
    use ac_crypto::SigAlg;
    use ac_crypto::sig::SigningKey;
    use ac_primitives::market::traits::Credit;
    use ac_primitives::market::voucher::VOUCHER_CONTEXT;
    use ac_primitives::market::{MicroUsd, SignedVoucher, VoucherBody};
    use frame_benchmarking::v2::{account, whitelisted_caller};
    // The macro expands to code naming `impl_test_function` unqualified.
    use frame_benchmarking::impl_test_function;
    use frame_support::traits::fungible::Mutate;
    use frame_system::RawOrigin;
    use sp_runtime::{AccountId32, SaturatedConversion, Saturating};

    const ATC: u128 = 1_000_000_000_000_000_000;

    /// ML-DSA-87: the largest key and signature, the slowest verification.
    fn signer() -> SigningKey {
        SigningKey::from_seed(SigAlg::MlDsa87, &ac_crypto::dev_seed("bench-user").unwrap()).unwrap()
    }

    fn setup<T: Config>() -> (AccountId32, AccountId32) {
        T::BenchmarkHelper::set_rate(ATC);
        let user: AccountId32 = whitelisted_caller();
        let gateway: AccountId32 = account("gateway", 0, 0);
        T::BenchmarkHelper::activate_gateway(&gateway);
        T::BenchmarkHelper::register_key(&user, &signer().public_key().unwrap());
        T::Currency::set_balance(&user, 1_000 * ATC);
        (user, gateway)
    }

    fn opened<T: Config>() -> (AccountId32, AccountId32) {
        let (user, gateway) = setup::<T>();
        Pallet::<T>::deposit(
            RawOrigin::Signed(user.clone()).into(),
            gateway.clone(),
            100 * ATC,
        )
        .unwrap();
        (user, gateway)
    }

    fn later<T: Config>() {
        let at = frame_system::Pallet::<T>::block_number()
            .saturating_add(Params::<T>::get().withdrawal_delay.saturated_into());
        frame_system::Pallet::<T>::set_block_number(at);
    }

    #[benchmark]
    fn deposit() {
        let (user, gateway) = opened::<T>();
        #[extrinsic_call]
        _(RawOrigin::Signed(user.clone()), gateway.clone(), ATC);
        assert_eq!(
            Channels::<T>::get(&user, &gateway).unwrap().escrow,
            101 * ATC
        );
    }

    #[benchmark]
    fn request_withdrawal() {
        let (user, gateway) = opened::<T>();
        Pallet::<T>::request_withdrawal(
            RawOrigin::Signed(user.clone()).into(),
            gateway.clone(),
            ATC,
        )
        .unwrap();
        #[extrinsic_call]
        _(RawOrigin::Signed(user), gateway, ATC);
    }

    /// The whole escrow is withdrawn and the channel resets.
    #[benchmark]
    fn withdraw() {
        let (user, gateway) = opened::<T>();
        Pallet::<T>::request_withdrawal(
            RawOrigin::Signed(user.clone()).into(),
            gateway.clone(),
            100 * ATC,
        )
        .unwrap();
        later::<T>();
        #[extrinsic_call]
        _(RawOrigin::Signed(user.clone()), gateway.clone());
        assert_eq!(Channels::<T>::get(&user, &gateway).unwrap().number, 1);
    }

    #[benchmark]
    fn request_key_change() {
        let (user, gateway) = opened::<T>();
        #[extrinsic_call]
        _(RawOrigin::Signed(user), gateway);
    }

    /// One redemption: a due key change is applied, an ML-DSA-87 signature verified, the escrow
    /// moved on hold to the payee.
    #[benchmark]
    fn redeem() {
        let (user, gateway) = opened::<T>();
        Pallet::<T>::request_key_change(RawOrigin::Signed(user.clone()).into(), gateway.clone())
            .unwrap();
        later::<T>();
        let payee: AccountId32 = account("payee", 0, 0);
        T::Currency::set_balance(&payee, ATC);
        let body = VoucherBody {
            genesis: Pallet::<T>::genesis(),
            user: user.clone(),
            gateway: gateway.clone(),
            channel: 0,
            cumulative: MicroUsd(1_000_000),
        };
        let key = signer();
        let voucher = SignedVoucher {
            signature: key
                .sign_deterministic(&body.payload().unwrap(), VOUCHER_CONTEXT)
                .unwrap(),
            public_key: key.public_key().unwrap(),
            body,
        };
        #[block]
        {
            <Pallet<T> as Credit<AccountId32, u128>>::redeem(&gateway, &voucher, &payee).unwrap();
        }
        assert_eq!(
            Channels::<T>::get(&user, &gateway).unwrap().escrow,
            99 * ATC
        );
    }

    impl_benchmark_test_suite!(Pallet, crate::mock::ext(), crate::mock::Test);
}
