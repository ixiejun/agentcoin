#![doc = include_str!("../README.md")]
#![cfg_attr(not(feature = "std"), no_std)]
#![forbid(unsafe_code)]

extern crate alloc;

#[cfg(feature = "runtime-benchmarks")]
mod benchmarking;
mod currency;
mod extension;
pub mod precompiles;
pub mod weights;

#[cfg(test)]
mod mock;
#[cfg(test)]
mod precompile_tests;
#[cfg(test)]
mod tests;

pub use currency::ReviveCurrency;
pub use extension::SetEvmPayer;
// Deliberate glob re-export (G.MOD.03 deviation): `construct_runtime` resolves hidden items that
// the `#[pallet]` macro generates inside `pallet`, so they cannot be listed explicitly.
pub use pallet::*;
pub use pallet_revive;
pub use precompiles::PqPrecompiles;
pub use weights::WeightInfo;

use core::marker::PhantomData;
use frame_support::traits::Get;

// FRAME's `pallet` macro expands to code (genesis defaults, storage metadata) that uses
// `expect` / `unreachable!` on our spans; hand-written code here uses neither.
#[allow(clippy::expect_used, clippy::unreachable)]
#[frame_support::pallet]
pub mod pallet {
    use frame_support::{
        pallet_prelude::{BuildGenesisConfig, Hooks, OptionQuery, StorageValue, Weight},
        traits::Get,
    };
    use frame_system::pallet_prelude::BlockNumberFor;

    /// Configuration of the EVM support pallet.
    #[pallet::config]
    pub trait Config: frame_system::Config + pallet_revive::Config {
        /// Weights of the PQ precompiles.
        type WeightInfo: crate::WeightInfo;
    }

    #[pallet::pallet]
    pub struct Pallet<T>(_);

    /// The account paying for amounts `pallet-revive` would otherwise mint (a new contract's
    /// existential deposit) while a contract transaction or a dry run executes. Set by
    /// [`crate::SetEvmPayer`] for the dispatch of a contract transaction and cleared right after
    /// it; cleared again at the end of every block.
    #[pallet::storage]
    #[pallet::whitelist_storage]
    pub type EvmPayer<T: Config> = StorageValue<_, T::AccountId, OptionQuery>;

    /// Genesis: creates `pallet-revive`'s own account without minting (see
    /// [`Pallet::ensure_revive_account`]).
    #[pallet::genesis_config]
    #[derive(frame_support::DefaultNoBound)]
    pub struct GenesisConfig<T: Config> {
        #[serde(skip)]
        _marker: core::marker::PhantomData<T>,
    }

    #[pallet::genesis_build]
    impl<T: Config> BuildGenesisConfig for GenesisConfig<T> {
        fn build(&self) {
            Pallet::<T>::ensure_revive_account();
        }
    }

    #[pallet::hooks]
    impl<T: Config> Hooks<BlockNumberFor<T>> for Pallet<T> {
        fn on_runtime_upgrade() -> Weight {
            // Chains that existed before this pallet (dev and local networks) get the account on
            // the upgrade that adds it.
            Pallet::<T>::ensure_revive_account();
            T::DbWeight::get().reads_writes(1, 1)
        }

        fn on_initialize(_: BlockNumberFor<T>) -> Weight {
            // Accounts for the unconditional clean-up in `on_finalize`.
            T::DbWeight::get().writes(1)
        }

        fn on_finalize(_: BlockNumberFor<T>) {
            EvmPayer::<T>::kill();
        }
    }
}

impl<T: Config> Pallet<T> {
    /// Makes `pallet-revive`'s own account exist by giving it a system provider reference.
    ///
    /// Revive holds code-upload deposits on its pallet account and expects that account to
    /// exist; upstream it mints an existential deposit into it at genesis, which AgentCoin cannot
    /// do (no premine, D9; [`ReviveCurrency`] refuses the mint). A provider reference makes the
    /// account exist with a zero balance, so no ATC is created.
    pub fn ensure_revive_account() {
        let account = pallet_revive::Pallet::<T>::account_id();
        if !frame_system::Pallet::<T>::account_exists(&account) {
            frame_system::Pallet::<T>::inc_providers(&account);
        }
    }

    /// The current payer, if a contract transaction or a dry run is executing.
    pub fn payer() -> Option<T::AccountId> {
        EvmPayer::<T>::get()
    }

    /// Runs `f` with `payer` as the payer, then clears it. Used by the dry-run runtime APIs,
    /// whose state changes are discarded anyway.
    pub fn with_payer<R>(payer: T::AccountId, f: impl FnOnce() -> R) -> R {
        EvmPayer::<T>::put(payer);
        let result = f();
        EvmPayer::<T>::kill();
        result
    }
}

/// [`Get`] adapter returning the current payer, for [`ReviveCurrency`].
pub struct CurrentPayer<T>(PhantomData<T>);

impl<T: Config> Get<Option<T::AccountId>> for CurrentPayer<T> {
    fn get() -> Option<T::AccountId> {
        Pallet::<T>::payer()
    }
}
