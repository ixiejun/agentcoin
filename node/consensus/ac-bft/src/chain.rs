//! The client's block tree as seen by the AC-BFT state machine and the tracker.

use std::marker::PhantomData;

use ac_primitives::ac_bft::BlockRef;
use sp_blockchain::{HeaderBackend, HeaderMetadata};
use sp_core::H256;
use sp_runtime::traits::{Block as BlockT, Header as HeaderT};

use crate::protocol::Chain;
use crate::tracker::{Ancestry, TrackerState};

/// Ancestry queries answered by the client's header metadata.
pub struct ClientAncestry<'a, B, C> {
    client: &'a C,
    _block: PhantomData<B>,
}

impl<'a, B, C> ClientAncestry<'a, B, C> {
    /// Queries `client`.
    pub fn new(client: &'a C) -> Self {
        Self {
            client,
            _block: PhantomData,
        }
    }
}

impl<B, C> Ancestry for ClientAncestry<'_, B, C>
where
    B: BlockT<Hash = H256>,
    B::Header: HeaderT<Number = u32>,
    C: HeaderBackend<B> + HeaderMetadata<B, Error = sp_blockchain::Error>,
{
    fn is_descendant(&self, block: &BlockRef, ancestor: &BlockRef) -> bool {
        is_descendant(self.client, block, ancestor)
    }
}

fn contains<B, C>(client: &C, block: &BlockRef) -> bool
where
    B: BlockT<Hash = H256>,
    B::Header: HeaderT<Number = u32>,
    C: HeaderBackend<B>,
{
    matches!(client.number(block.hash), Ok(Some(n)) if n == block.number)
}

fn is_descendant<B, C>(client: &C, block: &BlockRef, ancestor: &BlockRef) -> bool
where
    B: BlockT<Hash = H256>,
    B::Header: HeaderT<Number = u32>,
    C: HeaderBackend<B> + HeaderMetadata<B, Error = sp_blockchain::Error>,
{
    if block.number < ancestor.number || !contains(client, block) || !contains(client, ancestor) {
        return false;
    }
    if block == ancestor {
        return true;
    }
    matches!(
        sp_blockchain::lowest_common_ancestor(client, block.hash, ancestor.hash),
        Ok(lca) if lca.hash == ancestor.hash
    )
}

/// [`Chain`] over the client, with pending set changes from the tracker.
pub struct ClientChain<'a, B, C> {
    client: &'a C,
    tracker: &'a TrackerState,
    _block: PhantomData<B>,
}

impl<'a, B, C> ClientChain<'a, B, C> {
    /// A view of `client` with `tracker`'s pending changes.
    pub fn new(client: &'a C, tracker: &'a TrackerState) -> Self {
        Self {
            client,
            tracker,
            _block: PhantomData,
        }
    }
}

impl<B, C> Chain for ClientChain<'_, B, C>
where
    B: BlockT<Hash = H256>,
    B::Header: HeaderT<Number = u32>,
    C: HeaderBackend<B> + HeaderMetadata<B, Error = sp_blockchain::Error>,
{
    fn contains(&self, block: &BlockRef) -> bool {
        contains(self.client, block)
    }

    fn is_descendant(&self, block: &BlockRef, ancestor: &BlockRef) -> bool {
        is_descendant(self.client, block, ancestor)
    }

    fn best(&self) -> BlockRef {
        let info = self.client.info();
        let best = BlockRef {
            hash: info.best_hash,
            number: info.best_number,
        };
        let finalized = BlockRef {
            hash: info.finalized_hash,
            number: info.finalized_number,
        };
        // The client moves its best block onto the finalized chain when finalizing; this only
        // guards against a stale view in between.
        if is_descendant(self.client, &best, &finalized) {
            best
        } else {
            finalized
        }
    }

    fn first_change(&self, after: &BlockRef, upto: &BlockRef) -> Option<BlockRef> {
        self.tracker
            .first_change(after, upto, &ClientAncestry::<B, C>::new(self.client))
    }
}
