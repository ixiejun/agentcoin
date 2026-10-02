//! Mock runtime for unit tests: balances, public jobs, and settable models, rate, randomness and
//! emission epoch.
// Test code: an overflow here fails the test loudly (debug builds panic), which is what we want.
#![allow(
    clippy::arithmetic_side_effects,
    clippy::unwrap_used,
    clippy::indexing_slicing
)]

use core::cell::RefCell;
use std::collections::BTreeSet;

use frame_support::traits::fungible::Credit;
use frame_support::traits::{Hooks, Imbalance, OnUnbalanced};
use frame_support::{derive_impl, parameter_types};
use frame_system::EnsureRoot;
use sp_core::H256;
use sp_runtime::traits::IdentityLookup;
use sp_runtime::{AccountId32, BuildStorage};

use ac_primitives::emission::{EpochIndex, EpochIndexSource};
use ac_primitives::market::public::{
    JobId, JobSpec, PublicParams, RULES_V1, Reveal, Summary, UnitIndex, Url, WorkerModels,
    WorkerReveal, commitment,
};
use ac_primitives::market::traits::{ModelLookup, PriceSource};
use ac_primitives::market::work::JobKind;
use ac_primitives::market::{AtcPerUsd, MicroUsd, ModelId};

use crate as pallet_public_jobs;
use crate::JobsRandomness;

type Block = frame_system::mocking::MockBlock<Test>;

#[frame_support::runtime]
mod runtime {
    #[runtime::runtime]
    #[runtime::derive(
        RuntimeCall,
        RuntimeEvent,
        RuntimeError,
        RuntimeOrigin,
        RuntimeFreezeReason,
        RuntimeHoldReason,
        RuntimeSlashReason,
        RuntimeLockId,
        RuntimeTask,
        RuntimeViewFunction
    )]
    pub struct Test;

    #[runtime::pallet_index(0)]
    pub type System = frame_system::Pallet<Test>;

    #[runtime::pallet_index(1)]
    pub type Balances = pallet_balances::Pallet<Test>;

    #[runtime::pallet_index(2)]
    pub type PublicJobs = pallet_public_jobs::Pallet<Test>;
}

#[derive_impl(frame_system::config_preludes::TestDefaultConfig)]
impl frame_system::Config for Test {
    type Block = Block;
    type AccountId = AccountId32;
    type Lookup = IdentityLookup<AccountId32>;
    type AccountData = pallet_balances::AccountData<u128>;
}

parameter_types! {
    pub const ExistentialDeposit: u128 = 1;
    pub const PruneLimit: u32 = 4;
}

#[derive_impl(pallet_balances::config_preludes::TestDefaultConfig)]
impl pallet_balances::Config for Test {
    type Balance = u128;
    type AccountStore = System;
    type ExistentialDeposit = ExistentialDeposit;
    type RuntimeHoldReason = RuntimeHoldReason;
}

/// One ATC.
pub const ATC: u128 = 1_000_000_000_000_000_000;

std::thread_local! {
    static RATE: RefCell<Option<u128>> = const { RefCell::new(Some(ATC)) };
    static MODELS: RefCell<BTreeSet<ModelId>> = const { RefCell::new(BTreeSet::new()) };
    static RANDOM: RefCell<Option<H256>> = const { RefCell::new(Some(H256([7; 32]))) };
    static EPOCH: RefCell<EpochIndex> = const { RefCell::new(0) };
    static BURNED: RefCell<u128> = const { RefCell::new(0) };
}

/// Rate settable by tests (smallest units per dollar).
pub struct TestPrice;
impl PriceSource for TestPrice {
    fn atc_per_usd() -> Option<AtcPerUsd> {
        RATE.with(|r| r.borrow().map(AtcPerUsd))
    }
}

/// Sets the mock rate.
pub fn set_rate(rate: Option<u128>) {
    RATE.with(|r| *r.borrow_mut() = rate);
}

/// Registered models.
pub struct TestModels;
impl ModelLookup for TestModels {
    fn exists(id: &ModelId) -> bool {
        MODELS.with(|m| m.borrow().contains(id))
    }
}

