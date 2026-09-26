//! Authority-set tracking across forks (design D4 of `m2-finality`).
//!
//! The runtime announces set changes in boundary-block digests. A change applies to the blocks
//! after its change block, and only once the change block is finalized; the change block
//! itself is finalized by the old set. [`TrackerState`] records unfinalized change blocks on
//! every fork, answers which set must finalize a given block, and enacts changes as finality
//! advances. It is pure; [`load`] and [`store`] persist it in the client's auxiliary storage.

use ac_primitives::ac_bft::{Authority, BlockRef, ScheduledChange, SetId};
use parity_scale_codec::{Decode, Encode};

/// Auxiliary-storage key of the tracker.
pub const AUX_KEY: &[u8] = b"acbft:tracker";

/// Ancestry queries on the block tree.
pub trait Ancestry {
    /// Whether `block` is `ancestor` or one of its descendants.
    fn is_descendant(&self, block: &BlockRef, ancestor: &BlockRef) -> bool;
}

/// Tracked sets and pending changes.
#[derive(Clone, Debug, PartialEq, Eq, Encode, Decode)]
pub struct TrackerState {
    /// Id of the set that finalizes the blocks right after `finalized`.
    pub set_id: SetId,
    /// Members of that set.
    pub authorities: Vec<Authority>,
    /// Last finalized block.
    pub finalized: BlockRef,
    /// Unfinalized change blocks on any fork, with the set they announce.
    pub pending: Vec<(BlockRef, SetId, Vec<Authority>)>,
    /// Number of the last block whose finality proof was stored.
    pub last_stored_proof: u32,
}

impl TrackerState {
    /// Genesis state: set 0 finalizes the blocks after genesis.
    #[must_use]
    pub fn genesis(genesis: BlockRef, authorities: Vec<Authority>) -> Self {
        Self {
            set_id: 0,
            authorities,
            finalized: genesis,
            pending: Vec::new(),
            last_stored_proof: 0,
        }
    }

    /// Records the change announced by an imported block.
    pub fn note_change(&mut self, block: BlockRef, change: &ScheduledChange) {
        if block.number <= self.finalized.number || self.pending.iter().any(|(b, _, _)| *b == block)
        {
            return;
        }
        self.pending
            .push((block, change.set_id, change.authorities.to_vec()));
    }

    /// Whether `block` announces a set change.
    #[must_use]
    pub fn is_change_block(&self, block: &BlockRef) -> bool {
        self.pending.iter().any(|(b, _, _)| b == block)
    }

    /// The set that must finalize `block`: the set announced by the last change strictly
    /// before `block` on its chain, or the current set.
    #[must_use]
    pub fn set_for(&self, block: &BlockRef, chain: &impl Ancestry) -> (SetId, Vec<Authority>) {
        self.pending
            .iter()
            .filter(|(c, _, _)| c.number < block.number && chain.is_descendant(block, c))
            .max_by_key(|(c, _, _)| c.number)
            .map_or_else(
                || (self.set_id, self.authorities.clone()),
                |(_, id, set)| (*id, set.clone()),
            )
    }

    /// The first unfinalized change block after `after` up to `upto`, on `upto`'s chain.
    #[must_use]
    pub fn first_change(
        &self,
        after: &BlockRef,
        upto: &BlockRef,
        chain: &impl Ancestry,
    ) -> Option<BlockRef> {
        self.pending
            .iter()
            .map(|(c, _, _)| *c)
            .filter(|c| {
                c.number > after.number && c.number <= upto.number && chain.is_descendant(upto, c)
            })
            .min_by_key(|c| c.number)
    }

    /// Advances finality to `block`: enacts every change up to and including `block` on its
    /// chain (in order) and drops changes on abandoned forks. Returns the new set if it
    /// changed.
    pub fn on_finalized(
        &mut self,
        block: BlockRef,
        chain: &impl Ancestry,
    ) -> Option<(SetId, Vec<Authority>)> {
        if block.number <= self.finalized.number {
            return None;
        }
        let mut enacted: Vec<(BlockRef, SetId, Vec<Authority>)> = self
            .pending
            .iter()
            .filter(|(c, _, _)| c.number <= block.number && chain.is_descendant(&block, c))
            .cloned()
            .collect();
        enacted.sort_by_key(|(c, _, _)| c.number);
        let changed = enacted.last().map(|(_, id, set)| (*id, set.clone()));
        if let Some((id, set)) = &changed {
            self.set_id = *id;
            self.authorities.clone_from(set);
        }
        self.pending
            .retain(|(c, _, _)| c.number > block.number && chain.is_descendant(c, &block));
        self.finalized = block;
        changed
    }
}

