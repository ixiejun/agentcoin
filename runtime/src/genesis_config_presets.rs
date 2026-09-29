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
use ac_primitives::staking::TransitionParams;
use pallet_staking_pos::StakingParams;

/// Development account names, in endowment order.
pub const DEV_ACCOUNTS: [&str; 4] = ["alice", "bob", "charlie", "dave"];
/// Endowment of each development account.
pub const DEV_ENDOWMENT: u128 = 1_000_000 * ATC;

/// Motion duration of the development presets, in blocks.
pub const DEV_MOTION_DURATION: u32 = 20;

/// Entropy of the public development wallet: the BIP-39 test mnemonic
/// `abandon abandon … abandon art` (24 words). Development and local chains endow its first
/// ML-DSA-44 key so that `ac-wallet import` works out of the box. Never use it for real funds.
pub const DEV_WALLET_ENTROPY: [u8; 32] = [0u8; 32];

/// Account ID of the public development wallet.
///
/// # Errors
///
/// Only if key derivation failed (never in practice).
pub fn dev_wallet_account() -> Result<AccountId, ac_crypto::Error> {
    let entropy = ac_crypto::WalletEntropy::new(DEV_WALLET_ENTROPY);
    let seed = ac_crypto::wallet_key_seed(&entropy, SigAlg::MlDsa44, 0)?;
    Ok(pallet_pq_accounts::derived_account(
        &SigningKey::from_seed(SigAlg::MlDsa44, &seed)?.public_key()?,
    ))
}

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

/// Staking parameters of the development preset (epochs of 10 blocks): everything unbonds
/// within two epochs so tests see withdrawals quickly (design D10 of `m3-pos`).
pub const DEV_STAKING: StakingParams = StakingParams {
    max_candidates: 50,
    max_nominators: 200,
    self_unbond_blocks: 20,
    nomination_unbond_min: 10,
    nomination_unbond_max: 20,
    commission_delay: 10,
};

/// Staking parameters of the local-testnet preset (epochs of 20 blocks).
pub const LOCAL_STAKING: StakingParams = StakingParams {
    max_candidates: 50,
    max_nominators: 200,
    self_unbond_blocks: 40,
    nomination_unbond_min: 20,
    nomination_unbond_max: 40,
    commission_delay: 20,
};

/// Switch parameters of the development preset: one qualified candidate, the conditions
/// holding from block 20 for 20 blocks, so a single node switches within a minute.
pub const DEV_TRANSITION: TransitionParams = TransitionParams {
    stake_bps: 1_000,
    min_candidates: 1,
    min_height: 20,
    sustain_blocks: 20,
};

/// Switch parameters of the local-testnet preset: three candidates, 40 + 40 blocks.
pub const LOCAL_TRANSITION: TransitionParams = TransitionParams {
    stake_bps: 1_000,
    min_candidates: 3,
    min_height: 40,
    sustain_blocks: 40,
};

/// Market delays of the test presets: rate interval, heartbeat interval and escrow withdrawal
/// (live chains: one day, 600 blocks and one day).
const DEV_MARKET_DELAY: u64 = 10;
/// Unbonding period of providers and gateways on the test presets (live chains: seven days).
const DEV_MARKET_UNBOND: u64 = 20;
/// Epochs a work report stays queryable after it matures, in the dev and local presets.
const DEV_WORK_RETENTION: u32 = 20;

/// Parameters of one test preset.
struct Preset<'a> {
    authorities: &'a [&'a str],
    epoch_length: u64,
    admins: &'a [&'a str],
    threshold: u32,
    staking: StakingParams,
    transition: TransitionParams,
    validator_count: u32,
}

