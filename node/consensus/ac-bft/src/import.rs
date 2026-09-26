//! Finality proofs on import and sync (design D5 of `m2-finality`).
//!
//! [`AcBftBlockImport`] wraps the node's block import:
//! - blocks announcing an authority-set change are recorded in the tracker, and ask the sync
//!   service for their finality proof when they arrive without one;
//! - a finality proof arriving with a block, or on its own through [`JustificationImport`], is
//!   verified against the set that must finalize that block; a valid proof finalizes the block
//!   and is stored, an invalid one is dropped and reported as bad.

use std::marker::PhantomData;
use std::sync::{Arc, Mutex};

use ac_primitives::ac_bft::{
    BlockRef, ConsensusLog, ENGINE_ID, VersionedFinalityProof, verify_finality_proof,
};
use parity_scale_codec::Decode;
use sc_client_api::{AuxStore, Backend, Finalizer};
use sc_consensus::{
    BlockCheckParams, BlockImport, BlockImportParams, ImportResult, JustificationImport,
};
use sp_blockchain::{HeaderBackend, HeaderMetadata};
use sp_consensus::Error as ConsensusError;
use sp_core::H256;
use sp_runtime::Justification;
use sp_runtime::traits::{Block as BlockT, Header as HeaderT, NumberFor};

use crate::chain::ClientAncestry;
use crate::tracker::{self, TrackerState};

/// Shared, persisted tracker.
pub type SharedTracker = Arc<Mutex<TrackerState>>;

/// Block import that verifies and applies AC-BFT finality proofs.
pub struct AcBftBlockImport<B, BE, I, C> {
    inner: I,
    client: Arc<C>,
    tracker: SharedTracker,
    _marker: PhantomData<(B, BE)>,
}

impl<B, BE, I: Clone, C> Clone for AcBftBlockImport<B, BE, I, C> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
            client: self.client.clone(),
            tracker: self.tracker.clone(),
            _marker: PhantomData,
        }
    }
}

impl<B, BE, I, C> AcBftBlockImport<B, BE, I, C> {
    /// Wraps `inner`.
    pub fn new(inner: I, client: Arc<C>, tracker: SharedTracker) -> Self {
        Self {
            inner,
            client,
            tracker,
            _marker: PhantomData,
        }
    }
}

/// Why a finality proof is rejected.
#[derive(Debug, PartialEq, Eq)]
pub enum ProofError {
    /// Not an AC-BFT justification, or it does not decode.
    Malformed,
    /// The proof does not verify against the set that must finalize the block, or it is for
    /// another block.
    Invalid,
}

impl<B, BE, I, C> AcBftBlockImport<B, BE, I, C>
where
    B: BlockT<Hash = H256>,
    B::Header: HeaderT<Number = u32>,
    BE: Backend<B>,
    C: HeaderBackend<B>
        + HeaderMetadata<B, Error = sp_blockchain::Error>
        + AuxStore
        + Finalizer<B, BE>,
{
    /// Verifies `justification` for `block` and, if valid, finalizes the block and stores the
    /// proof.
    ///
    /// # Errors
    ///
    /// [`ProofError`] for invalid proofs; client errors are reported as [`ProofError::Invalid`]
    /// after logging, since the proof cannot be applied.
    pub fn apply_proof(
        &self,
        block: BlockRef,
        justification: &Justification,
    ) -> Result<(), ProofError> {
        if justification.0 != ENGINE_ID {
            return Err(ProofError::Malformed);
        }
        let proof = VersionedFinalityProof::decode(&mut &justification.1[..])
            .map_err(|_| ProofError::Malformed)?;
        let genesis = self.client.info().genesis_hash;
        let chain = ClientAncestry::<B, C>::new(&*self.client);
        let (set_id, authorities) = {
            let tracker = self.tracker.lock().map_err(|_| ProofError::Invalid)?;
            if block.number <= tracker.finalized.number {
                // Already final: nothing to do (a proof for an old block is harmless).
                return Ok(());
            }
            tracker.set_for(&block, &chain)
        };
        let target = verify_finality_proof(&genesis, set_id, &authorities, &proof)
            .map_err(|_| ProofError::Invalid)?;
        if target != block {
            return Err(ProofError::Invalid);
        }
        if let Err(e) = self
            .client
            .finalize_block(block.hash, Some(justification.clone()), true)
        {
            log::warn!(target: crate::LOG_TARGET, "cannot finalize #{} with an imported proof: {e}", block.number);
            return Err(ProofError::Invalid);
        }
        let mut tracker = self.tracker.lock().map_err(|_| ProofError::Invalid)?;
        tracker.on_finalized(block, &chain);
        tracker.last_stored_proof = block.number;
        if let Err(e) = tracker::store(&*self.client, &tracker) {
            log::warn!(target: crate::LOG_TARGET, "cannot persist the AC-BFT tracker: {e}");
        }
        Ok(())
    }
}

