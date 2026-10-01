//! Benchmarks: every call at its worst case under the guardrails, the round start by number of
//! auditors and one pruning step by number of removed entries.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)] // Benchmark setup.

use frame_benchmarking::v2::benchmarks;

use crate::{Config, Pallet};

#[benchmarks]
mod benchmarks {
    use super::{Config, Pallet};
    use crate::{
        Adjustable, BenchmarkHelper, Call, Disputes, OpenDispute, Params, PruneNext, RoundRequests,
        UsedRequests, VerdictSubmission,
    };
    use ac_crypto::SigAlg;
    use ac_crypto::sig::SigningKey;
    use ac_primitives::market::audit::{
        AuditMetric, FailReason, MAX_AUDITORS, RoundIndex, VerdictOutcome, Vote, round_start,
    };
    use ac_primitives::market::receipt::{RECEIPT_CONTEXT, fee_for};
    use ac_primitives::market::work::JobKind;
    use ac_primitives::market::{MicroUsd, ModelId, PricePerMTok, ReceiptBody, SignedReceipt};
    use alloc::boxed::Box;
    use alloc::vec::Vec;
    use frame_benchmarking::v2::{account, whitelisted_caller};
    // The macro expands to code naming `impl_test_function` unqualified.
    use frame_benchmarking::impl_test_function;
    use frame_support::traits::Get;
    use frame_support::traits::Hooks;
    use frame_support::traits::fungible::Mutate;
    use frame_system::RawOrigin;
    use sp_core::H256;
    use sp_runtime::{AccountId32, SaturatedConversion, Saturating};

    const ATC: u128 = 1_000_000_000_000_000_000;
    const MODEL: ModelId = ModelId([0x42; 32]);
    const PRICE: PricePerMTok = PricePerMTok {
        input: MicroUsd(100_000),
        output: MicroUsd(300_000),
    };
    const FAIL: VerdictOutcome = VerdictOutcome::Fail(FailReason::Threshold {
        chunk: 0,
        metric: AuditMetric::MantissaMean,
    });

    /// ML-DSA-87: the largest keys and signatures, the slowest verification.
    fn signer(name: &str) -> SigningKey {
        SigningKey::from_seed(SigAlg::MlDsa87, &ac_crypto::dev_seed(name).unwrap()).unwrap()
    }

    fn provider() -> AccountId32 {
        account("provider", 0, 0)
    }

    fn gateway() -> AccountId32 {
        account("gateway", 0, 0)
    }

    fn auditor(i: u32) -> AccountId32 {
        account("auditor", i, 0)
    }

    /// Rate, randomness, a provider and a gateway with keys, a funded pot, and `n` auditors.
    fn world<T: Config>(n: u32) {
        T::BenchmarkHelper::set_rate(ATC);
        T::BenchmarkHelper::set_randomness(H256([7; 32]));
        T::BenchmarkHelper::register_provider(&provider(), MODEL, PRICE);
        T::BenchmarkHelper::register_gateway(&gateway());
        T::BenchmarkHelper::set_key(&provider(), &signer("bench-provider").public_key().unwrap());
        T::BenchmarkHelper::set_key(&gateway(), &signer("bench-gateway").public_key().unwrap());
        T::Currency::set_balance(&Pallet::<T>::pot(), 1_000_000 * ATC);
        for i in 0..n {
            let a = auditor(i);
            T::Currency::set_balance(&a, 10_000 * ATC);
            Pallet::<T>::register(RawOrigin::Signed(a).into(), 1_000 * ATC).unwrap();
        }
    }

    /// Moves to the first block of the next round and starts it.
    fn next_round<T: Config>() -> RoundIndex {
        let params = Params::<T>::get().unwrap();
        let r = Pallet::<T>::current_round(&params).saturating_add(1);
        let at: u32 = round_start(r, params.round_blocks);
        frame_system::Pallet::<T>::set_block_number(at.saturated_into());
        Pallet::<T>::on_initialize(at.saturated_into());
        r
    }