fn testnet_genesis(preset: &Preset<'_>) -> Result<Value, ac_crypto::Error> {
    let Preset {
        authorities,
        epoch_length,
        admins,
        threshold,
        staking,
        transition,
        validator_count,
    } = *preset;
    let admins = admins
        .iter()
        .map(|name| dev_account(name))
        .collect::<Result<Vec<_>, _>>()?;
    let authorities = authorities
        .iter()
        .map(|name| dev_public_key(name, SigAlg::MlDsa65))
        .collect::<Result<Vec<_>, _>>()?;
    let mut balances = DEV_ACCOUNTS
        .iter()
        .map(|name| dev_account(name).map(|a| (a, DEV_ENDOWMENT)))
        .collect::<Result<Vec<_>, _>>()?;
    balances.push((dev_wallet_account()?, DEV_ENDOWMENT));
    Ok(build_struct_json_patch!(RuntimeGenesisConfig {
        balances: BalancesConfig { balances },
        aura_pq: pallet_aura_pq::GenesisConfig { authorities },
        validator_set: pallet_validator_set::GenesisConfig {
            epoch_length,
            transition,
            validator_count
        },
        // Emission epochs as short as the validator epochs, so tests see settlements quickly;
        // both lengths divide the four-year period.
        emission: pallet_emission::GenesisConfig { epoch_length },
        treasury_dual: pallet_treasury_dual::GenesisConfig {
            community_share: pallet_treasury_dual::DEFAULT_COMMUNITY_SHARE
        },
        poa_council: pallet_collective::GenesisConfig { members: admins },
        // Short motions so tests can let them expire.
        poa_admin: pallet_poa_admin::GenesisConfig {
            threshold,
            motion_duration: DEV_MOTION_DURATION
        },
        staking_pos: pallet_staking_pos::GenesisConfig { params: staking },
        // Market (m5-market-registry design D8): 1 ATC = 1 USD and short periods so tests see
        // heartbeat expiry, unbonding and withdrawals within a few blocks.
        ref_rate: pallet_ref_rate::GenesisConfig {
            initial: Some(ac_primitives::market::AtcPerUsd(ATC)),
            params: pallet_ref_rate::RefRateParams {
                min_interval: DEV_MARKET_DELAY
            }
        },
        providers: pallet_providers::GenesisConfig {
            params: pallet_providers::ProviderParams {
                heartbeat_interval: DEV_MARKET_DELAY,
                unbond_blocks: DEV_MARKET_UNBOND,
                ..pallet_providers::ProviderParams::LIVE
            }
        },
        gateways: pallet_gateways::GenesisConfig {
            params: pallet_gateways::GatewayParams {
                unbond_blocks: DEV_MARKET_UNBOND,
                ..pallet_gateways::GatewayParams::LIVE
            }
        },
        credits: pallet_credits::GenesisConfig {
            params: pallet_credits::CreditsParams {
                withdrawal_delay: DEV_MARKET_DELAY
            }
        },
        work: pallet_work::GenesisConfig {
            params: ac_primitives::market::WorkParams {
                retention_epochs: DEV_WORK_RETENTION,
                ..ac_primitives::market::WorkParams::LIVE
            }
        },
    }))
}

/// Returns the JSON patch for a named preset, or `None` for unknown names.
pub fn get_preset(id: &PresetId) -> Option<Vec<u8>> {
    let patch = match id.as_ref() {
        // Short epochs so tests see authority-set changes and randomness quickly.
        sp_genesis_builder::DEV_RUNTIME_PRESET => testnet_genesis(&Preset {
            authorities: &["alice"],
            epoch_length: 10,
            admins: &["alice"],
            threshold: 1,
            staking: DEV_STAKING,
            transition: DEV_TRANSITION,
            validator_count: 5,
        }),
        sp_genesis_builder::LOCAL_TESTNET_RUNTIME_PRESET => {
            // Four authorities tolerate one faulty member (n ≥ 3f + 1).
            testnet_genesis(&Preset {
                authorities: &["alice", "bob", "charlie", "dave"],
                epoch_length: 20,
                admins: &["alice", "bob", "charlie"],
                threshold: 2,
                staking: LOCAL_STAKING,
                transition: LOCAL_TRANSITION,
                validator_count: 10,
            })
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
