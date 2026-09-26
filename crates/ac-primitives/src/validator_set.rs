//! Interfaces between the consensus pallets, wired together by the runtime (design D6/D7 of
//! `m2-finality`). Keeping them here lets each pallet depend only on shared crates.

use alloc::vec::Vec;

use ac_crypto::PqPublicKey;

use crate::ac_bft::{Authority, SetId};
use crate::aura_pq::AuthorityError;
use crate::epoch::EpochIndex;

/// Read and replace the block-authoring (Aura-PQ) authority list.
pub trait BlockAuthorities {
    /// Current authorities, in slot-assignment order.
    fn authorities() -> Vec<PqPublicKey>;
    /// Validates and installs a new list; takes effect for the next block.
    ///
    /// # Errors
    ///
    /// [`AuthorityError`] if the list is empty, too large, has duplicates or non-ML-DSA-65 keys.
    fn set_authorities(authorities: Vec<PqPublicKey>) -> Result<(), AuthorityError>;
}

/// The author of the block being executed.
pub trait CurrentAuthor {
    /// Key of the authority whose slot this block is in; `None` without a slot digest.
    fn current_author() -> Option<PqPublicKey>;
}

/// The validator set as seen by the offences pallet.
pub trait ValidatorSetInterface {
    /// Current set id and members.
    fn current() -> (SetId, Vec<Authority>);
    /// Members of set `set_id` if it was active within the evidence retention window.
    fn historical(set_id: SetId) -> Option<Vec<Authority>>;
    /// Whether `key` belongs to a set active within the retention window.
    fn is_recent_member(key: &PqPublicKey) -> bool;
    /// Schedules `key` for removal at the next epoch boundary. Returns whether it is a member of
    /// the current set (otherwise nothing changes).
    fn disable(key: &PqPublicKey) -> bool;
    /// Index of the current epoch.
    fn current_epoch() -> EpochIndex;
}

sp_api::decl_runtime_apis! {
    /// Validator-set queries for the node and clients.
    pub trait ValidatorSetApi {
        /// Current set id and members.
        fn authority_set() -> (SetId, Vec<Authority>);
        /// Epoch length in blocks.
        fn epoch_length() -> u64;
        /// Members of a set still kept for evidence verification.
        fn historical_set(set_id: SetId) -> Option<Vec<Authority>>;
    }
}
