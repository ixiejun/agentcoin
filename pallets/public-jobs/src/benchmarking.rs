//! Benchmarks: every call at its worst case under the bounds, the round start by number of
//! workers and units opened, and one pruning step by number of removed records.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)] // Benchmark setup.

use frame_benchmarking::v2::benchmarks;

use crate::{Config, Pallet};

#[benchmarks]
mod benchmarks {
    use super::{Config, Pallet};
    use crate::{
        ActiveJobs, BenchmarkHelper, Call, Epochs, Jobs, Locked, MAX_CLAIM_EPOCHS,
        MAX_PENDING_EPOCHS, Params, Pending, PendingList, QueueBounds, SettledQueue, Units,
        Workers,
    };
    use ac_primitives::emission::PublicPayout;
    use ac_primitives::market::public::{
        CanaryReveal, CanarySiblings, EpochPublic, JobSpec, LockedList, MAX_ACTIVE_JOBS,
        MAX_SUMMARY, MAX_UNITS_PER_ROUND, MAX_URL, MAX_WORKER_MODELS, MAX_WORKERS, RULES_V1,
        Reveal, Summary, UnitState, Url, WorkerModels, WorkerReveal, canary_leaf, canary_root,
        commitment,
    };
    use ac_primitives::market::work::JobKind;
    use ac_primitives::market::{MicroUsd, ModelId};
    use alloc::boxed::Box;
    use alloc::vec;
    use alloc::vec::Vec;
    use frame_benchmarking::v2::account;
    // The macro expands to code naming `impl_test_function` unqualified.
    use frame_benchmarking::impl_test_function;
    use frame_support::BoundedVec;
    use frame_support::traits::Get;
    use frame_support::traits::fungible::{Mutate, MutateHold};
    use frame_system::RawOrigin;
    use sp_core::H256;
    use sp_runtime::{AccountId32, SaturatedConversion};

    const ATC: u128 = 1_000_000_000_000_000_000;

    fn model(i: u32) -> ModelId {
        let mut m = [0x40; 32];
        m[..4].copy_from_slice(&i.to_le_bytes());
        ModelId(m)
    }

    fn all_models() -> WorkerModels {
        BoundedVec::truncate_from((0..MAX_WORKER_MODELS).map(model).collect())
    }

    fn worker(i: u32) -> AccountId32 {
        account("worker", i, 0)
    }

    /// Rate, randomness, registered models and `n` funded workers running all of them.
    fn world<T: Config>(n: u32) {
        T::BenchmarkHelper::set_rate(ATC);
        T::BenchmarkHelper::set_randomness(H256([7; 32]));
        T::BenchmarkHelper::set_epoch(0);
        for i in 0..MAX_WORKER_MODELS {
            T::BenchmarkHelper::register_model(model(i));
        }
        T::Currency::set_balance(&Pallet::<T>::pot(), 1_000_000 * ATC);
        for i in 0..n {
            let w = worker(i);
            T::Currency::set_balance(&w, 1_000 * ATC);
            Pallet::<T>::register(RawOrigin::Signed(w).into(), all_models()).unwrap();
        }
    }

    fn url() -> Url {
        BoundedVec::truncate_from(vec![b'u'; MAX_URL as usize])
    }

    /// The largest summary: 256 embedding fingerprints.
    fn big_summary(fill: u8) -> Summary {
        BoundedVec::truncate_from(vec![fill; MAX_SUMMARY as usize])
    }

    fn embed_spec(canary_root: Option<[u8; 32]>) -> JobSpec {
        JobSpec {
            kind: JobKind::Embed,
            model: Some(model(MAX_WORKER_MODELS - 1)),
            rules: RULES_V1,
            manifest_hash: [1; 32],
            manifest_url: url(),
            results_url: url(),
            units: 1,
            price: MicroUsd(10_000),
            canary_root,
        }
    }

    /// Publishes a job and opens its first unit for workers 0..3 at the next round's start.
    fn opened_unit<T: Config>(canary_root: Option<[u8; 32]>) -> (u32, [AccountId32; 3]) {
        world::<T>(3);
        let params = Params::<T>::get().unwrap();
        let job = crate::NextJob::<T>::get();
        Pallet::<T>::publish(RawOrigin::Root.into(), Box::new(embed_spec(canary_root))).unwrap();
        for i in 0..3 {
            Pallet::<T>::ready(RawOrigin::Signed(worker(i)).into()).unwrap();
        }
        let r = Pallet::<T>::current_round(&params) + 1;
        let start = ac_primitives::market::audit::round_start(r, params.round_blocks);
        frame_system::Pallet::<T>::set_block_number(start.saturated_into());
        Pallet::<T>::start_round(r, &params);
        let assigned = Units::<T>::get(job, 0).unwrap().assigned;
        (job, assigned)
    }

