//! Mock runtime for unit tests: balances, Aura-PQ, the validator set and offences.

use frame_support::{derive_impl, traits::ConstU32, traits::ConstU64};

use ac_primitives::offences::OffenceKind;

use crate as pallet_ac_offences;

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
    pub type AuraPq = pallet_aura_pq::Pallet<Test>;

    #[runtime::pallet_index(3)]
    pub type ValidatorSet = pallet_validator_set::Pallet<Test>;

    #[runtime::pallet_index(4)]
    pub type Offences = pallet_ac_offences::Pallet<Test>;
}

#[derive_impl(frame_system::config_preludes::TestDefaultConfig)]
impl frame_system::Config for Test {
    type Block = Block;
    type AccountData = pallet_balances::AccountData<u64>;
}

#[derive_impl(pallet_balances::config_preludes::TestDefaultConfig)]
impl pallet_balances::Config for Test {
    type AccountStore = System;
}

impl pallet_aura_pq::Config for Test {
    type SlotDuration = ConstU64<1000>;
    type MaxAuthorities = ConstU32<10>;
}

impl pallet_validator_set::Config for Test {
    type RuntimeEvent = RuntimeEvent;
    type BlockAuthorities = AuraPq;
    type MaxAuthorities = ConstU32<10>;
    type HistoryEpochs = ConstU32<2>;
    type WeightInfo = ();
}

impl pallet_ac_offences::Config for Test {
    type RuntimeEvent = RuntimeEvent;
    type ValidatorSet = ValidatorSet;
    type Slots = AuraPq;
    type SlashHandler = RecordingSlash;
    type MaxAuthorities = ConstU32<10>;
    type MaxEvidenceAge = ConstU64<24>;
    type WeightInfo = ();
}

/// Genesis with the four development authorities, for the benchmark test suite.
#[cfg(feature = "runtime-benchmarks")]
pub fn new_bench_ext() -> sp_io::TestExternalities {
    use ac_crypto::{SigAlg, dev_seed, sig::SigningKey};
    use sp_runtime::BuildStorage;
    let authorities = ["alice", "bob", "charlie", "dave"]
        .iter()
        .map(|n| {
            SigningKey::from_seed(SigAlg::MlDsa65, &dev_seed(n).unwrap())
                .unwrap()
                .public_key()
                .unwrap()
        })
        .collect();
    RuntimeGenesisConfig {
        aura_pq: pallet_aura_pq::GenesisConfig {
            authorities,
            ..Default::default()
        },
        validator_set: pallet_validator_set::GenesisConfig {
            epoch_length: 8,
            ..Default::default()
        },
        ..Default::default()
    }
    .build_storage()
    .unwrap()
    .into()
}

std::thread_local! {
    /// Calls of the slash handler: offence kind and kinds recorded before it.
    pub static SLASH_CALLS: core::cell::RefCell<Vec<(OffenceKind, Vec<OffenceKind>)>> =
        const { core::cell::RefCell::new(Vec::new()) };
}

/// Slash handler that records its calls and slashes nothing, like `()`.
pub struct RecordingSlash;

impl ac_primitives::validator_set::SlashHandler for RecordingSlash {
    fn on_offence(
        _offender: &ac_crypto::PqPublicKey,
        kind: OffenceKind,
        prior: &[OffenceKind],
    ) -> u128 {
        SLASH_CALLS.with(|calls| calls.borrow_mut().push((kind, prior.to_vec())));
        0
    }
}