/// Randomness settable by tests.
pub struct TestRandomness;
impl JobsRandomness for TestRandomness {
    fn random(subject: &[u8]) -> Option<H256> {
        RANDOM.with(|r| {
            r.borrow().map(|seed| {
                let mut d = seed.as_bytes().to_vec();
                d.extend_from_slice(subject);
                H256(ac_crypto::hash::blake3_256(&d))
            })
        })
    }
}

/// Sets (or removes) the randomness.
pub fn set_randomness(seed: Option<H256>) {
    RANDOM.with(|r| *r.borrow_mut() = seed);
}

/// The emission epoch, settable by tests.
pub struct TestEpochs;
impl EpochIndexSource for TestEpochs {
    fn current_epoch() -> EpochIndex {
        EPOCH.with(|e| *e.borrow())
    }
}

/// Sets the current emission epoch.
pub fn set_epoch(epoch: EpochIndex) {
    EPOCH.with(|e| *e.borrow_mut() = epoch);
}

/// Burns slashed rewards and counts them.
pub struct Burn;
impl OnUnbalanced<Credit<AccountId32, Balances>> for Burn {
    fn on_nonzero_unbalanced(amount: Credit<AccountId32, Balances>) {
        BURNED.with(|b| *b.borrow_mut() += amount.peek());
        drop(amount);
    }
}

/// Total burned through [`Burn`].
pub fn burned() -> u128 {
    BURNED.with(|b| *b.borrow())
}

impl pallet_public_jobs::Config for Test {
    type RuntimeEvent = RuntimeEvent;
    type RuntimeHoldReason = RuntimeHoldReason;
    type Currency = Balances;
    type Models = TestModels;
    type Price = TestPrice;
    type Randomness = TestRandomness;
    type Epochs = TestEpochs;
    type Burn = Burn;
    type PublisherOrigin = EnsureRoot<AccountId32>;
    type AdminOrigin = EnsureRoot<AccountId32>;
    type PruneLimit = PruneLimit;
    type WeightInfo = ();
    #[cfg(feature = "runtime-benchmarks")]
    type BenchmarkHelper = ();
}

#[cfg(feature = "runtime-benchmarks")]
impl crate::BenchmarkHelper for () {
    fn set_rate(rate: u128) {
        set_rate(Some(rate));
    }
    fn register_model(model: ModelId) {
        MODELS.with(|m| m.borrow_mut().insert(model));
    }
    fn set_randomness(seed: H256) {
        set_randomness(Some(seed));
    }
    fn set_epoch(epoch: EpochIndex) {
        set_epoch(epoch);
    }
}

/// The model the tests' evaluation jobs use.
pub const MODEL: ModelId = ModelId([0x42; 32]);
/// A second registered model.
pub const MODEL2: ModelId = ModelId([0x43; 32]);
/// Round length of the tests.
pub const ROUND: u64 = 10;

/// Test parameters: rounds of 10 blocks, 4 units per round, commit within 5 blocks and reveal
/// within 3 more, 1 challenge epoch, 30-block lock and suspension, 100-block retention.
pub fn params() -> PublicParams {
    PublicParams {
        round_blocks: 10,
        units_per_round: 4,
        commit_blocks: 5,
        reveal_blocks: 3,
        challenge_epochs: 1,
        lock_blocks: 32,
        suspend_blocks: 30,
        retention_blocks: 100,
        price_cap: MicroUsd(1_000_000),
    }
}

/// An account by number.
pub fn acc(n: u8) -> AccountId32 {
    AccountId32::new([n; 32])
}

/// Models list.
pub fn models(m: &[ModelId]) -> WorkerModels {
    WorkerModels::truncate_from(m.to_vec())
}

/// A job specification of `kind` with `units` units at $0.01, without canaries.
pub fn spec(kind: JobKind, units: u32) -> JobSpec {
    JobSpec {
        kind,
        model: (kind != JobKind::DataClean).then_some(MODEL),
        rules: RULES_V1,
        manifest_hash: [1; 32],
        manifest_url: Url::truncate_from(b"https://data.example/m.json".to_vec()),
        results_url: Url::truncate_from(b"https://results.example".to_vec()),
        units,
        price: MicroUsd(10_000),
        canary_root: None,
    }
}

