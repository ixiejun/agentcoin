//! Mock runtime for unit tests: balances, audits, and settable providers, gateways, keys, rate
//! and randomness.
// Test code: an overflow here fails the test loudly (debug builds panic), which is what we want.
#![allow(
    clippy::arithmetic_side_effects,
    clippy::unwrap_used,
    clippy::indexing_slicing
)]

use core::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};

use frame_support::traits::fungible::Credit;
use frame_support::traits::{Hooks, Imbalance, OnUnbalanced};
use frame_support::{derive_impl, parameter_types};
use frame_system::EnsureRoot;
use sp_core::H256;
use sp_runtime::traits::IdentityLookup;
use sp_runtime::{AccountId32, BuildStorage, DispatchResult, Perbill};

use ac_crypto::SigAlg;
use ac_crypto::sig::SigningKey;
use ac_primitives::market::audit::{RoundIndex, VerdictOutcome};
use ac_primitives::market::receipt::RECEIPT_CONTEXT;
use ac_primitives::market::traits::{
    AccountKeys, GatewayLookup, PriceSource, ProviderAudit, ProviderPenalty,
};
use ac_primitives::market::voucher::key_fingerprint;
use ac_primitives::market::work::JobKind;
use ac_primitives::market::{
    AtcPerUsd, MicroUsd, ModelId, PricePerMTok, ReceiptBody, SignedReceipt, receipt::fee_for,
};

use crate as pallet_audit;
use crate::{AuditGenesis, AuditRandomness, VerdictSubmission};

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
    pub type Audit = pallet_audit::Pallet<Test>;
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
    pub const RetentionRounds: u32 = 3;
    pub const MaxVerdictsPerRound: u32 = 64;
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
    static KEYS: RefCell<BTreeMap<AccountId32, [u8; 32]>> = const { RefCell::new(BTreeMap::new()) };
    static GATEWAYS: RefCell<BTreeSet<AccountId32>> = const { RefCell::new(BTreeSet::new()) };
    static PROVIDERS: RefCell<BTreeMap<AccountId32, PricePerMTok>> = const { RefCell::new(BTreeMap::new()) };
    static PENALTIES: RefCell<Vec<(AccountId32, Perbill)>> = const { RefCell::new(Vec::new()) };
    static JAILED: RefCell<BTreeSet<AccountId32>> = const { RefCell::new(BTreeSet::new()) };
    static RANDOM: RefCell<Option<H256>> = const { RefCell::new(Some(H256([7; 32]))) };
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

/// Account keys.
pub struct TestKeys;
impl AccountKeys<AccountId32> for TestKeys {
    fn key_fingerprint(who: &AccountId32) -> Option<[u8; 32]> {
        KEYS.with(|k| k.borrow().get(who).copied())
    }
}

/// Gateways.
pub struct TestGateways;
impl GatewayLookup<AccountId32> for TestGateways {
    fn is_active(who: &AccountId32) -> bool {
        GATEWAYS.with(|g| g.borrow().contains(who))
    }
    fn fee_bps(_: &AccountId32) -> Option<u16> {
        None
    }
    fn is_registered(who: &AccountId32) -> bool {
        Self::is_active(who)
    }
}

/// Providers: prices, and the penalties applied to them.
pub struct TestProviders;
impl ProviderAudit<AccountId32> for TestProviders {
    fn is_registered(who: &AccountId32) -> bool {
        PROVIDERS.with(|p| p.borrow().contains_key(who))
    }
    fn price(who: &AccountId32, model: &ModelId) -> Option<PricePerMTok> {
        (*model == MODEL)
            .then(|| PROVIDERS.with(|p| p.borrow().get(who).copied()))
            .flatten()
    }
}
impl ProviderPenalty<AccountId32, u128> for TestProviders {
    fn slash(who: &AccountId32, ratio: Perbill) -> u128 {
        PENALTIES.with(|p| p.borrow_mut().push((who.clone(), ratio)));
        0
    }
    fn jail(who: &AccountId32) -> DispatchResult {
        JAILED.with(|j| j.borrow_mut().insert(who.clone()));
        Ok(())
    }
}

