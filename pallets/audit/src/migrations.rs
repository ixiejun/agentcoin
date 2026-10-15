//! Storage migrations of the audit pallet.

/// Storage that versions 1 and earlier kept and version 2 removes.
// `storage_alias` generates undocumented prefix types.
#[allow(missing_docs)]
pub mod old {
    use crate::pallet::{Config, Pallet, VersionedSprt};
    use ac_primitives::market::audit::StatsConfig;
    use frame_support::pallet_prelude::OptionQuery;
    use frame_support::{Identity, storage_alias};
    use sp_runtime::AccountId32;

    /// The statistical judgment's global parameter version and switch (versions 1 and earlier).
    #[storage_alias]
    pub type StatsSettings<T: Config> = StorageValue<Pallet<T>, StatsConfig, OptionQuery>;

    /// Each provider's statistical state on all models together (version 1).
    #[storage_alias]
    pub type SprtStates<T: Config> =
        StorageMap<Pallet<T>, Identity, AccountId32, VersionedSprt, OptionQuery>;
}

/// Version 0 → 1 (m6-audit-sprt design D6, D7, D10): verdicts gain their statistics (`None`
/// for those stored before), disputes their kind (`Fail`), and the statistical judgment's
/// configuration starts at the runtime's latest parameter version, disabled.
///
/// The number of verdict lists is bounded by `MaxVerdictsPerRound × RetentionRounds`; disputes
/// are rare. The migration runs in one block, as no live chain stores audits yet.
pub mod v1 {
    use super::old::StatsSettings;
    use crate::pallet::{Config, Disputes, Pallet, VerdictList, Verdicts};
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

/// Version 1 → 2 (m6-audit-hardening design D1, D2, D5, D7): the global thresholds and
/// statistical versions become the default audit profile (bf16 models without their own), the
/// switch moves to `StatsEnabled`, the adjustable parameters trade the thresholds version for
/// the spot-check rate (the live 5%), the light penalty takes its live values, and the
/// statistical states, kept per provider until now, are emptied: they restart per model.
///
/// No live chain stores audits yet and the statistical judgment is off on live chains, so the
/// migration runs in one block.
pub mod v2 {
    use super::old::{SprtStates, StatsSettings};
    use crate::pallet::{
        Adjustable, Config, DefaultProfile, Pallet, Penalty, RetainedVerdicts, StatsEnabled,
    };
    use ac_primitives::market::MicroUsd;
    use ac_primitives::market::audit::{
        AdjustableParams, AuditProfile, CURRENT_STATS, PenaltyParams,
    };
    use alloc::vec::Vec;
    use frame_support::pallet_prelude::Weight;
    use frame_support::traits::{Get, UncheckedOnRuntimeUpgrade};
    use parity_scale_codec::{Decode, Encode};

    /// The adjustable parameters as version 1 stored them.
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Encode, Decode)]
    pub struct OldAdjustable {
        /// Auditor stake threshold.
        pub stake_usd: MicroUsd,
        /// Payment per verdict or winning vote.
        pub payment_usd: MicroUsd,
        /// The global thresholds version.
        pub thresholds_version: u16,
    }

    /// The live spot-check rate, in basis points.
    const LIVE_SPOT_CHECK_BPS: u16 = 500;

    /// The migration's body; run it through [`MigrateToV2`], which checks the storage version.
    pub struct InnerMigrateToV2<T>(core::marker::PhantomData<T>);

    impl<T: Config> UncheckedOnRuntimeUpgrade for InnerMigrateToV2<T> {
        fn on_runtime_upgrade() -> Weight {
            let settings = StatsSettings::<T>::take();
            let stats_version = settings.map_or(CURRENT_STATS.version, |c| c.version);
            let mut thresholds = None;
            let _ = Adjustable::<T>::translate::<OldAdjustable, _>(|old| {
                old.map(|o| {
                    thresholds = Some(o.thresholds_version);
                    AdjustableParams {
                        stake_usd: o.stake_usd,
                        payment_usd: o.payment_usd,
                        spot_check_bps: LIVE_SPOT_CHECK_BPS,
                    }
                })
            });
            if let Some(thresholds_version) = thresholds
                && !DefaultProfile::<T>::exists()
            {
                DefaultProfile::<T>::put(AuditProfile {
                    thresholds_version,
                    stats_version,
                });
            }
            StatsEnabled::<T>::put(settings.is_some_and(|c| c.enabled));
            if !Penalty::<T>::exists() {
                Penalty::<T>::put(PenaltyParams::LIVE);
            }
            // The per-provider states restart per model; the verdict lists only they kept go.
            let states = SprtStates::<T>::clear(u32::MAX, None).unique;
            let stale: Vec<_> = RetainedVerdicts::<T>::iter_keys()
                .filter(|(round, provider)| !Pallet::<T>::is_referenced(*round, provider))
                .collect();
            for (round, provider) in &stale {
                RetainedVerdicts::<T>::remove(round, provider);
            }
            let touched = u64::from(states)
                .saturating_add(u64::try_from(stale.len()).unwrap_or(u64::MAX))
                .saturating_add(5);
            T::DbWeight::get().reads_writes(touched, touched)
        }

        #[cfg(feature = "try-runtime")]
        fn pre_upgrade() -> Result<Vec<u8>, sp_runtime::TryRuntimeError> {
            Ok(Vec::new())
        }

        #[cfg(feature = "try-runtime")]
        fn post_upgrade(_state: Vec<u8>) -> Result<(), sp_runtime::TryRuntimeError> {
            frame_support::ensure!(!StatsSettings::<T>::exists(), "global settings left");
            frame_support::ensure!(Penalty::<T>::exists(), "no penalty parameters");
            frame_support::ensure!(
                SprtStates::<T>::iter_keys().next().is_none(),
                "per-provider states left"
            );
            Ok(())
        }
    }

    /// Version 1 → 2, run only on storage version 1.
    pub type MigrateToV2<T> = frame_support::migrations::VersionedMigration<
        1,
        2,
        InnerMigrateToV2<T>,
        Pallet<T>,
        <T as frame_system::Config>::DbWeight,
    >;
}