    fn receipt<T: Config>(id: u32) -> SignedReceipt {
        let mut request_id = [0xaa; 32];
        request_id[..4].copy_from_slice(&id.to_le_bytes());
        let body = ReceiptBody {
            genesis: Pallet::<T>::genesis(),
            gateway: gateway(),
            provider: provider(),
            kind: JobKind::Inference,
            model: MODEL,
            request_id,
            in_tokens: 4_000,
            out_tokens: 4_000,
            fee: fee_for(&PRICE, 4_000, 4_000).unwrap(),
            toploc_commit: [9; 32],
            ttft_ms: 100,
            total_ms: 9_000,
        };
        let payload = body.payload().unwrap();
        let (p, g) = (signer("bench-provider"), signer("bench-gateway"));
        SignedReceipt {
            provider_key: p.public_key().unwrap(),
            provider_sig: p.sign_deterministic(&payload, RECEIPT_CONTEXT).unwrap(),
            gateway_key: g.public_key().unwrap(),
            gateway_sig: g.sign_deterministic(&payload, RECEIPT_CONTEXT).unwrap(),
            body,
        }
    }

    fn verdict<T: Config>(
        round: RoundIndex,
        id: u32,
        outcome: VerdictOutcome,
    ) -> Box<VerdictSubmission> {
        Box::new(VerdictSubmission {
            provider: provider(),
            round,
            outcome,
            thresholds_version: Adjustable::<T>::get().unwrap().thresholds_version,
            evidence: matches!(outcome, VerdictOutcome::Fail(_)).then_some([1; 32]),
            receipt: receipt::<T>(id),
        })
    }

    /// Every assigned auditor fails the provider: the last verdict opens a dispute. Returns the
    /// round and the dispute.
    fn disputed<T: Config>() -> (RoundIndex, u64) {
        world::<T>(MAX_AUDITORS);
        let r = next_round::<T>();
        for (i, a) in Pallet::<T>::assignment(r, &provider())
            .into_iter()
            .enumerate()
        {
            let id = u32::try_from(i).unwrap();
            Pallet::<T>::submit_verdict(RawOrigin::Signed(a).into(), verdict::<T>(r, id, FAIL))
                .unwrap();
        }
        (r, OpenDispute::<T>::get(provider()).unwrap())
    }

    #[benchmark]
    fn register() {
        world::<T>(0);
        let caller: AccountId32 = whitelisted_caller();
        T::Currency::set_balance(&caller, 10_000 * ATC);
        #[extrinsic_call]
        _(RawOrigin::Signed(caller.clone()), 1_000 * ATC);
        assert!(Pallet::<T>::auditor(&caller).is_some());
    }

    #[benchmark]
    fn bond_extra() {
        world::<T>(1);
        #[extrinsic_call]
        _(RawOrigin::Signed(auditor(0)), 10 * ATC);
    }

    #[benchmark]
    fn unbond() {
        world::<T>(1);
        Pallet::<T>::bond_extra(RawOrigin::Signed(auditor(0)).into(), 10 * ATC).unwrap();
        #[extrinsic_call]
        _(RawOrigin::Signed(auditor(0)), 10 * ATC);
    }

    #[benchmark]
    fn exit() {
        world::<T>(1);
        #[extrinsic_call]
        _(RawOrigin::Signed(auditor(0)));
    }

    #[benchmark]
    fn withdraw_unbonded() {
        world::<T>(1);
        Pallet::<T>::exit(RawOrigin::Signed(auditor(0)).into()).unwrap();
        let params = Params::<T>::get().unwrap();
        let later = frame_system::Pallet::<T>::block_number()
            .saturating_add(params.unbond_blocks.saturated_into());
        frame_system::Pallet::<T>::set_block_number(later);
        #[extrinsic_call]
        _(RawOrigin::Signed(auditor(0)));
        assert!(Pallet::<T>::auditor(&auditor(0)).is_none());
    }

