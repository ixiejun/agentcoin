//! Block authoring: claims the slots assigned to the local authority key and seals blocks.

use std::marker::PhantomData;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use ac_crypto::PqPublicKey;
use ac_crypto::sig::SigningKey;
use ac_primitives::aura_pq::{AuraPqApi, Slot, find_slot, pre_digest, slot_author};
use futures::{Future, TryFutureExt};
use sc_consensus::{BlockImport, BlockImportParams, ForkChoiceStrategy, StateAction};
use sc_consensus_slots::{
    SimpleSlotWorker, SlotInfo, SlotLenienceType, SlotProportion, StorageChanges,
};
use sc_telemetry::TelemetryHandle;
use sp_api::ProvideRuntimeApi;
use sp_blockchain::HeaderBackend;
use sp_consensus::{BlockOrigin, Environment, Error as ConsensusError, Proposer, SyncOracle};
use sp_runtime::DigestItem;
use sp_runtime::traits::{Block as BlockT, Header as HeaderT};

use crate::seal::seal;

const LOG_TARGET: &str = "aura-pq";

/// Aura-PQ slot worker.
pub struct AuraPqWorker<B, C, E, I, SO, L> {
    pub(crate) client: Arc<C>,
    pub(crate) block_import: I,
    pub(crate) env: E,
    pub(crate) key: SigningKey,
    pub(crate) public: PqPublicKey,
    pub(crate) sync_oracle: SO,
    pub(crate) justification_sync_link: L,
    pub(crate) force_authoring: bool,
    pub(crate) block_proposal_slot_portion: SlotProportion,
    pub(crate) max_block_proposal_slot_portion: Option<SlotProportion>,
    pub(crate) telemetry: Option<TelemetryHandle>,
    pub(crate) _phantom: PhantomData<fn() -> B>,
}

#[async_trait::async_trait]
impl<B, C, E, I, SO, L, Error> SimpleSlotWorker<B> for AuraPqWorker<B, C, E, I, SO, L>
where
    B: BlockT,
    C: ProvideRuntimeApi<B> + HeaderBackend<B> + Send + Sync,
    C::Api: AuraPqApi<B>,
    E: Environment<B, Error = Error> + Send + Sync,
    E::Proposer: Proposer<B, Error = Error>,
    I: BlockImport<B> + Send + Sync + 'static,
    SO: SyncOracle + Send + Sync + Clone,
    L: sc_consensus::JustificationSyncLink<B>,
    Error: std::error::Error + Send + From<ConsensusError> + 'static,
{
    type BlockImport = I;
    type SyncOracle = SO;
    type JustificationSyncLink = L;
    type CreateProposer =
        Pin<Box<dyn Future<Output = Result<E::Proposer, ConsensusError>> + Send + 'static>>;
    type Proposer = E::Proposer;
    type Claim = PqPublicKey;
    type AuxData = Vec<PqPublicKey>;

    fn logging_target(&self) -> &'static str {
        LOG_TARGET
    }

    fn block_import(&mut self) -> &mut Self::BlockImport {
        &mut self.block_import
    }

    fn aux_data(&self, header: &B::Header, _slot: Slot) -> Result<Self::AuxData, ConsensusError> {
        self.client
            .runtime_api()
            .authorities(header.hash())
            .map_err(|e| ConsensusError::ClientImport(e.to_string()))
    }

    fn authorities_len(&self, authorities: &Self::AuxData) -> Option<usize> {
        Some(authorities.len())
    }

    async fn claim_slot(
        &mut self,
        _header: &B::Header,
        slot: Slot,
        authorities: &Self::AuxData,
    ) -> Option<Self::Claim> {
        (slot_author(slot, authorities) == Some(&self.public)).then(|| self.public.clone())
    }

    fn pre_digest_data(&self, slot: Slot, _claim: &Self::Claim) -> Vec<DigestItem> {
        vec![pre_digest(slot)]
    }

    async fn block_import_params(
        &self,
        header: B::Header,
        header_hash: &B::Hash,
        body: Vec<B::Extrinsic>,
        storage_changes: StorageChanges<B>,
        _claim: Self::Claim,
        _authorities: Self::AuxData,
    ) -> Result<BlockImportParams<B>, ConsensusError> {
        let seal_item = seal(&self.key, header_hash.as_ref())
            .map_err(|e| ConsensusError::CannotSign(format!("cannot seal block: {e}")))?;
        let mut import = BlockImportParams::new(BlockOrigin::Own, header);
        import.post_digests.push(seal_item);
        import.body = Some(body);
        import.state_action =
            StateAction::ApplyChanges(sc_consensus::StorageChanges::Changes(storage_changes));
        import.fork_choice = Some(ForkChoiceStrategy::LongestChain);
        Ok(import)
    }

    fn force_authoring(&self) -> bool {
        self.force_authoring
    }

    fn sync_oracle(&mut self) -> &mut Self::SyncOracle {
        &mut self.sync_oracle
    }

    fn justification_sync_link(&mut self) -> &mut Self::JustificationSyncLink {
        &mut self.justification_sync_link
    }

    fn proposer(&mut self, block: &B::Header) -> Self::CreateProposer {
        Box::pin(
            self.env
                .init(block)
                .map_err(|e| ConsensusError::ClientImport(format!("{e:?}"))),
        )
    }

    fn telemetry(&self) -> Option<TelemetryHandle> {
        self.telemetry.clone()
    }

    fn proposing_remaining_duration(&self, slot_info: &SlotInfo<B>) -> Duration {
        let parent_slot = find_slot(slot_info.chain_head.digest()).ok();
        sc_consensus_slots::proposing_remaining_duration(
            parent_slot,
            slot_info,
            &self.block_proposal_slot_portion,
            self.max_block_proposal_slot_portion.as_ref(),
            SlotLenienceType::Exponential,
            LOG_TARGET,
        )
    }
}