/// Loads the tracker from auxiliary storage.
///
/// # Errors
///
/// Propagates storage errors; a stored value that does not decode is an error too.
pub fn load(aux: &impl sc_client_api::AuxStore) -> sp_blockchain::Result<Option<TrackerState>> {
    match aux.get_aux(AUX_KEY)? {
        None => Ok(None),
        Some(bytes) => TrackerState::decode(&mut &bytes[..])
            .map(Some)
            .map_err(|e| sp_blockchain::Error::Backend(format!("corrupt AC-BFT tracker: {e}"))),
    }
}

/// Stores the tracker in auxiliary storage.
///
/// # Errors
///
/// Propagates storage errors.
pub fn store(
    aux: &impl sc_client_api::AuxStore,
    state: &TrackerState,
) -> sp_blockchain::Result<()> {
    aux.insert_aux(&[(AUX_KEY, state.encode().as_slice())], &[])
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::indexing_slicing)]

    use super::*;
    use crate::protocol::Chain;
    use crate::sim::{TreeChain, dummy_authorities};
    use sp_runtime::BoundedVec;

    impl Ancestry for TreeChain {
        fn is_descendant(&self, block: &BlockRef, ancestor: &BlockRef) -> bool {
            Chain::is_descendant(self, block, ancestor)
        }
    }

    fn change(set_id: SetId, n: usize) -> ScheduledChange {
        ScheduledChange {
            set_id,
            authorities: BoundedVec::try_from(dummy_authorities(n)).unwrap(),
        }
    }

    // Changes on two forks; finalizing one fork enacts its change and drops the other.
    #[test]
    fn forks_and_enactment() {
        let mut chain = TreeChain::new();
        let g = TreeChain::genesis();
        let a1 = chain.child(&g, 1);
        let a2 = chain.child(&a1, 2);
        let b1 = chain.child(&g, 3);
        let b2 = chain.child(&b1, 4);
        let mut t = TrackerState::genesis(g, dummy_authorities(4));
        t.note_change(a1, &change(1, 3));
        t.note_change(b2, &change(1, 2));
        t.note_change(a1, &change(1, 3));
        assert_eq!(t.pending.len(), 2);
        // a1 itself is finalized by set 0, a2 by set 1.
        assert_eq!(t.set_for(&a1, &chain).0, 0);
        assert_eq!(t.set_for(&a2, &chain), (1, dummy_authorities(3)));
        assert_eq!(t.set_for(&b2, &chain).0, 0);
        assert_eq!(t.first_change(&g, &a2, &chain), Some(a1));
        assert_eq!(t.first_change(&g, &b1, &chain), None);
        assert!(t.is_change_block(&a1));

        assert_eq!(t.on_finalized(a1, &chain), Some((1, dummy_authorities(3))));
        assert_eq!((t.set_id, t.finalized), (1, a1));
        assert!(t.pending.is_empty(), "the other fork's change is dropped");
        assert_eq!(t.on_finalized(a2, &chain), None);
    }

    // A proof far ahead enacts every change on the way, in order.
    #[test]
    fn several_changes_at_once() {
        let mut chain = TreeChain::new();
        let g = TreeChain::genesis();
        let c1 = chain.child(&g, 1);
        let c2 = chain.child(&c1, 2);
        let c3 = chain.child(&c2, 3);
        let mut t = TrackerState::genesis(g, dummy_authorities(4));
        t.note_change(c1, &change(1, 3));
        t.note_change(c2, &change(2, 2));
        assert_eq!(t.set_for(&c3, &chain).0, 2);
        assert_eq!(t.on_finalized(c3, &chain).map(|s| s.0), Some(2));
        assert!(t.pending.is_empty());
    }

    // Round trip through auxiliary storage (restart).
    #[test]
    fn encoding_round_trip() {
        let mut chain = TreeChain::new();
        let g = TreeChain::genesis();
        let c1 = chain.child(&g, 1);
        let mut t = TrackerState::genesis(g, dummy_authorities(4));
        t.note_change(c1, &change(1, 3));
        t.last_stored_proof = 7;
        let decoded = TrackerState::decode(&mut &t.encode()[..]).unwrap();
        assert_eq!(decoded, t);
    }
}
