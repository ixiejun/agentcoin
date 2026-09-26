//! `PqAuthorize`: ML-DSA authorization of v5 `General` transactions (decision D36).

use core::marker::PhantomData;

use ac_crypto::{PqPublicKey, PqSignature, SigAlg};
use frame_support::pallet_prelude::TransactionSource;
use frame_support::traits::OriginTrait;
use parity_scale_codec::{Decode, DecodeWithMemTracking, Encode};
use scale_info::TypeInfo;
use sp_runtime::AccountId32;
use sp_runtime::traits::{
    AsTransactionAuthorizedOrigin, DispatchInfoOf, Dispatchable, PostDispatchInfoOf,
    TransactionExtension, ValidateResult,
};
use sp_runtime::transaction_validity::{
    InvalidTransaction, TransactionValidityError, ValidTransaction,
};
use sp_runtime::{DispatchResult, Weight};

use crate::{
    Config, KeyOwner, Keys, Pallet, TX_SIGNING_CONTEXT, WeightInfo, derived_account,
    key_fingerprint, signing_payload,
};

/// Authorization data carried by a transaction.
#[derive(Clone, PartialEq, Eq, Debug, Encode, Decode, DecodeWithMemTracking, TypeInfo)]
pub enum PqAuth {
    /// No authorization: the transaction gets no account origin, so calls that need one fail.
    #[codec(index = 0)]
    None,
    /// Signed by `who`.
    #[codec(index = 1)]
    Signed {
        /// The signing account.
        who: AccountId32,
        /// ML-DSA signature over [`crate::signing_payload`].
        signature: PqSignature,
        /// The account's first public key; present only while the account is unregistered.
        public_key: Option<PqPublicKey>,
    },
}

/// The transaction extension. It must come first in the extension pipeline so that its
/// signature covers the call and every other extension.
#[derive(Clone, PartialEq, Eq, Encode, Decode, DecodeWithMemTracking, TypeInfo)]
#[scale_info(skip_type_params(T))]
pub struct PqAuthorize<T>(pub PqAuth, PhantomData<T>);

impl<T> PqAuthorize<T> {
    /// Wraps authorization data.
    #[must_use]
    pub const fn new(auth: PqAuth) -> Self {
        Self(auth, PhantomData)
    }
}

impl<T> core::fmt::Debug for PqAuthorize<T> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match &self.0 {
            PqAuth::None => f.write_str("PqAuthorize(None)"),
            PqAuth::Signed {
                who, public_key, ..
            } => f
                .debug_struct("PqAuthorize")
                .field("who", who)
                .field("registers_key", &public_key.is_some())
                .finish(),
        }
    }
}

/// Key to register once the transaction is applied.
pub type PendingRegistration = Option<(AccountId32, PqPublicKey)>;

impl<T: Config + Send + Sync> TransactionExtension<T::RuntimeCall> for PqAuthorize<T>
where
    <T::RuntimeCall as Dispatchable>::RuntimeOrigin: AsTransactionAuthorizedOrigin,
{
    const IDENTIFIER: &'static str = "PqAuthorize";
    type Implicit = ();
    type Val = PendingRegistration;
    type Pre = ();

    fn weight(&self, _call: &T::RuntimeCall) -> Weight {
        match &self.0 {
            PqAuth::None => Weight::zero(),
            PqAuth::Signed {
                signature,
                public_key,
                ..
            } => {
                let verify = match signature.alg() {
                    SigAlg::MlDsa44 => T::WeightInfo::authorize_ml_dsa_44(),
                    SigAlg::MlDsa65 => T::WeightInfo::authorize_ml_dsa_65(),
                    // ML-DSA-87 is the most expensive implemented algorithm; it also bounds
                    // any algorithm a future runtime adds before its own benchmark exists.
                    _ => T::WeightInfo::authorize_ml_dsa_87(),
                };
                if public_key.is_some() {
                    verify.saturating_add(T::WeightInfo::register_key())
                } else {
                    verify
                }
            }
        }
    }

    fn validate(
        &self,
        mut origin: <T::RuntimeCall as Dispatchable>::RuntimeOrigin,
        _call: &T::RuntimeCall,
        _info: &DispatchInfoOf<T::RuntimeCall>,
        _len: usize,
        _self_implicit: (),
        inherited_implication: &impl Encode,
        _source: TransactionSource,
    ) -> ValidateResult<Self::Val, T::RuntimeCall> {
        let PqAuth::Signed {
            who,
            signature,
            public_key,
        } = &self.0
        else {
            return Ok((ValidTransaction::default(), None, origin));
        };
        // This extension heads the authorization pipeline; nothing before it may authorize.
        if origin.is_transaction_authorized() {
            return Err(InvalidTransaction::BadSigner.into());
        }

        let (key, pending) = match (public_key, Keys::<T>::get(who)) {
            // First transaction: the key must derive the account and be unused.
            (Some(key), None) => {
                if derived_account(key) != *who || KeyOwner::<T>::contains_key(key_fingerprint(key))
                {
                    return Err(InvalidTransaction::BadSigner.into());
                }
                (key.clone(), Some((who.clone(), key.clone())))
            }
            (None, Some(record)) => (record.public_key, None),
            // Registered accounts must not carry a key; unregistered ones must.
            (Some(_), Some(_)) | (None, None) => {
                return Err(InvalidTransaction::BadSigner.into());
            }
        };

        let payload = signing_payload(inherited_implication)
            .map_err(|_| TransactionValidityError::from(InvalidTransaction::BadProof))?;
        ac_crypto::sig::verify(&key, &payload, TX_SIGNING_CONTEXT, signature)
            .map_err(|_| TransactionValidityError::from(InvalidTransaction::BadProof))?;

        origin.set_caller_from_signed(who.clone());
        Ok((ValidTransaction::default(), pending, origin))
    }

    fn prepare(
        self,
        val: Self::Val,
        _origin: &<T::RuntimeCall as Dispatchable>::RuntimeOrigin,
        _call: &T::RuntimeCall,
        _info: &DispatchInfoOf<T::RuntimeCall>,
        _len: usize,
    ) -> Result<Self::Pre, TransactionValidityError> {
        // Registration is written only when the transaction is actually applied; pool
        // validation never changes state.
        if let Some((who, key)) = val {
            Pallet::<T>::register(&who, key);
        }
        Ok(())
    }

    fn post_dispatch_details(
        _pre: Self::Pre,
        _info: &DispatchInfoOf<T::RuntimeCall>,
        _post_info: &PostDispatchInfoOf<T::RuntimeCall>,
        _len: usize,
        _result: &DispatchResult,
    ) -> Result<Weight, TransactionValidityError> {
        Ok(Weight::zero())
    }
}
