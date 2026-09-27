//! Holder-treasury lock (spec economics/treasury "持币人国库锁定"; decision D42).
//!
//! The holder treasury receives funds but has no way out until on-chain holder voting (M8):
//! - its account is derived from a `PalletId`, so nobody holds its key;
//! - `pallet-treasury-dual` has no call that spends from it;
//! - [`HolderTreasuryLock`] refuses every call that could move or rewrite its balance by force.
//!   It is both the base call filter (for signed and council origins) and the filter of
//!   `PoaAdmin::dispatch_as_root`, because Root bypasses the base call filter.
//!
//! A runtime upgrade (`System::set_code`) can of course change any rule, including this one;
//! upgrades are public motions of the administration and the new runtime stays bound by the
//! node invariants.

use frame_support::traits::Contains;

use crate::{AccountId, Runtime, RuntimeCall, TreasuryDual};

/// Refuses calls that would move funds out of, or overwrite, the holder treasury.
pub struct HolderTreasuryLock;

impl HolderTreasuryLock {
    fn holder() -> AccountId {
        TreasuryDual::holder_account()
    }

    /// Storage key of the holder treasury's `System::Account` entry.
    fn account_key() -> alloc::vec::Vec<u8> {
        frame_system::Account::<Runtime>::hashed_key_for(Self::holder())
    }
}

impl Contains<RuntimeCall> for HolderTreasuryLock {
    fn contains(call: &RuntimeCall) -> bool {
        match call {
            RuntimeCall::Balances(
                pallet_balances::Call::force_transfer { source: who, .. }
                | pallet_balances::Call::force_set_balance { who, .. }
                | pallet_balances::Call::force_unreserve { who, .. },
            ) => *who != Self::holder(),
            RuntimeCall::System(frame_system::Call::set_storage { items }) => {
                let key = Self::account_key();
                items.iter().all(|(k, _)| *k != key)
            }
            RuntimeCall::System(frame_system::Call::kill_storage { keys }) => {
                let key = Self::account_key();
                keys.iter().all(|k| *k != key)
            }
            RuntimeCall::System(frame_system::Call::kill_prefix { prefix, .. }) => {
                !Self::account_key().starts_with(prefix)
            }
            RuntimeCall::PoaAdmin(pallet_poa_admin::Call::dispatch_as_root { call }) => {
                Self::contains(call)
            }
            _ => true,
        }
    }
}
