//! Import verifier: every block from the network (or our own after a restart) is checked here.

use std::sync::Arc;

use ac_primitives::aura_pq::{AuraPqApi, find_slot};
use sc_client_api::backend::AuxStore;
use sc_consensus::import_queue::{BasicQueue, BoxBlockImport, BoxJustificationImport};
use sc_consensus::{BlockImportParams, Verifier};
use sc_consensus_slots::InherentDataProviderExt;
use sp_api::ProvideRuntimeApi;
use sp_block_builder::BlockBuilder as BlockBuilderApi;
use sp_blockchain::HeaderBackend;
use sp_inherents::{CreateInherentDataProviders, InherentDataProvider};
use sp_runtime::traits::{Block as BlockT, Header as HeaderT};

use crate::equivocation;
use crate::seal::{SealError, check_header};

/// Submits double-signing evidence on chain (the node implements it with the transaction pool).
pub trait EquivocationReporter: Send + Sync {
    /// Reports `evidence` (best effort).
    fn report(&self, evidence: ac_primitives::offences::Evidence);
}

/// Verifies Aura-PQ headers before import.
pub struct AuraPqVerifier<C, CIDP> {
    client: Arc<C>,
    create_inherent_data_providers: CIDP,
    reporter: Option<Arc<dyn EquivocationReporter>>,
}

impl<C, CIDP> AuraPqVerifier<C, CIDP> {
    /// Creates a verifier; seal double signing is reported through `reporter` when given.
    pub fn new(
        client: Arc<C>,
        create_inherent_data_providers: CIDP,
        reporter: Option<Arc<dyn EquivocationReporter>>,
    ) -> Self {
        Self {
            client,
            create_inherent_data_providers,
            reporter,
        }
    }
}

#[async_trait::async_trait]
impl<B, C, CIDP> Verifier<B> for AuraPqVerifier<C, CIDP>
where
    B: BlockT,
    C: ProvideRuntimeApi<B> + HeaderBackend<B> + AuxStore + Send + Sync,
    C::Api: AuraPqApi<B> + BlockBuilderApi<B>,
    CIDP: CreateInherentDataProviders<B, ()> + Send + Sync,
    CIDP::InherentDataProviders: InherentDataProviderExt + InherentDataProvider + Send + Sync,
{
    async fn verify(
        &self,
        mut block: BlockImportParams<B>,
    ) -> Result<BlockImportParams<B>, String> {
        // Blocks imported with state (warp sync) or explicitly trusted skip the checks.
        if block.with_state() || block.state_action.skip_execution_checks() {
            block.fork_choice = Some(sc_consensus::ForkChoiceStrategy::Custom(block.with_state()));
            return Ok(block);
        }

        let hash = block.header.hash();
        let parent_hash = *block.header.parent_hash();
        let authorities = self
            .client
            .runtime_api()
            .authorities(parent_hash)
            .map_err(|e| format!("cannot read Aura-PQ authorities at {parent_hash:?}: {e}"))?;
        let parent_slot = self
            .client
            .header(parent_hash)
            .map_err(|e| e.to_string())?
            .and_then(|h| find_slot(h.digest()).ok());

        let providers = self
            .create_inherent_data_providers
            .create_inherent_data_providers(parent_hash, ())
            .await
            .map_err(|e| format!("cannot create inherent data providers: {e}"))?;
        let slot_now = providers.slot();

        // One slot of clock drift is tolerated, as in upstream Aura.
        let full_header = block.header.clone();
        let checked = match check_header(block.header, parent_slot, slot_now + 1, &authorities) {
            Ok(checked) => checked,
            Err(SealError::FutureSlot(slot)) => {
                return Err(format!(
                    "header {hash:?} rejected: slot {slot} is too far in the future"
                ));
            }
            Err(e) => return Err(format!("header {hash:?} rejected: {e}")),
        };

        let report = equivocation::check(
            self.client.as_ref(),
            slot_now,
            checked.slot,
            &full_header,
            &checked.author,
        )
        .map_err(|e| e.to_string())?;
        if let (Some(report), Some(reporter)) = (report, &self.reporter)
            && let Some(evidence) = equivocation::evidence(&report, &checked.author)
        {
            reporter.report(evidence);
        }

        if let Some(body) = block.body.take() {
            let new_block = B::new(checked.pre_header.clone(), body);
            let inherent_data = providers
                .create_inherent_data()
                .await
                .map_err(|e| format!("cannot create inherent data: {e}"))?;
            sp_block_builder::check_inherents_with_data(
                self.client.clone(),
                parent_hash,
                new_block.clone(),
                &providers,
                inherent_data,
            )
            .await
            .map_err(|e| format!("invalid inherents in {hash:?}: {e:?}"))?;
            let (_, body) = new_block.deconstruct();
            block.body = Some(body);
        }

        block.header = checked.pre_header;
        block.post_digests.push(checked.seal);
        block.fork_choice = Some(sc_consensus::ForkChoiceStrategy::LongestChain);
        block.post_hash = Some(hash);
        Ok(block)
    }
}

/// Parameters of [`import_queue`].
pub struct ImportQueueParams<'a, B: BlockT, C, CIDP, S> {
    /// The client.
    pub client: Arc<C>,
    /// Block import run after verification (the AC-BFT wrapper around the client).
    pub block_import: BoxBlockImport<B>,
    /// Import of finality proofs requested separately from blocks.
    pub justification_import: Option<BoxJustificationImport<B>>,
    /// Inherent data providers for checking inherents.
    pub create_inherent_data_providers: CIDP,
    /// Receives seal double-signing evidence.
    pub reporter: Option<Arc<dyn EquivocationReporter>>,
    /// Spawner of the queue's task.
    pub spawner: &'a S,
    /// Metrics registry.
    pub registry: Option<&'a substrate_prometheus_endpoint::Registry>,
}

/// Builds the import queue that runs [`AuraPqVerifier`] before the block import.
pub fn import_queue<B, C, CIDP, S>(params: ImportQueueParams<'_, B, C, CIDP, S>) -> BasicQueue<B>
where
    B: BlockT,
    C: ProvideRuntimeApi<B> + HeaderBackend<B> + AuxStore + Send + Sync + 'static,
    C::Api: AuraPqApi<B> + BlockBuilderApi<B>,
    CIDP: CreateInherentDataProviders<B, ()> + Send + Sync + 'static,
    CIDP::InherentDataProviders: InherentDataProviderExt + InherentDataProvider + Send + Sync,
    S: sp_core::traits::SpawnEssentialNamed,
{
    let verifier = AuraPqVerifier::new(
        params.client,
        params.create_inherent_data_providers,
        params.reporter,
    );
    BasicQueue::new(
        verifier,
        params.block_import,
        params.justification_import,
        params.spawner,
        params.registry,
    )
}