    fn commit_hash(job: u32, who: &AccountId32, summary: &[u8]) -> H256 {
        commitment(&Reveal {
            job,
            unit: 0,
            attempt: 1,
            worker: who,
            summary,
            result_hash: &[9; 32],
            salt: &[3; 32],
        })
    }

    /// Fills each of `who`'s pending lists to one below the bound, so a credit searches it all.
    fn fill_pending<T: Config>(who: &AccountId32) {
        let list: Vec<(u64, u128)> = (1_000..1_000 + u64::from(MAX_PENDING_EPOCHS) - 1)
            .map(|e| (e, ATC))
            .collect();
        Pending::<T>::insert(who, PendingList::truncate_from(list));
    }

    /// Commits for all three and moves past the commit deadline.
    fn committed<T: Config>(job: u32, workers: &[AccountId32; 3], summary: &[u8]) {
        for w in workers {
            fill_pending::<T>(w);
            Pallet::<T>::commit(
                RawOrigin::Signed(w.clone()).into(),
                job,
                0,
                commit_hash(job, w, summary),
            )
            .unwrap();
        }
        let u = Units::<T>::get(job, 0).unwrap();
        frame_system::Pallet::<T>::set_block_number(u.commit_by + 1u32.into());
    }

    #[benchmark]
    fn register() {
        world::<T>(0);
        let who = worker(0);
        T::Currency::set_balance(&who, 1_000 * ATC);
        #[extrinsic_call]
        _(RawOrigin::Signed(who.clone()), all_models());
        assert!(Workers::<T>::contains_key(who));
    }

    #[benchmark]
    fn set_models() {
        world::<T>(1);
        #[extrinsic_call]
        _(RawOrigin::Signed(worker(0)), all_models());
    }

    #[benchmark]
    fn deregister() {
        world::<T>(1);
        #[extrinsic_call]
        _(RawOrigin::Signed(worker(0)));
        assert!(!Workers::<T>::contains_key(worker(0)));
    }

    #[benchmark]
    fn ready() {
        world::<T>(1);
        #[extrinsic_call]
        _(RawOrigin::Signed(worker(0)));
    }

    /// Publishing next to 15 active jobs, with the longest URLs and a canary root.
    #[benchmark]
    fn publish() {
        world::<T>(0);
        for _ in 0..MAX_ACTIVE_JOBS - 1 {
            Pallet::<T>::publish(RawOrigin::Root.into(), Box::new(embed_spec(None))).unwrap();
        }
        #[extrinsic_call]
        _(RawOrigin::Root, Box::new(embed_spec(Some([5; 32]))));
        assert_eq!(ActiveJobs::<T>::get().len() as u32, MAX_ACTIVE_JOBS);
    }

    /// Cancelling the last of 16 active jobs.
    #[benchmark]
    fn cancel() {
        world::<T>(0);
        for _ in 0..MAX_ACTIVE_JOBS {
            Pallet::<T>::publish(RawOrigin::Root.into(), Box::new(embed_spec(None))).unwrap();
        }
        #[extrinsic_call]
        _(RawOrigin::Root, MAX_ACTIVE_JOBS - 1);
        assert!(Jobs::<T>::get(MAX_ACTIVE_JOBS - 1).unwrap().cancelled);
    }

    #[benchmark]
    fn commit() {
        let (job, w) = opened_unit::<T>(None);
        let hash = commit_hash(job, &w[0], &big_summary(0));
        #[extrinsic_call]
        _(RawOrigin::Signed(w[0].clone()), job, 0, hash);
    }

