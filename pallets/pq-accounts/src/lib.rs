//! # Post-quantum accounts
//!
//! - A public-key registry: every account that has sent a transaction has exactly one current
//!   ML-DSA public key (with its AlgId) and a rotation counter.
//! - [`PqAuthorize`]: the transaction extension that authorizes v5 `General` transactions with
//!   ML-DSA signatures. An account's first transaction carries its public key and registers it;
//!   later transactions carry only the account ID and the signature (decision D36).
//! - `rotate_key`: replaces the current key — possibly with another algorithm — while the
//!   account ID stays the same; the new key must prove possession.
//!
//! The account ID of an account is always derived from its **first** public key
//! (`crypto/hashing`), so funds can be sent to an address before the account ever transacts.

#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

// Deliberate glob re-export (G.MOD.03 deviation): `construct_runtime` resolves hidden items that
// the `#[pallet]` macro generates inside `pallet`, so they cannot be listed explicitly.
pub use pallet::*;

/// Runs the README example as a doctest.
#[cfg(doctest)]
#[doc = include_str!("../README.md")]
pub struct ReadmeDoctests;

#[cfg(feature = "runtime-benchmarks")]
mod benchmarking;
mod extension;
#[cfg(test)]
mod mock;
#[cfg(test)]
mod tests;
pub mod weights;

pub use extension::{PqAuth, PqAuthorize};
pub use weights::WeightInfo;

use ac_crypto::PqPublicKey;
use parity_scale_codec::{Decode, DecodeWithMemTracking, Encode, MaxEncodedLen};
use scale_info::TypeInfo;
use sp_runtime::AccountId32;

/// Hashing context of the 32-byte transaction signing payload. Never change it.
pub const TX_PAYLOAD_CONTEXT: &str = "agentcoin 2026-09 tx-payload v1";
/// ML-DSA signing context of transactions. Never change it.
pub const TX_SIGNING_CONTEXT: &[u8] = b"agentcoin/tx/v1";
/// ML-DSA signing context of key-rotation proofs of possession. Never change it.
pub const KEY_ROTATION_CONTEXT: &[u8] = b"agentcoin/key-rotation/v1";
/// Domain tag at the start of every key-rotation statement.
pub const ROTATION_STATEMENT_TAG: &[u8] = b"agentcoin/key-rotation-statement";

/// The registered key of an account.
#[derive(
    Clone, PartialEq, Eq, Debug, Encode, Decode, DecodeWithMemTracking, MaxEncodedLen, TypeInfo,
)]
pub struct KeyRecord {
    /// Current public key; transactions of the account must verify under it.
    pub public_key: PqPublicKey,
    /// Number of completed rotations.
    pub rotations: u32,
}

/// The 32-byte payload that an account signs (context [`TX_SIGNING_CONTEXT`]) to authorize a
/// transaction: `derive_key(TX_PAYLOAD_CONTEXT, SCALE(inherited implication))`.
///
/// The inherited implication is what the SDK passes to the first transaction extension: the
/// extension version, the call, and the explicit and implicit data of every later extension
/// (spec version, transaction version, genesis hash, mortality, nonce, …).
///
/// # Errors
///
/// Only if the built-in context were malformed (never in practice).
pub fn signing_payload(implication: &impl Encode) -> Result<[u8; 32], ac_crypto::Error> {
    implication.using_encoded(|bytes| ac_crypto::hash::derive(TX_PAYLOAD_CONTEXT, bytes))
}

/// The statement that a new key signs (context [`KEY_ROTATION_CONTEXT`]) to prove possession
/// during `rotate_key`: SCALE of (tag, genesis hash, account, current rotation count, new key).
#[must_use]
pub fn rotation_statement<H: Encode>(
    genesis_hash: &H,
    who: &AccountId32,
    rotations: u32,
    new_key: &PqPublicKey,
) -> alloc::vec::Vec<u8> {
    (
        ROTATION_STATEMENT_TAG,
        genesis_hash,
        who,
        rotations,
        new_key,
    )
        .encode()
}

/// Index key of the reverse map from public keys to accounts.
#[must_use]
pub fn key_fingerprint(public_key: &PqPublicKey) -> [u8; 32] {
    ac_crypto::hash::blake3_256(&public_key.to_canonical())
}

/// The account ID a public key derives (only meaningful for an account's first key).
#[must_use]
pub fn derived_account(public_key: &PqPublicKey) -> AccountId32 {
    ac_primitives::to_runtime_account(&ac_crypto::account_id(public_key))
}

// FRAME's `pallet` macro expands to code (call and error metadata) that uses `expect` /
// `unreachable!` on our spans; hand-written code here uses neither.
#[allow(clippy::expect_used, clippy::unreachable)]
#[frame_support::pallet]
pub mod pallet {
    use super::{
        KEY_ROTATION_CONTEXT, KeyRecord, WeightInfo, derived_account, key_fingerprint,
        rotation_statement,
    };
    use ac_crypto::{PqPublicKey, PqSignature};
    use frame_support::pallet_prelude::{
        DispatchResult, IsType, OptionQuery, StorageMap, ValueQuery, ensure,
    };
    use frame_support::{Identity, pallet_prelude::StorageValue};
    use frame_system::pallet_prelude::{BlockNumberFor, OriginFor, ensure_signed};
    use sp_runtime::{AccountId32, traits::Zero};