    /// The worst verdict: a failure that opens a dispute (draws the reviewers), with the
    /// largest receipt, a full roster and a payment.
    #[benchmark]
    fn submit_verdict() {
        world::<T>(MAX_AUDITORS);
        let r = next_round::<T>();
        let assigned = Pallet::<T>::assignment(r, &provider());
        let (last, before) = assigned.split_last().unwrap();
        for (i, a) in before.iter().enumerate() {
            let id = u32::try_from(i).unwrap();
            Pallet::<T>::submit_verdict(
                RawOrigin::Signed(a.clone()).into(),
                verdict::<T>(r, id, FAIL),
            )
            .unwrap();
        }
        let v = verdict::<T>(r, 1_000, FAIL);
        #[extrinsic_call]
        _(RawOrigin::Signed(last.clone()), v);
        assert!(OpenDispute::<T>::get(provider()).is_some());
    }

    /// The deciding vote of a rejection: every accuser is slashed and made to exit, the
    /// winning reviewers are paid.
    #[benchmark]
    fn vote() {
        let (_, id) = disputed::<T>();
        let params = Params::<T>::get().unwrap();
        let reviewers: Vec<AccountId32> = Disputes::<T>::get(id)
            .unwrap()
            .reviewers
            .iter()
            .map(|(r, _)| r.clone())
            .collect();
        let quorum = usize::from(params.quorum);
        for r in reviewers.iter().take(quorum.saturating_sub(1)) {
            Pallet::<T>::vote(
                RawOrigin::Signed(r.clone()).into(),
                provider(),
                id,
                Vote::Reject,
            )
            .unwrap();
        }
        let last = reviewers[quorum.saturating_sub(1)].clone();
        #[extrinsic_call]
        _(RawOrigin::Signed(last), provider(), id, Vote::Reject);
        assert!(OpenDispute::<T>::get(provider()).is_none());
    }

    /// Closing an undecided dispute: every reviewer missed its vote.
    #[benchmark]
    fn close_dispute() {
        let (_, id) = disputed::<T>();
        let deadline = Disputes::<T>::get(id).unwrap().deadline;
        frame_system::Pallet::<T>::set_block_number(deadline.saturating_add(1u32.into()));
        let caller: AccountId32 = whitelisted_caller();
        #[extrinsic_call]
        _(RawOrigin::Signed(caller), provider(), id);
        assert!(OpenDispute::<T>::get(provider()).is_none());
    }

    #[benchmark]
    fn set_params() {
        world::<T>(0);
        let mut p = Adjustable::<T>::get().unwrap();
        p.thresholds_version = p.thresholds_version.saturating_add(1);
        #[extrinsic_call]
        _(RawOrigin::Root, p);
    }

    /// A round's first block with `n` registered auditors.
    #[benchmark]
    fn start_round(n: Linear<0, MAX_AUDITORS>) {
        world::<T>(n);
        let params = Params::<T>::get().unwrap();
        let r = Pallet::<T>::current_round(&params).saturating_add(1);
        #[block]
        {
            Pallet::<T>::start_round(r);
        }
    }

    /// One pruning step removing `n` used request IDs of an old round.
    #[benchmark]
    fn prune(n: Linear<1, { T::PruneLimit::get() }>) {
        world::<T>(0);
        for i in 0..n {
            let mut id = [0xbb; 32];
            id[..4].copy_from_slice(&i.to_le_bytes());
            UsedRequests::<T>::insert(id, 0);
            RoundRequests::<T>::insert(0, id, ());
        }
        PruneNext::<T>::put(0);
        let current = T::RetentionRounds::get().saturating_add(1);
        #[block]
        {
            Pallet::<T>::prune_step(current);
        }
        assert_eq!(RoundRequests::<T>::iter_key_prefix(0).count(), 0);
    }

    frame_benchmarking::impl_benchmark_test_suite!(
        Pallet,
        crate::mock::new_test_ext(0),
        crate::mock::Test
    );
}