    /// The third reveal of the largest summaries: it settles the unit and credits three workers
    /// whose pending lists are almost full.
    #[benchmark]
    fn reveal() {
        let (job, w) = opened_unit::<T>(None);
        let s = big_summary(0);
        committed::<T>(job, &w, &s);
        for who in &w[..2] {
            Pallet::<T>::reveal(
                RawOrigin::Signed(who.clone()).into(),
                job,
                0,
                WorkerReveal {
                    summary: s.clone(),
                    result_hash: [9; 32],
                    salt: [3; 32],
                },
            )
            .unwrap();
        }
        #[extrinsic_call]
        _(
            RawOrigin::Signed(w[2].clone()),
            job,
            0,
            WorkerReveal {
                summary: s,
                result_hash: [9; 32],
                salt: [3; 32],
            },
        );
        assert!(matches!(
            Units::<T>::get(job, 0).unwrap().state,
            UnitState::Accepted { .. }
        ));
    }

    /// Closing a unit with two agreeing reveals of the largest summaries.
    #[benchmark]
    fn close() {
        let (job, w) = opened_unit::<T>(None);
        let s = big_summary(0);
        committed::<T>(job, &w, &s);
        for who in &w[..2] {
            Pallet::<T>::reveal(
                RawOrigin::Signed(who.clone()).into(),
                job,
                0,
                WorkerReveal {
                    summary: s.clone(),
                    result_hash: [9; 32],
                    salt: [3; 32],
                },
            )
            .unwrap();
        }
        let u = Units::<T>::get(job, 0).unwrap();
        frame_system::Pallet::<T>::set_block_number(u.reveal_by + 1u32.into());
        #[extrinsic_call]
        _(RawOrigin::Signed(w[2].clone()), job, 0);
    }

    /// A failing canary with the deepest proof: three workers with the largest summaries, the
    /// two-worker majority each holding full locked lists and full pending lists of settled
    /// epochs whose shares are burned from the payout account.
    #[benchmark]
    fn reveal_canary() {
        let leaves_count = 1u32 << 20;
        let expected = big_summary(0xee);
        let salt = [0x5a; 32];
        // The deepest tree is too large to build: hash a path of 20 siblings by hand.
        let job = crate::NextJob::<T>::get();
        let leaf = canary_leaf(job, 0, &expected, &salt);
        let siblings: Vec<[u8; 32]> = (0..20u8).map(|i| [i; 32]).collect();
        let mut root = leaf;
        for s in &siblings {
            root = canary_root(&[root, *s]).unwrap();
        }
        let proof = ac_primitives::market::public::CanaryProof {
            index: 0,
            leaves: leaves_count,
            siblings: CanarySiblings::truncate_from(siblings),
        };
        let (job, w) = opened_unit::<T>(Some(root));
        let s = big_summary(0);
        committed::<T>(job, &w, &s);
        let mut third = s.clone();
        third[0] = 0xee;
        for (i, who) in w.iter().enumerate() {
            let summary = if i == 2 { third.clone() } else { s.clone() };
            if i == 2 {
                // Recommit the third worker to its own summary.
                Units::<T>::mutate(job, 0, |u| {
                    u.as_mut().unwrap().commits[2] = Some(commit_hash(job, who, &summary));
                });
            }
            Pallet::<T>::reveal(
                RawOrigin::Signed(who.clone()).into(),
                job,
                0,
                WorkerReveal {
                    summary,
                    result_hash: [9; 32],
                    salt: [3; 32],
                },
            )
            .unwrap();
        }
        let params = Params::<T>::get().unwrap();
        let now = frame_system::Pallet::<T>::block_number();
        for who in &w {
            let mut locked: Vec<(_, u128)> = Vec::new();
            for i in 0..32u32 {
                locked.push((now + (params.lock_blocks + i).into(), ATC));
            }
            T::Currency::hold(&crate::HoldReason::Locked.into(), who, 32 * ATC).unwrap();
            Locked::<T>::insert(who, LockedList::truncate_from(locked));
            for (e, work) in Pending::<T>::get(who) {
                Epochs::<T>::insert(
                    e,
                    EpochPublic {
                        verified: 3 * work,
                        emission: Some(3 * work),
                    },
                );
            }
        }
        crate::Unclaimed::<T>::put(1_000 * ATC);
        assert!(matches!(
            Units::<T>::get(job, 0).unwrap().state,
            UnitState::Accepted { .. }
        ));
        #[extrinsic_call]
        _(
            RawOrigin::Signed(worker(0)),
            job,
            0,
            Box::new(CanaryReveal {
                summary: expected,
                salt,
                proof,
            }),
        );
        assert_eq!(Units::<T>::get(job, 0).unwrap().state, UnitState::Failed);
    }