    #[pallet::pallet]
    pub struct Pallet<T>(_);

    #[pallet::config]
    pub trait Config: frame_system::Config<AccountId = AccountId32> {
        /// The overarching event type.
        #[allow(deprecated)] // Kept explicit for compatibility with the SDK's test tooling.
        type RuntimeEvent: From<Event<Self>> + IsType<<Self as frame_system::Config>::RuntimeEvent>;
        /// Benchmarked weights.
        type WeightInfo: WeightInfo;
    }

    /// Current key of every registered account. Keyed by the account ID itself: it is already
    /// a BLAKE3 output (decision D35).
    #[pallet::storage]
    pub type Keys<T: Config> = StorageMap<_, Identity, AccountId32, KeyRecord, OptionQuery>;

    /// Reverse index: BLAKE3 of a current public key → the account using it.
    #[pallet::storage]
    pub type KeyOwner<T: Config> = StorageMap<_, Identity, [u8; 32], AccountId32, OptionQuery>;

    /// Total number of registered accounts (informational).
    #[pallet::storage]
    pub type RegisteredCount<T: Config> = StorageValue<_, u64, ValueQuery>;

    #[pallet::event]
    #[pallet::generate_deposit(pub(super) fn deposit_event)]
    pub enum Event<T: Config> {
        /// An account's first transaction registered its public key.
        KeyRegistered {
            /// The account.
            who: AccountId32,
            /// AlgId of the registered key.
            alg: u8,
        },
        /// An account rotated its key; the account ID is unchanged.
        KeyRotated {
            /// The account.
            who: AccountId32,
            /// AlgId of the new key.
            alg: u8,
            /// Rotation count after this rotation.
            rotations: u32,
        },
    }

    #[pallet::error]
    pub enum Error<T> {
        /// The account has no registered key.
        NotRegistered,
        /// The new key is already the current key of an account, or derives an account ID
        /// that already exists.
        KeyInUse,
        /// The proof of possession does not verify under the new key.
        BadProof,
        /// The rotation counter would overflow.
        Overflow,
    }

    #[pallet::call]
    impl<T: Config> Pallet<T> {
        /// Replaces the caller's current key with `new_key`.
        ///
        /// Authorized by the current key through `PqAuthorize`; `proof` is the new key's
        /// signature over [`rotation_statement`] with context `agentcoin/key-rotation/v1`.
        #[pallet::call_index(0)]
        #[pallet::weight(T::WeightInfo::rotate_key())]
        pub fn rotate_key(
            origin: OriginFor<T>,
            new_key: PqPublicKey,
            proof: PqSignature,
        ) -> DispatchResult {
            let who = ensure_signed(origin)?;
            let record = Keys::<T>::get(&who).ok_or(Error::<T>::NotRegistered)?;
            let fingerprint = key_fingerprint(&new_key);
            ensure!(
                !KeyOwner::<T>::contains_key(fingerprint),
                Error::<T>::KeyInUse
            );
            let derived = derived_account(&new_key);
            ensure!(
                derived == who
                    || (!Keys::<T>::contains_key(&derived)
                        && !frame_system::Pallet::<T>::account_exists(&derived)),
                Error::<T>::KeyInUse
            );
            let genesis = frame_system::Pallet::<T>::block_hash(BlockNumberFor::<T>::zero());
            let statement = rotation_statement(&genesis, &who, record.rotations, &new_key);
            ac_crypto::sig::verify(&new_key, &statement, KEY_ROTATION_CONTEXT, &proof)
                .map_err(|_| Error::<T>::BadProof)?;
            let rotations = record
                .rotations
                .checked_add(1)
                .ok_or(Error::<T>::Overflow)?;

            KeyOwner::<T>::remove(key_fingerprint(&record.public_key));
            KeyOwner::<T>::insert(fingerprint, &who);
            let alg = new_key.alg().id();
            Keys::<T>::insert(
                &who,
                KeyRecord {
                    public_key: new_key,
                    rotations,
                },
            );
            Self::deposit_event(Event::KeyRotated {
                who,
                alg,
                rotations,
            });
            Ok(())
        }
    }

    impl<T: Config> Pallet<T> {
        /// Current key and rotation count of `who`, if registered.
        pub fn current_key(who: &AccountId32) -> Option<(PqPublicKey, u32)> {
            Keys::<T>::get(who).map(|r| (r.public_key, r.rotations))
        }

        /// Registers `public_key` as the first key of `who`. Called by `PqAuthorize` when the
        /// transaction that carries the key is applied.
        pub(crate) fn register(who: &AccountId32, public_key: PqPublicKey) {
            let alg = public_key.alg().id();
            KeyOwner::<T>::insert(key_fingerprint(&public_key), who);
            Keys::<T>::insert(
                who,
                KeyRecord {
                    public_key,
                    rotations: 0,
                },
            );
            RegisteredCount::<T>::mutate(|n| *n = n.saturating_add(1));
            Self::deposit_event(Event::KeyRegistered {
                who: who.clone(),
                alg,
            });
        }
    }
}

sp_api::decl_runtime_apis! {
    /// Queries of the public-key registry, used by wallets.
    pub trait PqAccountsApi {
        /// Current public key and rotation count of `who`, or `None` if it never transacted.
        fn current_key(who: AccountId32) -> Option<(PqPublicKey, u32)>;
    }
}