/// Runs blocks up to and including `n`.
pub fn run_to(n: u64) {
    while System::block_number() < n {
        let next = System::block_number() + 1;
        System::set_block_number(next);
        pallet_public_jobs::Pallet::<Test>::on_initialize(next);
    }
}

/// First block of round `r`.
pub fn round_start(r: u64) -> u64 {
    r * ROUND + 1
}

/// Accounts 1..=`workers` are funded and registered as workers running [`MODEL`]; the chain is
/// at block 1 (round 0).
pub fn new_test_ext(workers: u8) -> sp_io::TestExternalities {
    let mut balances: Vec<(AccountId32, u128)> =
        (1..=60u8).map(|n| (acc(n), 1_000 * ATC)).collect();
    balances.push((crate::Pallet::<Test>::pot(), 1));
    let mut storage = frame_system::GenesisConfig::<Test>::default()
        .build_storage()
        .unwrap();
    pallet_balances::GenesisConfig::<Test> {
        balances,
        ..Default::default()
    }
    .assimilate_storage(&mut storage)
    .unwrap();
    pallet_public_jobs::GenesisConfig::<Test> {
        params: params(),
        _marker: core::marker::PhantomData,
    }
    .assimilate_storage(&mut storage)
    .unwrap();
    let mut ext = sp_io::TestExternalities::new(storage);
    ext.execute_with(|| {
        set_rate(Some(ATC));
        set_randomness(Some(H256([7; 32])));
        set_epoch(0);
        BURNED.with(|b| *b.borrow_mut() = 0);
        MODELS.with(|m| {
            let mut m = m.borrow_mut();
            m.clear();
            m.insert(MODEL);
            m.insert(MODEL2);
        });
        System::set_block_number(1);
        pallet_public_jobs::Pallet::<Test>::on_initialize(1);
        for n in 1..=workers {
            PublicJobs::register(RuntimeOrigin::signed(acc(n)), models(&[MODEL])).unwrap();
        }
    });
    ext
}

/// Workers `ns` declare themselves ready.
pub fn ready(ns: &[u8]) {
    for n in ns {
        PublicJobs::ready(RuntimeOrigin::signed(acc(*n))).unwrap();
    }
}

/// Publishes `spec` as root and returns its identifier.
pub fn publish(spec: JobSpec) -> JobId {
    let id = crate::NextJob::<Test>::get();
    PublicJobs::publish(RuntimeOrigin::root(), Box::new(spec)).unwrap();
    id
}

/// The salt and result hash worker `who` uses in tests.
pub fn salt_of(who: &AccountId32) -> [u8; 32] {
    let mut s: [u8; 32] = *who.as_ref();
    s[0] ^= 0xaa;
    s
}

/// The commitment of `who` to `summary` for a unit's current attempt.
pub fn commitment_of(who: &AccountId32, job: JobId, unit: UnitIndex, summary: &[u8]) -> H256 {
    let attempt = crate::Units::<Test>::get(job, unit).unwrap().attempt;
    commitment(&Reveal {
        job,
        unit,
        attempt,
        worker: who,
        summary,
        result_hash: &[0x99; 32],
        salt: &salt_of(who),
    })
}

/// `who` commits to `summary`.
pub fn commit(who: &AccountId32, job: JobId, unit: UnitIndex, summary: &[u8]) {
    let hash = commitment_of(who, job, unit, summary);
    PublicJobs::commit(RuntimeOrigin::signed(who.clone()), job, unit, hash).unwrap();
}

/// `who` reveals `summary`.
pub fn reveal(
    who: &AccountId32,
    job: JobId,
    unit: UnitIndex,
    summary: &[u8],
) -> sp_runtime::DispatchResultWithInfo<frame_support::dispatch::PostDispatchInfo> {
    PublicJobs::reveal(
        RuntimeOrigin::signed(who.clone()),
        job,
        unit,
        WorkerReveal {
            summary: Summary::truncate_from(summary.to_vec()),
            result_hash: [0x99; 32],
            salt: salt_of(who),
        },
    )
}
