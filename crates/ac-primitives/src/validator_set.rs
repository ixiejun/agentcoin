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

/// The author and slot of the block being executed.
pub trait CurrentAuthor {
    /// Key of the authority whose slot this block is in; `None` without a slot digest.
    fn current_author() -> Option<PqPublicKey>;
    /// Slot of the block being executed (0 without a slot digest).
    fn current_slot() -> u64;
}

/// Slashing of offenders. M2 validators hold no stake (decisions D9, D19), so the M2
/// implementation `()` slashes nothing; M3 connects stake and burns 100%.
///
/// Called for every recorded offence. An offender is recorded at most once per offence kind and
/// authority set; `prior` lists the kinds already recorded for it in the same set. Implementations
/// must slash according to the most severe kind among `prior` and `kind`, never adding up: only
/// the difference to what the earlier records already slashed may be taken.
pub trait SlashHandler {
    /// Slashes `offender` for an offence of `kind`, given the kinds already recorded for it in
    /// the same set, and returns the amount slashed (smallest ATC unit).
    fn on_offence(
        offender: &PqPublicKey,
        kind: crate::offences::OffenceKind,
        prior: &[crate::offences::OffenceKind],
    ) -> u128;
}

impl SlashHandler for () {
    fn on_offence(
        _offender: &PqPublicKey,
        _kind: crate::offences::OffenceKind,
        _prior: &[crate::offences::OffenceKind],
    ) -> u128 {
        0
    }
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
    /// Epoch length in blocks.
    fn epoch_length() -> u64;
    /// Validator phase. Only the validator-set pallet knows it; the default is the PoA phase
    /// that M2 chains and test mocks are in.
    fn phase() -> crate::staking::ChainPhase {
        crate::staking::ChainPhase::Poa
    }
}

/// Validators that missed their randomness reveal (spec chain/randomness "未揭示计数").
pub trait RevealTracker {
    /// Key identifiers (`ac_primitives::staking::validator_key_id`) of the validators whose
    /// commitment of the epoch before last was not revealed during the epoch that just ended,
    /// if that conclusion happened in the block being executed; empty otherwise.
    fn missed_now() -> Vec<[u8; 32]>;
}

/// No randomness pallet: nobody misses.
impl RevealTracker for () {
    fn missed_now() -> Vec<[u8; 32]> {
        Vec::new()
    }
}

/// The staking ledger as seen by the validator set (design D1, D5, D6 of `m3-pos`).
pub trait StakingInterface {
    /// All active stake, summed over the ledger exactly as the node sums it.
    fn total_active() -> u128;
    /// Total issuance.
    fn total_issuance() -> u128;
    /// Qualified candidates: self-stake at the minimum and not chilled.
    fn qualified_candidates() -> u32;
    /// Runs and records an election for `seats` validators; returns the winners in set order
    /// with their keys and backings. `preview` marks a buffer-period preview.
    fn elect(seats: u32, preview: bool) -> Vec<(PqPublicKey, u128)>;
    /// Worst-case weight of reading the switch inputs (a pass over the ledger).
    fn inputs_weight() -> sp_runtime::Weight {
        sp_runtime::Weight::zero()
    }
    /// Worst-case weight of one election for `seats` validators.
    fn election_weight(_seats: u32) -> sp_runtime::Weight {
        sp_runtime::Weight::zero()
    }
}

/// No staking (M2 chains and mocks): nothing is staked and nobody can be elected, so the
/// switch conditions are never met.
impl StakingInterface for () {
    fn total_active() -> u128 {
        0
    }
    fn total_issuance() -> u128 {
        0
    }
    fn qualified_candidates() -> u32 {
        0
    }
    fn elect(_seats: u32, _preview: bool) -> Vec<(PqPublicKey, u128)> {
        Vec::new()
    }
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
