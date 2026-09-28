//! Test runtime: `pallet-revive` (EVM bytecode, AccountId32 addresses, auto mapping) on top of
//! `Balances` through [`ReviveCurrency`], with a burn handler that counts burned ATC.
#![allow(clippy::arithmetic_side_effects)]

use crate::{self as pallet_evm_support, CurrentPayer, ReviveCurrency};
use frame_support::{
    derive_impl, parameter_types,
    traits::{
        ConstBool, ConstU32, ConstU64, ConstU128, Imbalance as _, OnUnbalanced, fungible::Credit,
    },
};
use pallet_revive::AccountId32Mapper;
use sp_runtime::{AccountId32, BuildStorage, Perbill, traits::IdentityLookup};

pub type AccountId = AccountId32;
pub type Balance = u128;
type Block = frame_system::mocking::MockBlock<Test>;

/// 1 ATC in the smallest unit.
pub const ATC: Balance = 1_000_000_000_000_000_000;
/// Existential deposit, as in the runtime (0.001 ATC).
pub const ED: Balance = ATC / 1_000;

frame_support::construct_runtime!(
    pub enum Test {
        System: frame_system,
        Balances: pallet_balances,
        Timestamp: pallet_timestamp,
        Revive: pallet_revive,
        EvmSupport: pallet_evm_support,
    }
);

#[derive_impl(frame_system::config_preludes::TestDefaultConfig)]
impl frame_system::Config for Test {
    type Block = Block;
    type AccountId = AccountId;
    type Lookup = IdentityLookup<AccountId>;
    type AccountData = pallet_balances::AccountData<Balance>;
    type OnNewAccount = pallet_revive::AutoMapper<Test>;
    type OnKilledAccount = pallet_revive::AutoMapper<Test>;
    type DbWeight = frame_support::weights::constants::RocksDbWeight;
}

#[derive_impl(pallet_balances::config_preludes::TestDefaultConfig)]
impl pallet_balances::Config for Test {
    type Balance = Balance;
    type ExistentialDeposit = ConstU128<ED>;
    type AccountStore = System;
    type RuntimeHoldReason = RuntimeHoldReason;
    type RuntimeFreezeReason = RuntimeFreezeReason;
    type MaxFreezes = frame_support::traits::VariantCountOf<RuntimeFreezeReason>;
}

#[derive_impl(pallet_timestamp::config_preludes::TestDefaultConfig)]
impl pallet_timestamp::Config for Test {}

/// Storage key of the burn counter; storage (not a thread-local) so that it is rolled back with
/// everything else when revive reverts a frame, like `Emission::TotalBurned`.
const BURNED_KEY: &[u8] = b":test:burned";

/// ATC burned through [`CountBurns`] (the test stand-in for `Emission::TotalBurned`).
pub fn burned() -> Balance {
    frame_support::storage::unhashed::get_or_default(BURNED_KEY)
}

/// Burns the credit and counts it, like `Emission`'s `OnUnbalanced`.
pub struct CountBurns;

impl OnUnbalanced<Credit<AccountId, Balances>> for CountBurns {
    fn on_nonzero_unbalanced(amount: Credit<AccountId, Balances>) {
        frame_support::storage::unhashed::put(BURNED_KEY, &burned().saturating_add(amount.peek()));
        drop(amount);
    }
}

parameter_types! {
    pub const DepositPerByte: Balance = ATC / 10_000;
    pub const DepositPerItem: Balance = ATC / 100;
    pub const CodeHashLockupDepositPercent: Perbill = Perbill::from_percent(30);
}

/// The currency revive sees.
pub type TestReviveCurrency = ReviveCurrency<Balances, CurrentPayer<Test>, CountBurns>;

#[derive_impl(pallet_revive::config_preludes::TestDefaultConfig)]
impl pallet_revive::Config for Test {
    type Time = Timestamp;
    type AddressMapper = AccountId32Mapper<Self>;
    type Balance = Balance;
    type Currency = TestReviveCurrency;
    type OnBurn = CountBurns;
    type DepositPerByte = DepositPerByte;
    type DepositPerItem = DepositPerItem;
    type DepositPerChildTrieItem = DepositPerItem;
    type CodeHashLockupDepositPercent = CodeHashLockupDepositPercent;
    type AllowEVMBytecode = ConstBool<true>;
    type NativeToEthRatio = ConstU32<1>;
    type ChainId = ConstU64<{ ac_primitives::evm::EVM_CHAIN_ID }>;
    type AutoMap = ConstBool<true>;
    type UploadOrigin = frame_system::EnsureSigned<AccountId>;
    type InstantiateOrigin = frame_system::EnsureSigned<AccountId>;
}

impl pallet_evm_support::Config for Test {}

pub fn alice() -> AccountId {
    AccountId32::new([1; 32])
}

pub fn bob() -> AccountId {
    AccountId32::new([2; 32])
}

/// Genesis: Alice and Bob hold 1,000 ATC each; nothing is minted by revive.
pub fn new_test_ext() -> sp_io::TestExternalities {
    let storage = RuntimeGenesisConfig {
        balances: pallet_balances::GenesisConfig::<Test> {
            balances: vec![(alice(), 1_000 * ATC), (bob(), 1_000 * ATC)],
            ..Default::default()
        },
        ..Default::default()
    }
    .build_storage()
    .unwrap_or_default();
    let mut ext = sp_io::TestExternalities::new(storage);
    ext.execute_with(|| System::set_block_number(1));
    ext
}

/// Everything ever minted: issuance plus burned (constant when nothing is minted).
pub fn gross_supply() -> Balance {
    pallet_balances::TotalIssuance::<Test>::get().saturating_add(burned())
}
