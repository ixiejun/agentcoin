//! # Aura-PQ
//!
//! Node side of AgentCoin's post-quantum block authoring (plan §4.1, decision D13): a fork of the
//! ideas of `sc-consensus-aura` on top of the generic `sc-consensus-slots` machinery, with
//! ML-DSA-65 authority keys held in memory (`ac_crypto::sig::SigningKey`) instead of the SDK
//! keystore, which only supports classic key types.
//!
//! - [`start_aura_pq`] runs the slot worker: slot `s` belongs to authority `s mod N`; the author
//!   seals the block with a hedged ML-DSA-65 signature (context `agentcoin/aura-seal/v1`).
//! - [`import_queue`] verifies every imported block: unique slot digest, slot within one slot of
//!   the local clock, slot above the parent's, author entitled to the slot, valid seal.
//! - Equivocations (two blocks of one author in one slot) are logged with both headers.

/// Runs the README example as a doctest.
#[cfg(doctest)]
#[doc = include_str!("../README.md")]
pub struct ReadmeDoctests;

pub mod equivocation;
pub mod seal;
mod verifier;
mod worker;

use std::marker::PhantomData;
use std::ops::Deref;
use std::sync::Arc;

use ac_crypto::sig::SigningKey;
use ac_primitives::aura_pq::{AuraPqApi, Slot};
use sc_consensus::BlockImport;
use sc_consensus_slots::{InherentDataProviderExt, SimpleSlotWorkerToSlotWorker, SlotProportion};
use sc_telemetry::TelemetryHandle;
use sp_api::ProvideRuntimeApi;
use sp_blockchain::HeaderBackend;
use sp_consensus::{Environment, Error as ConsensusError, Proposer, SelectChain, SyncOracle};
use sp_consensus_slots::SlotDuration;
use sp_inherents::{CreateInherentDataProviders, InherentData, InherentIdentifier};
use sp_runtime::traits::Block as BlockT;

pub use verifier::{AuraPqVerifier, EquivocationReporter, ImportQueueParams, import_queue};
pub use worker::AuraPqWorker;

/// Supplies the current slot to the slot machinery; contributes no inherent data (the runtime
/// reads the slot from the pre-runtime digest).
#[derive(Clone, Copy, Debug)]
pub struct SlotProvider(Slot);

impl SlotProvider {
    /// The slot containing `timestamp`.
    #[must_use]
    pub fn from_timestamp(timestamp: sp_timestamp::Timestamp, slot_duration: SlotDuration) -> Self {
        Self(Slot::from_timestamp(timestamp, slot_duration))
    }
}

impl Deref for SlotProvider {
    type Target = Slot;
    fn deref(&self) -> &Slot {
        &self.0
    }
}

#[async_trait::async_trait]
impl sp_inherents::InherentDataProvider for SlotProvider {
    async fn provide_inherent_data(&self, _: &mut InherentData) -> Result<(), sp_inherents::Error> {
        Ok(())
    }

    async fn try_handle_error(
        &self,
        _: &InherentIdentifier,
        _: &[u8],
    ) -> Option<Result<(), sp_inherents::Error>> {
        None
    }
}

/// Parameters of [`start_aura_pq`].
pub struct StartAuraPqParams<C, SC, I, PF, SO, L, CIDP> {
    /// Slot duration.
    pub slot_duration: SlotDuration,
    /// Client.
    pub client: Arc<C>,
    /// Chain selection.
    pub select_chain: SC,
    /// Where authored blocks are imported.
    pub block_import: I,
    /// Proposer factory.
    pub proposer_factory: PF,
    /// Sync oracle; no blocks are authored while major syncing.
    pub sync_oracle: SO,
    /// Justification sync link.
    pub justification_sync_link: L,
    /// Inherent data providers (slot first, then timestamp).
    pub create_inherent_data_providers: CIDP,
    /// Author even when offline.
    pub force_authoring: bool,
    /// The local authority key (ML-DSA-65).
    pub key: SigningKey,
    /// Portion of the slot used for proposing.
    pub block_proposal_slot_portion: SlotProportion,
    /// Telemetry.
    pub telemetry: Option<TelemetryHandle>,
}

/// Starts the Aura-PQ slot worker.
///
/// # Errors
///
/// [`ConsensusError::InvalidAuthoritiesSet`] if the key cannot produce a public key.
pub fn start_aura_pq<B, C, SC, I, PF, SO, L, CIDP, Error>(
    params: StartAuraPqParams<C, SC, I, PF, SO, L, CIDP>,
) -> Result<impl futures::Future<Output = ()>, ConsensusError>
where
    B: BlockT,
    C: ProvideRuntimeApi<B> + HeaderBackend<B> + Send + Sync + 'static,
    C::Api: AuraPqApi<B>,
    SC: SelectChain<B>,
    I: BlockImport<B> + Send + Sync + 'static,
    PF: Environment<B, Error = Error> + Send + Sync + 'static,
    PF::Proposer: Proposer<B, Error = Error>,
    SO: SyncOracle + Send + Sync + Clone + 'static,
    L: sc_consensus::JustificationSyncLink<B>,
    CIDP: CreateInherentDataProviders<B, ()> + Send + 'static,
    CIDP::InherentDataProviders: InherentDataProviderExt + Send,
    Error: std::error::Error + Send + From<ConsensusError> + 'static,
{
    let public = params
        .key
        .public_key()
        .map_err(|_| ConsensusError::InvalidAuthoritiesSet)?;
    let worker = AuraPqWorker {
        client: params.client,
        block_import: params.block_import,
        env: params.proposer_factory,
        key: params.key,
        public,
        sync_oracle: params.sync_oracle.clone(),
        justification_sync_link: params.justification_sync_link,
        force_authoring: params.force_authoring,
        block_proposal_slot_portion: params.block_proposal_slot_portion,
        max_block_proposal_slot_portion: None,
        telemetry: params.telemetry,
        _phantom: PhantomData,
    };
    Ok(sc_consensus_slots::start_slot_worker(
        params.slot_duration,
        params.select_chain,
        SimpleSlotWorkerToSlotWorker(worker),
        params.sync_oracle,
        params.create_inherent_data_providers,
    ))
}
