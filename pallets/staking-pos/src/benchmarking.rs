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
use ac_primitives::validator_set::StakingInterface;
use frame_benchmarking::impl_test_function;
use frame_benchmarking::v2::{account, benchmarks};
use frame_support::traits::Get;
use frame_support::traits::fungible::{Inspect, Mutate, MutateHold};
use frame_system::RawOrigin;
use frame_system::pallet_prelude::BlockNumberFor;
use sp_runtime::traits::Zero;

use crate::{
    Balance, Call, Candidates, Config, EpochPoints, HoldReason, KeyOwner, Ledger, MissStreak,
    Nominators, Pallet, Params, PayoutHead, PayoutTail, Payouts, StakingLedger, Targets,
    TotalActive, UnlockChunk,
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

/// Writes `c` qualified candidates and `n` nominators with 16 distinct targets each directly to
/// storage, as the election reads them.
fn electorate<T: Config>(c: u32, n: u32) {
    let key = signing_key(200).public_key().unwrap();
    let (min_self, min_nom) = Pallet::<T>::minimums();
    let mut total = 0u128;
    for i in 0..c {
        let who: T::AccountId = account("candidate", i, 0);
        let active = min_self.saturating_add(Balance::from(i).saturating_mul(7));
        Ledger::<T>::insert(
            &who,
            StakingLedger::<T::MaxUnlocking> {
                active,
                ..Default::default()
            },
        );
        total = total.saturating_add(active);
        Candidates::<T>::insert(
            &who,
            CandidateRecord {
                key: key.clone(),
                commission_bps: 1_000,
                pending_commission: None,
                chilled: false,
            },
        );
    }
    for j in 0..n {
        let who: T::AccountId = account("nominator", j, 0);
        let active = min_nom.saturating_add(Balance::from(j).saturating_mul(13));
        Ledger::<T>::insert(
            &who,
            StakingLedger::<T::MaxUnlocking> {
                active,
                ..Default::default()
            },
        );
        total = total.saturating_add(active);
        let mut targets: Vec<u32> = (0..16u32)
            .map(|k| {
                j.wrapping_mul(31)
                    .wrapping_add(k.wrapping_mul(17))
                    .checked_rem(c.max(1))
                    .unwrap_or(0)
            })
            .collect();
        targets.sort_unstable();
        targets.dedup();
        let targets: Vec<T::AccountId> = targets
            .into_iter()
            .map(|t| account("candidate", t, 0))
            .collect();
        Nominators::<T>::insert(&who, Targets::<T>::truncate_from(targets));
    }
    TotalActive::<T>::put(total);
}

#[benchmarks]
mod benchmarks {
    use super::{
        Balance, Call, CandidateRecord, Candidates, Config, EpochPoints, Get, Inspect, KeyOwner,
        Ledger, MissStreak, Mutate, Nominators, Pallet, Params, PayoutHead, PayoutTail, Payouts,
        RawOrigin, StakingInterface, UnlockChunk, Vec, account, candidates, funded,
        impl_test_function, key_and_proof, nominators, signing_key, stake,
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

    /// One election over `c` candidates and `n` nominators (16 targets each) for `s` seats.
    #[benchmark]
    fn elect(
        c: Linear<1, { T::MaxCandidates::get() }>,
        n: Linear<0, { T::MaxNominators::get() }>,
        s: Linear<1, 200>,
    ) {
        super::electorate::<T>(c, n);
        let elected;

        #[block]
        {
            elected = Pallet::<T>::elect(s, false);
        }

        assert_eq!(elected.len(), usize::try_from(s.min(c)).unwrap());
    }

    /// Reading the switch inputs at a PoA checkpoint: a pass over all ledgers and candidates.
    #[benchmark]
    fn transition_inputs(
        c: Linear<1, { T::MaxCandidates::get() }>,
        n: Linear<0, { T::MaxNominators::get() }>,
    ) {
        super::electorate::<T>(c, n);
        let (active, qualified);

        #[block]
        {
            active = <Pallet<T> as StakingInterface>::total_active();
            qualified = Pallet::<T>::qualified_candidates();
        }

        assert!(active > 0);
        assert_eq!(qualified, c);
    }

    /// Closes an epoch with `p` validators holding points and `m` missed reveals that chill
    /// their validators (third miss in a row).
    #[benchmark]
    fn close_epoch(p: Linear<0, 1_000>, m: Linear<0, 1_000>) {
        let key = signing_key(200).public_key().unwrap();
        let mut missed = Vec::new();
        for i in 0..p.max(m) {
            let who: T::AccountId = account("validator", i, 0);
            let mut id = [0u8; 32];
            id[..4].copy_from_slice(&i.to_le_bytes());
            KeyOwner::<T>::insert(id, &who);
            Candidates::<T>::insert(
                &who,
                CandidateRecord {
                    key: key.clone(),
                    commission_bps: 1_000,
                    pending_commission: None,
                    chilled: false,
                },
            );
            if i < p {
                EpochPoints::<T>::insert(id, 600);
            }
            if i < m {
                MissStreak::<T>::insert(id, 2);
                missed.push(id);
            }
        }

        #[block]
        {
            Pallet::<T>::close_epoch_points(missed);
        }

        assert_eq!(EpochPoints::<T>::iter().count(), 0);
    }

    #[benchmark]
    fn note_author() {
        let key = signing_key(200).public_key().unwrap();

        #[block]
        {
            Pallet::<T>::note_author(&key);
        }

        assert_eq!(EpochPoints::<T>::iter().count(), 1);
    }

    /// Pays `n` queued rewards to new accounts from a funded pot.
    #[benchmark]
    fn pay_rewards(n: Linear<0, { T::PayoutsPerBlock::get() }>) {
        let amount = T::Currency::minimum_balance().saturating_mul(10);
        let pot = Pallet::<T>::reward_pot();
        T::Currency::mint_into(
            &pot,
            amount
                .saturating_mul(Balance::from(n))
                .saturating_add(T::Currency::minimum_balance()),
        )
        .unwrap();
        for i in 0..n {
            let who: T::AccountId = account("staker", i, 0);
            Payouts::<T>::insert(u64::from(i), (who, amount));
        }
        PayoutTail::<T>::put(u64::from(n));

        #[block]
        {
            Pallet::<T>::pay_rewards(n);
        }

        assert_eq!(PayoutHead::<T>::get(), u64::from(n));
    }

    impl_benchmark_test_suite!(Pallet, crate::mock::ext(), crate::mock::Test);
}
