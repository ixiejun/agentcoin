//! Storage migrations of the work settlement pallet.

/// Version 0 → 1 (m6-audit-hardening design D4): the recent revenue starts empty for every
/// provider (`RecentSettled`), and the window is written with the live value, so it no longer
/// rests on the storage default. Nothing older is read or rewritten.
pub mod v1 {
    use crate::LIVE_REVENUE_WINDOW;
    use crate::pallet::{Config, Pallet, RevenueWindow};
    use frame_support::pallet_prelude::Weight;
    use frame_support::traits::{Get, UncheckedOnRuntimeUpgrade};

    /// The migration's body; run it through [`MigrateToV1`], which checks the storage version.
    pub struct InnerMigrateToV1<T>(core::marker::PhantomData<T>);

    impl<T: Config> UncheckedOnRuntimeUpgrade for InnerMigrateToV1<T> {
        fn on_runtime_upgrade() -> Weight {
            let exists = RevenueWindow::<T>::exists();
            if !exists {
                RevenueWindow::<T>::put(LIVE_REVENUE_WINDOW);
            }
            T::DbWeight::get().reads_writes(1, u64::from(!exists))
        }
    }

    /// Version 0 → 1, run once.
    pub type MigrateToV1<T> = frame_support::migrations::VersionedMigration<
        0,
        1,
        InnerMigrateToV1<T>,
        Pallet<T>,
        <T as frame_system::Config>::DbWeight,
    >;
}