/// Provider slashes applied so far.
pub fn penalties() -> Vec<(AccountId32, Perbill)> {
    PENALTIES.with(|p| p.borrow().clone())
}

/// Whether `who` was jailed.
pub fn jailed(who: &AccountId32) -> bool {
    JAILED.with(|j| j.borrow().contains(who))
}

/// Randomness settable by tests.
pub struct TestRandomness;
impl AuditRandomness for TestRandomness {
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

/// Burns slashed auditor stake and counts it.
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

impl pallet_audit::Config for Test {
    type RuntimeEvent = RuntimeEvent;
    type RuntimeHoldReason = RuntimeHoldReason;
    type Currency = Balances;
    type Providers = TestProviders;
    type Gateways = TestGateways;
    type Keys = TestKeys;
    type Price = TestPrice;
    type Randomness = TestRandomness;
    type Slash = Burn;
    type AdminOrigin = EnsureRoot<AccountId32>;
    type RetentionRounds = RetentionRounds;
    type MaxVerdictsPerRound = MaxVerdictsPerRound;
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
    fn set_key(who: &AccountId32, key: &ac_crypto::PqPublicKey) {
        KEYS.with(|k| k.borrow_mut().insert(who.clone(), key_fingerprint(key)));
    }
    fn register_provider(who: &AccountId32, _model: ModelId, price: PricePerMTok) {
        PROVIDERS.with(|p| p.borrow_mut().insert(who.clone(), price));
    }
    fn register_gateway(who: &AccountId32) {
        GATEWAYS.with(|g| g.borrow_mut().insert(who.clone()));
    }
    fn set_randomness(seed: H256) {
        set_randomness(Some(seed));
    }
}

/// The model every mock provider serves.
pub const MODEL: ModelId = ModelId([0x42; 32]);
/// The mock providers' price.
pub const PRICE: PricePerMTok = PricePerMTok {
    input: MicroUsd(100_000),
    output: MicroUsd(300_000),
};
/// Round length of the tests.
pub const ROUND: u64 = 10;

/// Test parameters: rounds of 10 blocks, 2 auditors per provider, 3 reviewers deciding by 2,
/// votes within 5 blocks, 20-block unbonding, $1,000 stake, $0.05 payments, thresholds v2.
pub fn genesis() -> AuditGenesis {
    AuditGenesis {
        round_blocks: 10,
        assign: 2,
        reviewers: 3,
        quorum: 2,
        vote_blocks: 5,
        unbond_blocks: 20,
        ..AuditGenesis::LIVE
    }
}

/// An account by number.
pub fn acc(n: u8) -> AccountId32 {
    AccountId32::new([n; 32])
}

/// The signing key of account `n`.
pub fn key(n: u8) -> SigningKey {
    SigningKey::from_seed(
        SigAlg::MlDsa44,
        &ac_crypto::dev_seed(&format!("audit-{n}")).unwrap(),
    )
    .unwrap()
}

/// Registers account `n`'s key.
pub fn register_key(n: u8) {
    let fp = key_fingerprint(&key(n).public_key().unwrap());
    KEYS.with(|k| k.borrow_mut().insert(acc(n), fp));
}

/// Provider accounts used by the tests.
pub const PROVIDER: u8 = 200;
/// A second provider.
pub const PROVIDER2: u8 = 201;
/// Gateway account used by the tests.
pub const GATEWAY: u8 = 210;

/// A receipt of `provider` through `gateway` with request ID `[id; 32]`, signed by both.
pub fn receipt(provider: u8, gateway: u8, id: u8) -> SignedReceipt {
    let body = ReceiptBody {
        genesis: crate::Pallet::<Test>::genesis(),
        gateway: acc(gateway),
        provider: acc(provider),
        kind: JobKind::Inference,
        model: MODEL,
        request_id: [id; 32],
        in_tokens: 100,
        out_tokens: 50,
        fee: fee_for(&PRICE, 100, 50).unwrap(),
        toploc_commit: [9; 32],
        ttft_ms: 10,
        total_ms: 100,
    };
    let payload = body.payload().unwrap();
    let (p, g) = (key(provider), key(gateway));
    SignedReceipt {
        provider_key: p.public_key().unwrap(),
        provider_sig: p.sign_deterministic(&payload, RECEIPT_CONTEXT).unwrap(),
        gateway_key: g.public_key().unwrap(),
        gateway_sig: g.sign_deterministic(&payload, RECEIPT_CONTEXT).unwrap(),
        body,
    }
}

/// A verdict submission on `provider` in the current round.
pub fn submission(provider: u8, id: u8, outcome: VerdictOutcome) -> Box<VerdictSubmission> {
    Box::new(VerdictSubmission {
        provider: acc(provider),
        round: current_round(),
        outcome,
        thresholds_version: 2,
        evidence: matches!(outcome, VerdictOutcome::Fail(_)).then_some([id; 32]),
        receipt: receipt(provider, GATEWAY, id),
    })
}

/// The current round.
pub fn current_round() -> RoundIndex {
    let p = pallet_audit::Params::<Test>::get().unwrap();
    pallet_audit::Pallet::<Test>::current_round(&p)
}

/// Runs blocks up to and including `n`.
pub fn run_to(n: u64) {
    while System::block_number() < n {
        let next = System::block_number() + 1;
        System::set_block_number(next);
        pallet_audit::Pallet::<Test>::on_initialize(next);
    }
}

/// Accounts 1..=`auditors` are funded and registered as auditors (stake 1,000 ATC); providers
/// and the gateway are set up; the chain is at the first block of round 1 (so round 1's roster
/// holds the auditors).
pub fn new_test_ext(auditors: u8) -> sp_io::TestExternalities {
    let mut balances: Vec<(AccountId32, u128)> =
        (1..=40u8).map(|n| (acc(n), 1_000_000 * ATC)).collect();
    balances.push((crate::Pallet::<Test>::pot(), 1_000 * ATC));
    let mut storage = frame_system::GenesisConfig::<Test>::default()
        .build_storage()
        .unwrap();
    pallet_balances::GenesisConfig::<Test> {
        balances,
        ..Default::default()
    }
    .assimilate_storage(&mut storage)
    .unwrap();
    pallet_audit::GenesisConfig::<Test> {
        params: genesis(),
        _marker: core::marker::PhantomData,
    }
    .assimilate_storage(&mut storage)
    .unwrap();
    let mut ext = sp_io::TestExternalities::new(storage);
    ext.execute_with(|| {
        set_rate(Some(ATC));
        set_randomness(Some(H256([7; 32])));
        PENALTIES.with(|p| p.borrow_mut().clear());
        JAILED.with(|j| j.borrow_mut().clear());
        BURNED.with(|b| *b.borrow_mut() = 0);
        KEYS.with(|k| k.borrow_mut().clear());
        GATEWAYS.with(|g| g.borrow_mut().clear());
        PROVIDERS.with(|p| {
            let mut p = p.borrow_mut();
            p.clear();
            p.insert(acc(PROVIDER), PRICE);
            p.insert(acc(PROVIDER2), PRICE);
        });
        GATEWAYS.with(|g| g.borrow_mut().insert(acc(GATEWAY)));
        for n in [PROVIDER, PROVIDER2, GATEWAY] {
            register_key(n);
        }
        System::set_block_number(1);
        for n in 1..=auditors {
            pallet_audit::Pallet::<Test>::register(RuntimeOrigin::signed(acc(n)), 1_000 * ATC)
                .unwrap();
        }
        run_to(ROUND + 1);
    });
    ext
}
