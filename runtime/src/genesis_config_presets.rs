//! Genesis presets.
//!
//! Only the development and local-testnet presets endow accounts; their keys are the public
//! development seeds (`ac_crypto::dev_seed`). Every other preset must have zero issuance
//! (decision D9, no premine); the node enforces this for live chains.

use alloc::{vec, vec::Vec};

use ac_crypto::sig::SigningKey;
use ac_crypto::{PqPublicKey, SigAlg};
use frame_support::build_struct_json_patch;
use serde_json::Value;
use sp_genesis_builder::PresetId;

use crate::{ATC, AccountId, BalancesConfig, RuntimeGenesisConfig};

/// Development account names, in endowment order.
pub const DEV_ACCOUNTS: [&str; 4] = ["alice", "bob", "charlie", "dave"];
/// Endowment of each development account.
pub const DEV_ENDOWMENT: u128 = 1_000_000 * ATC;

/// Public key of a development name: ML-DSA-44 for accounts, ML-DSA-65 for authorities.
///
/// # Errors
///
/// Only if key generation from the fixed seed failed (never in practice).
pub fn dev_public_key(name: &str, alg: SigAlg) -> Result<PqPublicKey, ac_crypto::Error> {
    SigningKey::from_seed(alg, &ac_crypto::dev_seed(name)?)?.public_key()
}

/// Account ID of a development name (its ML-DSA-44 key).
///
/// # Errors
///
/// Only if key generation from the fixed seed failed (never in practice).
pub fn dev_account(name: &str) -> Result<AccountId, ac_crypto::Error> {
    Ok(pallet_pq_accounts::derived_account(&dev_public_key(
        name,
        SigAlg::MlDsa44,
    )?))
}

fn testnet_genesis(authorities: &[&str]) -> Result<Value, ac_crypto::Error> {
    let authorities = authorities
        .iter()
        .map(|name| dev_public_key(name, SigAlg::MlDsa65))
        .collect::<Result<Vec<_>, _>>()?;
    let balances = DEV_ACCOUNTS
        .iter()
        .map(|name| dev_account(name).map(|a| (a, DEV_ENDOWMENT)))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(build_struct_json_patch!(RuntimeGenesisConfig {
        balances: BalancesConfig { balances },
        aura_pq: pallet_aura_pq::GenesisConfig { authorities },
    }))
}

/// Returns the JSON patch for a named preset, or `None` for unknown names.
pub fn get_preset(id: &PresetId) -> Option<Vec<u8>> {
    let patch = match id.as_ref() {
        sp_genesis_builder::DEV_RUNTIME_PRESET => testnet_genesis(&["alice"]),
        sp_genesis_builder::LOCAL_TESTNET_RUNTIME_PRESET => {
            testnet_genesis(&["alice", "bob", "charlie"])
        }
        _ => return None,
    };
    serde_json::to_vec(&patch.ok()?).ok()
}

/// Names of all presets.
pub fn preset_names() -> Vec<PresetId> {
    vec![
        PresetId::from(sp_genesis_builder::DEV_RUNTIME_PRESET),
        PresetId::from(sp_genesis_builder::LOCAL_TESTNET_RUNTIME_PRESET),
    ]
}
