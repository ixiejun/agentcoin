//! Benchmarks of every call, with full lists where a call may scan them (design D1 of
//! `m3-pos`).
//!
//! Existing candidates and nominators are written to storage directly (one shared validator
//! key): generating hundreds of ML-DSA keys would dominate the benchmark without changing what
//! the call does. The account under test uses a real key with a deterministic proof.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)] // Benchmark setup.

use alloc::vec;
use alloc::vec::Vec;

use ac_crypto::sig::{SecretSeed, SigningKey};
use ac_crypto::{PqPublicKey, PqSignature, SigAlg};
use ac_primitives::staking::{CandidateRecord, VALIDATOR_POP_CONTEXT, pop_statement};
// The `benchmarks` macro expands to code naming `Call` and `impl_test_function` unqualified.
use frame_benchmarking::impl_test_function;
use frame_benchmarking::v2::{account, benchmarks};
use frame_support::traits::Get;
use frame_support::traits::fungible::{Inspect, Mutate, MutateHold};
use frame_system::RawOrigin;
use frame_system::pallet_prelude::BlockNumberFor;
use sp_runtime::traits::Zero;

use crate::{
    Balance, Call, Candidates, Config, HoldReason, Ledger, Nominators, Pallet, Params,
    StakingLedger, Targets, TotalActive, UnlockChunk,
};

fn signing_key(seed: u8) -> SigningKey {
    SigningKey::from_seed(SigAlg::MlDsa65, &SecretSeed::new([seed; 32])).unwrap()
}

/// A key and its proof of possession for `who`.
fn key_and_proof<T: Config>(who: &T::AccountId, seed: u8) -> (PqPublicKey, PqSignature) {
    let key = signing_key(seed);
    let public = key.public_key().unwrap();
    let genesis = frame_system::Pallet::<T>::block_hash(BlockNumberFor::<T>::zero());
    let statement = pop_statement(&genesis, who, &public);
    let proof = key
        .sign_deterministic(&statement, VALIDATOR_POP_CONTEXT)
        .unwrap();
    (public, proof)
}

/// A generous stake: ten times the minimum self-stake (plus the existential deposit).
fn stake<T: Config>() -> Balance {
    let (min_self, _) = Pallet::<T>::minimums();
    min_self.saturating_mul(10).max(1_000_000_000_000_000_000)
}

/// A funded account.
fn funded<T: Config>(name: &'static str, index: u32) -> T::AccountId {
    let who: T::AccountId = account(name, index, 0);
    let amount = stake::<T>()
        .saturating_mul(4)
        .saturating_add(T::Currency::minimum_balance());
    T::Currency::mint_into(&who, amount).unwrap();
    who
}

/// Writes `who` as a bonded ledger of `value`, held.
fn bonded<T: Config>(who: &T::AccountId, value: Balance) {
    T::Currency::hold(&HoldReason::Staking.into(), who, value).unwrap();
    let ledger = StakingLedger::<T::MaxUnlocking> {
        active: value,
        ..Default::default()
    };
    Ledger::<T>::insert(who, ledger);
    TotalActive::<T>::mutate(|t| *t = t.saturating_add(value));
}

/// Fills the candidate list with `count` candidates of increasing stake, returns them.
fn candidates<T: Config>(count: u32) -> Vec<T::AccountId> {
    let key = signing_key(200).public_key().unwrap();
    let base = stake::<T>();
    (0..count)
        .map(|i| {
            let who = funded::<T>("candidate", i);
            bonded::<T>(&who, base.saturating_add(Balance::from(i)));
            Candidates::<T>::insert(
                &who,
                CandidateRecord {
                    key: key.clone(),
                    commission_bps: 1_000,
                    pending_commission: None,
                    chilled: false,
                },
            );
            who
        })
        .collect()
}

/// Fills the nominator list with `count` nominators of increasing stake naming `target`.
fn nominators<T: Config>(count: u32, target: &T::AccountId) {
    let (_, min_nom) = Pallet::<T>::minimums();
    for i in 0..count {
        let who = funded::<T>("nominator", i);
        bonded::<T>(&who, min_nom.saturating_add(Balance::from(i)));
        Nominators::<T>::insert(&who, Targets::<T>::truncate_from(vec![target.clone()]));
    }
}

#[benchmarks]
mod benchmarks {
    use super::{
        Balance, Call, Candidates, Config, Get, Inspect, Ledger, Nominators, Pallet, Params,
        RawOrigin, UnlockChunk, Vec, candidates, funded, impl_test_function, key_and_proof,
        nominators, stake,
    };

    /// Registration into a full list of `c` candidates: scans them and evicts the smallest.
    #[benchmark]
    fn register_candidate(c: Linear<1, { T::MaxCandidates::get() }>) {
        let mut params = Params::<T>::get();
        params.max_candidates = c;
        Params::<T>::put(params);
        let _ = candidates::<T>(c);
        let who = funded::<T>("new", 0);
        let (key, proof) = key_and_proof::<T>(&who, 1);
        let value = stake::<T>().saturating_mul(2);

        #[extrinsic_call]
        _(RawOrigin::Signed(who.clone()), key, proof, value, 1_000);

        assert!(Candidates::<T>::contains_key(&who));
        assert_eq!(Candidates::<T>::count(), c);
    }

