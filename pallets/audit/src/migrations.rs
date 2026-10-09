//! Storage migrations of the audit pallet.

/// Version 0 → 1 (m6-audit-sprt design D6, D7, D10): verdicts gain their statistics (`None`
/// for those stored before), disputes their kind (`Fail`), and the statistical judgment's
/// configuration starts at the runtime's latest parameter version, disabled.
///
/// The number of verdict lists is bounded by `MaxVerdictsPerRound × RetentionRounds`; disputes
/// are rare. The migration runs in one block, as no live chain stores audits yet.
pub mod v1 {
    use crate::pallet::{Config, Disputes, Pallet, StatsSettings, VerdictList, Verdicts};
    use ac_primitives::market::audit::{
        Accuser, CURRENT_STATS, DisputeKind, DisputeOutcome, DisputeRecord, MAX_ACCUSERS,
        MAX_REVIEWERS, RoundIndex, StatsConfig, VerdictOutcome, VerdictRecord, Vote,
    };
    use alloc::vec::Vec;
    use frame_support::BoundedVec;
    use frame_support::pallet_prelude::{ConstU32, Weight};
    use frame_support::traits::{Get, UncheckedOnRuntimeUpgrade};
    use frame_system::pallet_prelude::BlockNumberFor;
    use parity_scale_codec::{Decode, Encode};
    use sp_core::H256;
    use sp_runtime::AccountId32;

    /// A verdict as version 0 stored it.
    #[derive(Clone, Debug, PartialEq, Eq, Encode, Decode)]
    pub struct OldVerdictRecord {
        /// Who submitted it.
        pub auditor: AccountId32,
        /// The outcome.
        pub outcome: VerdictOutcome,
        /// Thresholds version used.
        pub thresholds_version: u16,
        /// Commitment to the evidence.
        pub evidence: Option<[u8; 32]>,
        /// BLAKE3 of the SCALE-encoded signed receipt.
        pub receipt_hash: H256,
        /// The receipt's request ID.
        pub request_id: [u8; 32],
        /// The receipt's gateway.
        pub gateway: AccountId32,
    }

    /// A dispute as version 0 stored it.
    #[derive(Clone, Debug, PartialEq, Eq, Encode, Decode)]
    pub struct OldDisputeRecord<BlockNumber> {
        /// The accused provider.
        pub provider: AccountId32,
        /// Round the dispute was opened in.
        pub round: RoundIndex,
        /// The accusers.
        pub accusers: BoundedVec<Accuser<AccountId32>, ConstU32<MAX_ACCUSERS>>,
        /// Reviewers and their votes.
        pub reviewers: BoundedVec<(AccountId32, Option<Vote>), ConstU32<{ MAX_REVIEWERS as u32 }>>,
        /// Last block votes are accepted in.
        pub deadline: BlockNumber,
        /// How it ended.
        pub outcome: Option<DisputeOutcome>,
        /// Block it was closed in.
        pub closed_at: Option<BlockNumber>,
    }

    /// The verdict list of version 0.
    pub type OldVerdictList = Vec<OldVerdictRecord>;

    fn verdict(old: OldVerdictRecord) -> VerdictRecord<AccountId32> {
        VerdictRecord {
            auditor: old.auditor,
            outcome: old.outcome,
            thresholds_version: old.thresholds_version,
            evidence: old.evidence,
            receipt_hash: old.receipt_hash,
            request_id: old.request_id,
            gateway: old.gateway,
            stats: None,
        }
    }

    fn dispute<B>(old: OldDisputeRecord<B>) -> DisputeRecord<AccountId32, B> {
        DisputeRecord {
            provider: old.provider,
            round: old.round,
            // MAX_ACCUSERS ≤ MAX_DISPUTE_ACCUSERS: nothing is cut.
            accusers: BoundedVec::truncate_from(old.accusers.into_inner()),
            reviewers: old.reviewers,
            deadline: old.deadline,
            outcome: old.outcome,
            closed_at: old.closed_at,
            kind: DisputeKind::Fail,
        }
    }

    /// The migration's body; run it through [`MigrateToV1`], which checks the storage version.
    pub struct InnerMigrateToV1<T>(core::marker::PhantomData<T>);

    impl<T: Config> UncheckedOnRuntimeUpgrade for InnerMigrateToV1<T> {
        fn on_runtime_upgrade() -> Weight {
            let mut entries = 0u64;
            Verdicts::<T>::translate::<OldVerdictList, _>(|_, _, old| {
                entries = entries.saturating_add(1);
                // Lists held at most MAX_ASSIGN verdicts and still do.
                Some(VerdictList::truncate_from(
                    old.into_iter().map(verdict).collect(),
                ))
            });
            Disputes::<T>::translate::<OldDisputeRecord<BlockNumberFor<T>>, _>(|_, old| {
                entries = entries.saturating_add(1);
                Some(dispute(old))
            });
            if !StatsSettings::<T>::exists() {
                StatsSettings::<T>::put(StatsConfig {
                    version: CURRENT_STATS.version,
                    enabled: false,
                });
            }
            T::DbWeight::get().reads_writes(entries.saturating_add(1), entries.saturating_add(1))
        }

        #[cfg(feature = "try-runtime")]
        fn pre_upgrade() -> Result<Vec<u8>, sp_runtime::TryRuntimeError> {
            let counts = (
                Verdicts::<T>::iter_keys().count() as u64,
                Disputes::<T>::iter_keys().count() as u64,
            );
            Ok(counts.encode())
        }

        #[cfg(feature = "try-runtime")]
        fn post_upgrade(state: Vec<u8>) -> Result<(), sp_runtime::TryRuntimeError> {
            let (verdicts, disputes) = <(u64, u64)>::decode(&mut &state[..])
                .map_err(|_| sp_runtime::TryRuntimeError::Other("bad pre-upgrade state"))?;
            frame_support::ensure!(
                Verdicts::<T>::iter().count() as u64 == verdicts,
                "verdict lists lost"
            );
            frame_support::ensure!(
                Disputes::<T>::iter().count() as u64 == disputes,
                "disputes lost"
            );
            frame_support::ensure!(StatsSettings::<T>::exists(), "no statistics configuration");
            Ok(())
        }
    }

    /// Version 0 → 1, run only on storage version 0.
    pub type MigrateToV1<T> = frame_support::migrations::VersionedMigration<
        0,
        1,
        InnerMigrateToV1<T>,
        Pallet<T>,
        <T as frame_system::Config>::DbWeight,
    >;
}