    /// Claiming `n` settled epochs.
    #[benchmark]
    fn claim(n: Linear<1, MAX_CLAIM_EPOCHS>) {
        world::<T>(1);
        let who = worker(0);
        let list: Vec<(u64, u128)> = (0..u64::from(n)).map(|e| (e, ATC)).collect();
        Pending::<T>::insert(&who, PendingList::truncate_from(list));
        for e in 0..u64::from(n) {
            Epochs::<T>::insert(
                e,
                EpochPublic {
                    verified: ATC,
                    emission: None,
                },
            );
            <Pallet<T> as PublicPayout<AccountId32>>::settled(e, ATC, ATC);
        }
        let epochs = BoundedVec::truncate_from((0..u64::from(n)).collect::<Vec<_>>());
        #[extrinsic_call]
        _(RawOrigin::Signed(who.clone()), epochs);
        assert!(Pending::<T>::get(&who).is_empty());
    }

    /// Withdrawing a full locked list that is all due.
    #[benchmark]
    fn withdraw() {
        world::<T>(1);
        let who = worker(0);
        let locked: Vec<_> = (0..32u32).map(|i| (i.into(), ATC)).collect();
        T::Currency::hold(&crate::HoldReason::Locked.into(), &who, 32 * ATC).unwrap();
        Locked::<T>::insert(&who, LockedList::truncate_from(locked));
        frame_system::Pallet::<T>::set_block_number(100u32.into());
        #[extrinsic_call]
        _(RawOrigin::Signed(who.clone()));
        assert!(Locked::<T>::get(&who).is_empty());
    }

    #[benchmark]
    fn set_price_cap() {
        world::<T>(0);
        #[extrinsic_call]
        _(RawOrigin::Root, MicroUsd(2_000_000));
    }

    /// A round's first block with `w` registered and ready workers, opening `u` units.
    #[benchmark]
    fn start_round(w: Linear<3, MAX_WORKERS>, u: Linear<0, MAX_UNITS_PER_ROUND>) {
        world::<T>(w);
        let mut params = Params::<T>::get().unwrap();
        params.units_per_round = u.max(1);
        Params::<T>::put(params);
        let mut spec = embed_spec(None);
        spec.units = u.max(1);
        Pallet::<T>::publish(RawOrigin::Root.into(), Box::new(spec)).unwrap();
        for i in 0..w {
            Pallet::<T>::ready(RawOrigin::Signed(worker(i)).into()).unwrap();
        }
        let r = Pallet::<T>::current_round(&params) + 1;
        let start = ac_primitives::market::audit::round_start(r, params.round_blocks);
        frame_system::Pallet::<T>::set_block_number(start.saturated_into());
        #[block]
        {
            Pallet::<T>::start_round(r, &params);
        }
    }

    /// One pruning step removing `n` settled unit records.
    #[benchmark]
    fn prune(n: Linear<1, { T::PruneLimit::get() }>) {
        world::<T>(0);
        let params = Params::<T>::get().unwrap();
        let (job, w) = (0u32, [worker(0), worker(1), worker(2)]);
        for i in 0..n {
            Units::<T>::insert(
                job,
                i,
                ac_primitives::market::public::UnitRecord {
                    attempt: 1,
                    opened_at: 0u32.into(),
                    commit_by: 0u32.into(),
                    reveal_by: 0u32.into(),
                    assigned: w.clone(),
                    tried: BoundedVec::default(),
                    commits: [None, None, None],
                    reveals: [Some((big_summary(1), [0; 32])), None, None],
                    state: UnitState::Failed,
                    settled_at: Some(1u32.into()),
                    canary_revealed: false,
                },
            );
            let at: frame_system::pallet_prelude::BlockNumberFor<T> = 1u32.into();
            SettledQueue::<T>::insert(u64::from(i), (job, i, at));
        }
        QueueBounds::<T>::put((u64::from(n), 0));
        let now: frame_system::pallet_prelude::BlockNumberFor<T> =
            (params.retention_blocks + 2).into();
        #[block]
        {
            Pallet::<T>::prune_step(now, &params);
        }
        assert!(Units::<T>::get(job, 0).is_none());
    }

    frame_benchmarking::impl_benchmark_test_suite!(
        Pallet,
        crate::mock::new_test_ext(0),
        crate::mock::Test
    );
}