    #[benchmark]
    fn bond_extra() {
        let who = candidates::<T>(1).remove(0);
        let before = Pallet::<T>::active(&who);
        let value = stake::<T>();

        #[extrinsic_call]
        _(RawOrigin::Signed(who.clone()), value);

        assert_eq!(Pallet::<T>::active(&who), before.saturating_add(value));
    }

    #[benchmark]
    fn set_validator_key() {
        let who = candidates::<T>(1).remove(0);
        let (key, proof) = key_and_proof::<T>(&who, 2);

        #[extrinsic_call]
        _(RawOrigin::Signed(who.clone()), key.clone(), proof);

        assert_eq!(Candidates::<T>::get(&who).map(|c| c.key), Some(key));
    }

    #[benchmark]
    fn retire() {
        let who = candidates::<T>(1).remove(0);

        #[extrinsic_call]
        _(RawOrigin::Signed(who.clone()));

        assert!(!Candidates::<T>::contains_key(&who));
    }

    /// Nomination into a full list of `n` nominators: scans them and evicts the smallest.
    #[benchmark]
    fn nominate(n: Linear<1, { T::MaxNominators::get() }>) {
        let mut params = Params::<T>::get();
        params.max_nominators = n;
        Params::<T>::put(params);
        let targets: Vec<T::AccountId> = candidates::<T>(16);
        nominators::<T>(n, &targets[0]);
        let who = funded::<T>("new", 0);

        #[extrinsic_call]
        _(RawOrigin::Signed(who.clone()), stake::<T>(), targets);

        assert!(Nominators::<T>::contains_key(&who));
        assert_eq!(Nominators::<T>::count(), n);
    }

    #[benchmark]
    fn set_nominations() {
        let targets: Vec<T::AccountId> = candidates::<T>(16);
        nominators::<T>(1, &targets[0]);
        let who: T::AccountId = frame_benchmarking::v2::account("nominator", 0, 0);

        #[extrinsic_call]
        _(RawOrigin::Signed(who.clone()), targets);

        assert_eq!(Nominators::<T>::get(&who).map(|t| t.len()), Some(16));
    }

    #[benchmark]
    fn unnominate() {
        let target = candidates::<T>(1).remove(0);
        nominators::<T>(1, &target);
        let who: T::AccountId = frame_benchmarking::v2::account("nominator", 0, 0);

        #[extrinsic_call]
        _(RawOrigin::Signed(who.clone()));

        assert!(!Nominators::<T>::contains_key(&who));
    }

    /// Partial unbonding of a nomination through the queue.
    #[benchmark]
    fn unbond() {
        let target = candidates::<T>(1).remove(0);
        let who = funded::<T>("big", 0);
        super::bonded::<T>(&who, stake::<T>());
        Nominators::<T>::insert(
            &who,
            crate::Targets::<T>::truncate_from(alloc::vec![target]),
        );

        #[extrinsic_call]
        _(RawOrigin::Signed(who.clone()), 1_000_000);

        assert_eq!(Ledger::<T>::get(&who).map(|l| l.unlocking.len()), Some(1));
    }

    /// Withdrawal of a full set of unlocked chunks.
    #[benchmark]
    fn withdraw_unbonded() {
        let who = funded::<T>("big", 0);
        let chunks = T::MaxUnlocking::get();
        let each: Balance = 1_000_000;
        super::bonded::<T>(&who, each.saturating_mul(Balance::from(chunks)));
        Ledger::<T>::mutate(&who, |ledger| {
            if let Some(ledger) = ledger {
                ledger.active = 0;
                for i in 0..chunks {
                    let _ = ledger.unlocking.try_push(UnlockChunk {
                        value: each,
                        unlock_at: u64::from(i),
                    });
                }
            }
        });
        frame_system::Pallet::<T>::set_block_number(chunks.into());
        let before = T::Currency::balance(&who);

        #[extrinsic_call]
        _(RawOrigin::Signed(who.clone()));

        assert!(!Ledger::<T>::contains_key(&who));
        assert!(T::Currency::balance(&who) > before);
    }

    #[benchmark]
    fn set_commission() {
        let who = candidates::<T>(1).remove(0);

        #[extrinsic_call]
        _(RawOrigin::Signed(who.clone()), 5_000);

        assert!(
            Candidates::<T>::get(&who)
                .and_then(|c| c.pending_commission)
                .is_some()
        );
    }

    #[benchmark]
    fn chill() {
        let who = candidates::<T>(1).remove(0);

        #[extrinsic_call]
        _(RawOrigin::Signed(who.clone()));

        assert!(Candidates::<T>::get(&who).is_some_and(|c| c.chilled));
    }

    #[benchmark]
    fn validate() {
        let who = candidates::<T>(1).remove(0);
        Pallet::<T>::set_chilled(&who);

        #[extrinsic_call]
        _(RawOrigin::Signed(who.clone()));

        assert!(Candidates::<T>::get(&who).is_some_and(|c| !c.chilled));
    }

    impl_benchmark_test_suite!(Pallet, crate::mock::ext(), crate::mock::Test);
}
