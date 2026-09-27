//! Constitution layer 1 in the node (spec node/invariants; design D5 of `m3-economics`).
//!
//! [`InvariantBlockImport`] sits between the AC-BFT block import and the client, so every block
//! — authored here, received from the network or synced — passes through it. It obtains the
//! block's storage changes (given by the proposer for our own blocks; for other blocks it
//! executes the block on the parent state itself, exactly as the client would, and hands the
//! result on so the block is not executed twice), reads the well-known issuance and burned
//! values before and after, and runs [`ac_invariants::check_block`]. It also reads the PoA → PoS
//! switch state before and after and runs [`ac_invariants::check_transition`], which at PoA
//! epoch boundaries recomputes the switch checkpoint from the parent state's stake ledger. A
//! violation rejects the block: it is neither imported nor, for our own blocks, announced.

use std::sync::Arc;

use ac_invariants::{
    GenesisParams, Ledger, StakeSnapshot, SwitchState, Violation, check_block, check_genesis,
    check_transition, keys, read_ledger, read_switch_state, stake_snapshot,
};
use ac_runtime::opaque::Block;
use sc_client_api::{Backend as _, StorageProvider};
use sc_consensus::{
    BlockCheckParams, BlockImport, BlockImportParams, ImportResult, StateAction, StorageChanges,
};
use sp_api::{ApiExt, CallContext, Core, ProvideRuntimeApi};
use sp_consensus::Error as ConsensusError;
use sp_core::storage::StorageKey;
use sp_runtime::traits::{Block as BlockT, Header as HeaderT};

use crate::service::{FullBackend, FullClient};

/// Log target of the invariant checks.
pub const LOG_TARGET: &str = "ac-invariants";

/// Constitution layer 1 at start-up (spec node/invariants "启动时校验创世参数", node/chain-spec
/// "正式链创世零发行"): reads the chain's genesis state from the client — built from the chain
/// spec, so no second genesis build is needed — and checks it with
/// [`ac_invariants::check_genesis`]: a valid emission epoch length and an issuance within the
/// cap for every chain; for a live chain also zero issuance, no balances and at least one PoA
/// admin member. Returns the parameters of the per-block checks.
///
/// # Errors
///
/// The genesis state cannot be read or breaks a rule; the node must not start.
pub fn genesis_params(client: &FullClient, live: bool) -> Result<GenesisParams, String> {
    use sc_client_api::HeaderBackend;
    let genesis = client.info().genesis_hash;
    let read = |key: &[u8]| -> Result<Option<Vec<u8>>, String> {
        client
            .storage(genesis, &StorageKey(key.to_vec()))
            .map(|v| v.map(|v| v.0))
            .map_err(|e| format!("cannot read the genesis state: {e}"))
    };
    let mut entries = Vec::new();
    for key in [
        keys::TOTAL_ISSUANCE.as_slice(),
        keys::EMISSION_EPOCH_LENGTH.as_slice(),
        keys::POA_COUNCIL_MEMBERS.as_slice(),
        keys::TRANSITION_PARAMS.as_slice(),
        keys::VALIDATOR_EPOCH_LENGTH.as_slice(),
    ] {
        if let Some(value) = read(key)? {
            entries.push((key.to_vec(), value));
        }
    }
    let prefix = StorageKey(keys::SYSTEM_ACCOUNT_PREFIX.to_vec());
    let accounts = client
        .storage_keys(genesis, Some(&prefix), None)
        .map_err(|e| format!("cannot read the genesis state: {e}"))?;
    for key in accounts {
        if let Some(value) = read(&key.0)? {
            entries.push((key.0, value));
        }
    }
    check_genesis(
        entries.iter().map(|(k, v)| (k.as_slice(), v.as_slice())),
        live,
    )
    .map_err(|e| format!("invalid chain spec genesis: {e}"))
}

type Hash = <Block as BlockT>::Hash;

/// A storage entry: key suffix (account) and value.
type Entry = (Vec<u8>, Vec<u8>);

/// Block import that enforces the node invariants before handing blocks to the client.
#[derive(Clone)]
pub struct InvariantBlockImport {
    client: Arc<FullClient>,
    backend: Arc<FullBackend>,
    params: GenesisParams,
}

impl InvariantBlockImport {
    /// Wraps `client`, checking every block against `params` (from [`genesis_params`]).
    pub fn new(client: Arc<FullClient>, backend: Arc<FullBackend>, params: GenesisParams) -> Self {
        Self {
            client,
            backend,
            params,
        }
    }

    /// Executes `block` on its parent state and returns its storage changes, checking the state
    /// root as the client would.
    fn execute(
        &self,
        params: &BlockImportParams<Block>,
    ) -> Result<sp_api::StorageChanges<Block>, ConsensusError> {
        let parent = *params.header.parent_hash();
        let body = params
            .body
            .clone()
            .ok_or_else(|| ConsensusError::ClientImport("block without body".into()))?;
        let mut api = self.client.runtime_api();
        let context = CallContext::Onchain { import: true };
        api.set_call_context(context);
        api.execute_block(parent, Block::new(params.header.clone(), body).into())
            .map_err(|e| ConsensusError::ClientImport(format!("cannot execute block: {e}")))?;
        let state = self
            .backend
            .state_at(parent, context.into())
            .map_err(|e| ConsensusError::ClientImport(format!("no parent state: {e}")))?;
        let changes = api
            .into_storage_changes(&state, parent)
            .map_err(|e| ConsensusError::ClientImport(format!("no storage changes: {e}")))?;
        if params.header.state_root() != &changes.transaction_storage_root {
            return Err(ConsensusError::ClientImport("invalid state root".into()));
        }
        Ok(changes)
    }

