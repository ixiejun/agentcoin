//! The AgentCoin runtime.
//!
//! Every integrity commitment (block hash, extrinsics root, state root) uses BLAKE3-256
//! (decision D35), and the legacy `Signed` extrinsic preamble is closed: its signature type
//! cannot be decoded (decision D36).

#![cfg_attr(not(feature = "std"), no_std)]
// `construct_runtime`-style macros generate items without docs and with patterns that the
// workspace lints reject; the lints still apply to hand-written code in the submodules.
#![allow(
    missing_docs,
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing
)]

#[cfg(feature = "std")]
include!(concat!(env!("OUT_DIR"), "/wasm_binary.rs"));

extern crate alloc;

/// Runs the README example as a doctest.
#[cfg(doctest)]
#[doc = include_str!("../README.md")]
pub struct ReadmeDoctests;

// `define_benchmarks!` produces macros used textually by `apis`, so this module comes first.
#[cfg(feature = "runtime-benchmarks")]
#[macro_use]
mod benchmarks;
mod apis;
mod configs;
pub mod genesis_config_presets;
pub mod transaction;

// The runtime macros expand to code that names `Vec` unqualified.
use alloc::vec::Vec;

use ac_primitives::{Blake3Hasher, NoClassicSignature};
use sp_runtime::generic;
#[cfg(feature = "std")]
use sp_version::NativeVersion;
use sp_version::RuntimeVersion;

pub use apis::RuntimeApi;
pub use frame_system::Call as SystemCall;
pub use pallet_balances::Call as BalancesCall;
pub use pallet_pq_accounts::Call as PqAccountsCall;
pub use pallet_timestamp::Call as TimestampCall;
#[cfg(feature = "std")]
pub use sp_runtime::BuildStorage;

/// Runtime version.
#[sp_version::runtime_version]
pub const VERSION: RuntimeVersion = RuntimeVersion {
    spec_name: alloc::borrow::Cow::Borrowed("agentcoin"),
    impl_name: alloc::borrow::Cow::Borrowed("agentcoin"),
    authoring_version: 1,
    spec_version: 1,
    impl_version: 1,
    apis: apis::RUNTIME_API_VERSIONS,
    // 2: `AuthorizeCall` joined the extension pipeline (m2-finality); encodings are unchanged.
    transaction_version: 2,
    system_version: 1,
};

/// Native version, used by the node to decide whether its native runtime matches.
#[cfg(feature = "std")]
pub fn native_version() -> NativeVersion {
    NativeVersion {
        runtime_version: VERSION,
        can_author_with: Default::default(),
    }
}

/// Target block time and slot length in milliseconds (mvp-technical-plan §8).
pub const MILLISECS_PER_BLOCK: u64 = 1000;

/// Account identifier: the 32-byte BLAKE3 account ID of `ac-crypto`.
pub type AccountId = sp_runtime::AccountId32;
/// Transaction counter of an account.
pub type Nonce = u32;
/// Block number.
pub type BlockNumber = u32;
/// Output of the chain hasher.
pub type Hash = sp_core::H256;
/// Block header: hashed with BLAKE3.
pub type Header = generic::Header<BlockNumber, Blake3Hasher>;
/// Block type.
pub type Block = generic::Block<Header, UncheckedExtrinsic>;

/// Balance in the smallest unit (1 ATC = 10^18).
pub type Balance = u128;

/// One ATC in the smallest unit (18 decimals, plan §8).
pub const ATC: Balance = 1_000_000_000_000_000_000;
/// Existential deposit: 0.001 ATC (design D8, revisited in M3).
pub const EXISTENTIAL_DEPOSIT: Balance = ATC / 1_000;

