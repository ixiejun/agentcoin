//! EVM call filter (m4-evm design D3; specs evm/contracts and chain/pq-transaction-auth).
//!
//! Only two ways into `pallet-revive` stay open, both authorized by the account's ML-DSA
//! signature: `Revive::call` and `Revive::instantiate_with_code` with EVM init code, plus
//! `dispatch_as_fallback_account` wrapping one of them (to recover funds sent to an EVM address
//! before its account existed). Everything else is refused, including every Revive call added by
//! a future upstream version (fail closed):
//! - the Ethereum transaction entry points (`eth_transact` and the `eth_*` calls it produces),
//!   which would authorize with secp256k1;
//! - PolkaVM code: `instantiate_with_code` with a PolkaVM program, `upload_code`, `remove_code`,
//!   `set_code` and `instantiate` from a code hash (only PolkaVM code is stored by hash);
//! - address-mapping changes: mappings are created with the account and never removed.

use frame_support::traits::Contains;

use crate::RuntimeCall;
use ac_primitives::evm::POLKAVM_MAGIC;

/// Refuses every `pallet-revive` call except EVM contract calls and deployments.
pub struct EvmOnly;

impl Contains<RuntimeCall> for EvmOnly {
    fn contains(call: &RuntimeCall) -> bool {
        match call {
            RuntimeCall::Revive(revive) => match revive {
                pallet_revive::Call::call { .. } => true,
                pallet_revive::Call::instantiate_with_code { code, .. } => {
                    !code.starts_with(&POLKAVM_MAGIC)
                }
                pallet_revive::Call::dispatch_as_fallback_account { call } => {
                    matches!(**call, RuntimeCall::Revive(_)) && Self::contains(call)
                }
                _ => false,
            },
            RuntimeCall::PoaAdmin(pallet_poa_admin::Call::dispatch_as_root { call }) => {
                Self::contains(call)
            }
            _ => true,
        }
    }
}