#[async_trait::async_trait]
impl<B, BE, I, C> BlockImport<B> for AcBftBlockImport<B, BE, I, C>
where
    B: BlockT<Hash = H256>,
    B::Header: HeaderT<Number = u32>,
    BE: Backend<B> + Send + Sync,
    I: BlockImport<B, Error = ConsensusError> + Send + Sync,
    C: HeaderBackend<B>
        + HeaderMetadata<B, Error = sp_blockchain::Error>
        + AuxStore
        + Finalizer<B, BE>
        + Send
        + Sync,
{
    type Error = ConsensusError;

    async fn check_block(&self, block: BlockCheckParams<B>) -> Result<ImportResult, Self::Error> {
        self.inner.check_block(block).await
    }

    async fn import_block(
        &self,
        mut params: BlockImportParams<B>,
    ) -> Result<ImportResult, Self::Error> {
        let block = BlockRef {
            hash: params.post_hash(),
            number: *params.header.number(),
        };
        let change = ConsensusLog::find_change(params.header.digest().logs());
        // Verified and stored by `apply_proof`, never stored unverified.
        let justification = params
            .justifications
            .as_ref()
            .and_then(|j| j.get(ENGINE_ID).cloned())
            .map(|proof| (ENGINE_ID, proof));
        if let Some(justifications) = params.justifications.as_mut() {
            justifications.remove(ENGINE_ID);
            if justifications.iter().next().is_none() {
                params.justifications = None;
            }
        }
        let mut result = self.inner.import_block(params).await?;
        if let ImportResult::Imported(aux) = &mut result {
            if let Some(change) = &change
                && let Ok(mut tracker) = self.tracker.lock()
            {
                tracker.note_change(block, change);
                if let Err(e) = tracker::store(&*self.client, &tracker) {
                    log::warn!(target: crate::LOG_TARGET, "cannot persist the AC-BFT tracker: {e}");
                }
            }
            let applied = match &justification {
                Some(j) => match self.apply_proof(block, j) {
                    Ok(()) => true,
                    Err(e) => {
                        log::debug!(target: crate::LOG_TARGET, "bad finality proof for #{}: {e:?}", block.number);
                        aux.bad_justification = true;
                        false
                    }
                },
                None => false,
            };
            if change.is_some() && !applied {
                aux.needs_justification = true;
            }
        }
        Ok(result)
    }
}

#[async_trait::async_trait]
impl<B, BE, I, C> JustificationImport<B> for AcBftBlockImport<B, BE, I, C>
where
    B: BlockT<Hash = H256>,
    B::Header: HeaderT<Number = u32>,
    BE: Backend<B> + Send + Sync,
    I: Send + Sync,
    C: HeaderBackend<B>
        + HeaderMetadata<B, Error = sp_blockchain::Error>
        + AuxStore
        + Finalizer<B, BE>
        + Send
        + Sync,
{
    type Error = ConsensusError;

    async fn on_start(&mut self) -> Vec<(B::Hash, NumberFor<B>)> {
        // Ask peers for the proofs of change blocks still pending.
        self.tracker
            .lock()
            .map(|t| {
                t.pending
                    .iter()
                    .map(|(b, _, _)| (b.hash, b.number))
                    .collect()
            })
            .unwrap_or_default()
    }

    async fn import_justification(
        &mut self,
        hash: B::Hash,
        number: NumberFor<B>,
        justification: Justification,
    ) -> Result<(), Self::Error> {
        self.apply_proof(BlockRef { hash, number }, &justification)
            .map_err(|e| {
                ConsensusError::ClientImport(format!("invalid AC-BFT finality proof: {e:?}"))
            })
    }
}