/// Transaction extensions after [`pallet_pq_accounts::PqAuthorize`], in pipeline order. The
/// signature covers the call and the explicit and implicit data of all of them.
///
/// `AuthorizeCall` lets a transaction without an account signature invoke calls that authorize
/// themselves (double-signing reports); it has no explicit or implicit data, so it changes
/// neither the encoding nor the signed payload of account transactions.
pub type AuthorizedExtensions = (
    frame_system::AuthorizeCall<Runtime>,
    frame_system::CheckNonZeroSender<Runtime>,
    frame_system::CheckSpecVersion<Runtime>,
    frame_system::CheckTxVersion<Runtime>,
    frame_system::CheckGenesis<Runtime>,
    frame_system::CheckMortality<Runtime>,
    frame_system::CheckNonce<Runtime>,
    frame_system::CheckWeight<Runtime>,
    pallet_transaction_payment::ChargeTransactionPayment<Runtime>,
    frame_system::WeightReclaim<Runtime>,
);

/// Transaction extensions, in pipeline order. `PqAuthorize` comes first so that its ML-DSA
/// signature covers everything after it (decision D36).
pub type TxExtension = (
    pallet_pq_accounts::PqAuthorize<Runtime>,
    frame_system::AuthorizeCall<Runtime>,
    frame_system::CheckNonZeroSender<Runtime>,
    frame_system::CheckSpecVersion<Runtime>,
    frame_system::CheckTxVersion<Runtime>,
    frame_system::CheckGenesis<Runtime>,
    frame_system::CheckMortality<Runtime>,
    frame_system::CheckNonce<Runtime>,
    frame_system::CheckWeight<Runtime>,
    pallet_transaction_payment::ChargeTransactionPayment<Runtime>,
    frame_system::WeightReclaim<Runtime>,
);

/// Extrinsic type. The `Signed` preamble uses [`NoClassicSignature`], so it never decodes;
/// accounts authorize v5 `General` transactions through `PqAuthorize`.
pub type UncheckedExtrinsic =
    generic::UncheckedExtrinsic<AccountId, RuntimeCall, NoClassicSignature, TxExtension>;

/// Executive: dispatches extrinsics to the pallets.
pub type Executive = frame_executive::Executive<
    Runtime,
    Block,
    frame_system::ChainContext<Runtime>,
    Runtime,
    AllPalletsWithSystem,
>;

/// Opaque types for the node, which does not need to understand extrinsics.
pub mod opaque {
    use super::{BlockNumber, generic};
    use ac_primitives::Blake3Hasher;

    pub use sp_runtime::OpaqueExtrinsic as UncheckedExtrinsic;
    /// Opaque block header.
    pub type Header = generic::Header<BlockNumber, Blake3Hasher>;
    /// Opaque block.
    pub type Block = generic::Block<Header, UncheckedExtrinsic>;
}

#[frame_support::runtime]
mod runtime {
    #[runtime::runtime]
    #[runtime::derive(
        RuntimeCall,
        RuntimeEvent,
        RuntimeError,
        RuntimeOrigin,
        RuntimeFreezeReason,
        RuntimeHoldReason,
        RuntimeSlashReason,
        RuntimeLockId,
        RuntimeTask,
        RuntimeViewFunction
    )]
    pub struct Runtime;

    #[runtime::pallet_index(0)]
    pub type System = frame_system;

    #[runtime::pallet_index(1)]
    pub type Timestamp = pallet_timestamp;

    #[runtime::pallet_index(2)]
    pub type AuraPq = pallet_aura_pq;

    // Pallet name and index are published: constitution layer 1 reads the well-known
    // `Balances::TotalIssuance` key, which must never be renamed (red line 3).
    #[runtime::pallet_index(3)]
    pub type Balances = pallet_balances;

    #[runtime::pallet_index(4)]
    pub type TransactionPayment = pallet_transaction_payment;

    #[runtime::pallet_index(5)]
    pub type PqAccounts = pallet_pq_accounts;

    #[runtime::pallet_index(6)]
    pub type ValidatorSet = pallet_validator_set;

    #[runtime::pallet_index(7)]
    pub type Offences = pallet_ac_offences;

    #[runtime::pallet_index(8)]
    pub type RandomnessCr = pallet_randomness_cr;
}
