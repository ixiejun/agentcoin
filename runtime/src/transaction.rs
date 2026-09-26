//! Building `PqAuthorize` transactions (used by the wallet and by tests).
//!
//! `PqAuthorize` is the first transaction extension, so the SDK hands it the implication
//! `(extension version, call) ‖ explicit data of the later extensions ‖ their implicit data`.
//! The account signs `pallet_pq_accounts::signing_payload` of exactly those bytes.

use ac_crypto::{PqPublicKey, PqSignature};
use pallet_pq_accounts::{PqAuth, PqAuthorize};
use parity_scale_codec::Encode;
use sp_runtime::generic::{Era, ExtensionVersion};
use sp_runtime::traits::TransactionExtension;
use sp_runtime::transaction_validity::TransactionValidityError;

use crate::{
    AccountId, AuthorizedExtensions, Balance, Hash, Nonce, Runtime, RuntimeCall, UncheckedExtrinsic,
};

/// Transaction extension version used by AgentCoin transactions.
pub const EXTENSION_VERSION: ExtensionVersion = 0;

/// Implicit data of [`AuthorizedExtensions`]: what the chain adds to the signed payload.
pub type AuthorizedImplicit = <AuthorizedExtensions as TransactionExtension<RuntimeCall>>::Implicit;

/// Chain facts a signer needs besides the call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChainContext {
    /// Hash of block 0.
    pub genesis_hash: Hash,
    /// Runtime `spec_version`.
    pub spec_version: u32,
    /// Runtime `transaction_version`.
    pub transaction_version: u32,
}

/// Per-transaction parameters.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TxParams {
    /// Account nonce.
    pub nonce: Nonce,
    /// Tip for the block author (burned with the fee in M1).
    pub tip: Balance,
    /// Mortality; `Era::Immortal` never expires.
    pub era: Era,
    /// Hash of the block that starts a mortal era (the genesis hash for immortal ones).
    pub era_birth_hash: Hash,
}

/// The extensions after `PqAuthorize` for the given parameters.
#[must_use]
pub fn authorized_extensions(params: &TxParams) -> AuthorizedExtensions {
    (
        frame_system::CheckNonZeroSender::new(),
        frame_system::CheckSpecVersion::new(),
        frame_system::CheckTxVersion::new(),
        frame_system::CheckGenesis::new(),
        frame_system::CheckMortality::from(params.era),
        frame_system::CheckNonce::from(params.nonce),
        frame_system::CheckWeight::new(),
        pallet_transaction_payment::ChargeTransactionPayment::from(params.tip),
        frame_system::WeightReclaim::new(),
    )
}

/// Implicit data of [`authorized_extensions`], computed from chain facts instead of state.
#[must_use]
pub fn implicit_from(context: &ChainContext, params: &TxParams) -> AuthorizedImplicit {
    (
        (),
        context.spec_version,
        context.transaction_version,
        context.genesis_hash,
        params.era_birth_hash,
        (),
        (),
        (),
        (),
    )
}

/// Bytes whose `signing_payload` the account must sign.
#[must_use]
pub fn implication_bytes(
    call: &RuntimeCall,
    extensions: &AuthorizedExtensions,
    implicit: &AuthorizedImplicit,
) -> alloc::vec::Vec<u8> {
    let mut bytes = (EXTENSION_VERSION, call).encode();
    extensions.encode_to(&mut bytes);
    implicit.encode_to(&mut bytes);
    bytes
}

/// The 32-byte payload to sign with context `agentcoin/tx/v1`.
///
/// # Errors
///
/// Only if the built-in hashing context were malformed (never in practice).
pub fn payload(
    call: &RuntimeCall,
    extensions: &AuthorizedExtensions,
    implicit: &AuthorizedImplicit,
) -> Result<[u8; 32], ac_crypto::Error> {
    ac_crypto::hash::derive(
        pallet_pq_accounts::TX_PAYLOAD_CONTEXT,
        &implication_bytes(call, extensions, implicit),
    )
}

/// Assembles a signed v5 general transaction.
///
/// `public_key` must be `Some` exactly when the account has not registered a key yet.
#[must_use]
pub fn assemble(
    call: RuntimeCall,
    who: AccountId,
    signature: PqSignature,
    public_key: Option<PqPublicKey>,
    extensions: AuthorizedExtensions,
) -> UncheckedExtrinsic {
    let auth = PqAuthorize::<Runtime>::new(PqAuth::Signed {
        who,
        signature,
        public_key,
    });
    let (a, b, c, d, e, f, g, h, i) = extensions;
    UncheckedExtrinsic::new_transaction(call, (auth, a, b, c, d, e, f, g, h, i))
}

/// Implicit data read from the current chain state (tests and in-runtime tooling).
///
/// # Errors
///
/// Propagates the extensions' own implicit-data errors.
pub fn implicit_from_state(
    extensions: &AuthorizedExtensions,
) -> Result<AuthorizedImplicit, TransactionValidityError> {
    extensions.implicit()
}