    /// A value of the parent state.
    fn parent_value(&self, parent: Hash, key: &[u8]) -> Option<Vec<u8>> {
        self.client
            .storage(parent, &StorageKey(key.to_vec()))
            .ok()
            .flatten()
            .map(|v| v.0)
    }

    /// A value of the block's state: its own change if it wrote the key, else the parent's.
    fn post_value(
        &self,
        parent: Hash,
        changes: &sp_api::StorageChanges<Block>,
        key: &[u8],
    ) -> Option<Vec<u8>> {
        match changes
            .main_storage_changes
            .iter()
            .rev()
            .find(|(k, _)| k.as_slice() == key)
        {
            Some((_, value)) => value.clone(),
            None => self.parent_value(parent, key),
        }
    }

    /// The `(account, value)` entries of an `Identity`-hashed map in the parent state.
    fn parent_map(&self, parent: Hash, prefix: &[u8]) -> Result<Vec<Entry>, Violation> {
        let pairs = self
            .client
            .storage_pairs(parent, Some(&StorageKey(prefix.to_vec())), None)
            .map_err(|_| Violation::MissingKey("stake ledger of the parent state"))?;
        Ok(pairs
            .filter_map(|(key, value)| {
                key.0
                    .strip_prefix(prefix)
                    .map(|suffix| (suffix.to_vec(), value.0))
            })
            .collect())
    }

    /// The parent state's stake snapshot for a PoA checkpoint.
    fn parent_stake(&self, parent: Hash, issuance: u128) -> Result<StakeSnapshot, Violation> {
        let ledgers = self.parent_map(parent, &keys::STAKING_LEDGER_PREFIX)?;
        let candidates = self.parent_map(parent, &keys::STAKING_CANDIDATES_PREFIX)?;
        stake_snapshot(
            issuance,
            ledgers.iter().map(|(k, v)| (k.as_slice(), v.as_slice())),
            candidates.iter().map(|(k, v)| (k.as_slice(), v.as_slice())),
        )
    }

    fn check(
        &self,
        params: &GenesisParams,
        number: u32,
        parent: Hash,
        changes: &sp_api::StorageChanges<Block>,
    ) -> Result<(), Violation> {
        let pre: Ledger = read_ledger(|key| self.parent_value(parent, key))?;
        let post = read_ledger(|key| self.post_value(parent, changes, key))?;
        check_block(params, u64::from(number), pre, post)?;
        let pre_switch: SwitchState = read_switch_state(|key| self.parent_value(parent, key))?;
        let post_switch = read_switch_state(|key| self.post_value(parent, changes, key))?;
        check_transition(
            &params.transition,
            u64::from(number),
            &pre_switch,
            &post_switch,
            || self.parent_stake(parent, pre.issuance),
        )
    }
}

#[async_trait::async_trait]
impl BlockImport<Block> for InvariantBlockImport {
    type Error = ConsensusError;

    async fn check_block(
        &self,
        block: BlockCheckParams<Block>,
    ) -> Result<ImportResult, Self::Error> {
        self.client.check_block(block).await
    }

    async fn import_block(
        &self,
        mut params: BlockImportParams<Block>,
    ) -> Result<ImportResult, Self::Error> {
        let number = *params.header.number();
        let parent = *params.header.parent_hash();
        let parent_number = number.saturating_sub(1);
        let parent_state = self.backend.have_state_at(parent, parent_number);
        let action = std::mem::replace(&mut params.state_action, StateAction::Skip);
        let changes = match action {
            StateAction::ApplyChanges(StorageChanges::Changes(changes)) => Some(changes),
            StateAction::Execute | StateAction::ExecuteIfPossible
                if parent_state && params.body.is_some() =>
            {
                let changes = self.execute(&params)?;
                log::debug!(target: LOG_TARGET, "executed block #{number} for the invariant check");
                Some(changes)
            }
            // Without parent state the client discards the block (`Execute`) or imports it
            // without state (`ExecuteIfPossible`, already-final history): nothing to check.
            action @ (StateAction::Execute | StateAction::ExecuteIfPossible) => {
                params.state_action = action;
                None
            }
            // State imported wholesale or skipped cannot be checked: rejected (fail-closed, no
            // warp / fast sync).
            StateAction::ApplyChanges(StorageChanges::Import(_)) | StateAction::Skip => {
                return Err(ConsensusError::ClientImport(format!(
                    "block #{number} would be imported without executing it; the node invariants \
                     require full execution"
                )));
            }
        };
        if let Some(changes) = changes {
            if let Err(violation) = self.check(&self.params, number, parent, &changes) {
                log::error!(
                    target: LOG_TARGET,
                    "rejecting block #{number} ({:?}): {violation}",
                    params.post_hash()
                );
                return Err(ConsensusError::ClientImport(violation.to_string()));
            }
            params.state_action = StateAction::ApplyChanges(StorageChanges::Changes(changes));
        }
        self.client.import_block(params).await
    }
}
