//! [`SetEvmPayer`]: records the signer of a contract transaction as the payer for its dispatch.

use crate::{Config, EvmPayer};
use core::marker::PhantomData;
use frame_support::{
    DefaultNoBound,
    dispatch::DispatchInfo,
    pallet_prelude::{TransactionSource, Weight},
    traits::{Get, IsSubType, OriginTrait},
};
use parity_scale_codec::{Decode, DecodeWithMemTracking, Encode};
use scale_info::TypeInfo;
use sp_runtime::{
    DispatchResult,
    traits::{
        DispatchInfoOf, DispatchOriginOf, Dispatchable, Implication, PostDispatchInfoOf,
        TransactionExtension, ValidateResult,
    },
    transaction_validity::TransactionValidityError,
};

/// Transaction extension that makes the signer of a contract transaction (`Revive::call`,
/// `Revive::instantiate_with_code`, or either wrapped in `Revive::dispatch_as_fallback_account`)
/// the payer of the existential deposit of contracts it creates ([`crate::ReviveCurrency`]).
///
/// It carries no explicit or implicit data. The payer is written just before dispatch and
/// removed right after it, whatever the dispatch result; `on_finalize` removes it again.
#[derive(Encode, Decode, DecodeWithMemTracking, DefaultNoBound, Clone, Eq, PartialEq, TypeInfo)]
#[scale_info(skip_type_params(T))]
pub struct SetEvmPayer<T>(PhantomData<T>);

impl<T> SetEvmPayer<T> {
    /// Creates the extension.
    pub fn new() -> Self {
        Self(PhantomData)
    }
}

impl<T> core::fmt::Debug for SetEvmPayer<T> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("SetEvmPayer")
    }
}

/// Whether `call` is a contract transaction whose signer pays for new contract accounts.
pub fn is_contract_transaction<T>(call: &<T as frame_system::Config>::RuntimeCall) -> bool
where
    T: Config,
    <T as frame_system::Config>::RuntimeCall: IsSubType<pallet_revive::Call<T>>,
{
    match call.is_sub_type() {
        Some(pallet_revive::Call::call { .. })
        | Some(pallet_revive::Call::instantiate_with_code { .. }) => true,
        Some(pallet_revive::Call::dispatch_as_fallback_account { call }) => {
            // `call` is revive's `RuntimeCall`, the same type as the system one.
            let inner: &<T as frame_system::Config>::RuntimeCall =
                frame_support::traits::IsType::into_ref(call.as_ref());
            is_contract_transaction::<T>(inner)
        }
        _ => false,
    }
}

impl<T> TransactionExtension<<T as frame_system::Config>::RuntimeCall> for SetEvmPayer<T>
where
    T: Config + Send + Sync,
    <T as frame_system::Config>::RuntimeCall:
        IsSubType<pallet_revive::Call<T>> + Dispatchable<Info = DispatchInfo>,
{
    const IDENTIFIER: &'static str = "SetEvmPayer";
    type Implicit = ();
    type Val = Option<T::AccountId>;
    type Pre = bool;

    fn weight(&self, call: &<T as frame_system::Config>::RuntimeCall) -> Weight {
        if is_contract_transaction::<T>(call) {
            // One write before and one after the dispatch.
            T::DbWeight::get().writes(2)
        } else {
            Weight::zero()
        }
    }

    fn validate(
        &self,
        origin: DispatchOriginOf<<T as frame_system::Config>::RuntimeCall>,
        call: &<T as frame_system::Config>::RuntimeCall,
        _info: &DispatchInfoOf<<T as frame_system::Config>::RuntimeCall>,
        _len: usize,
        _self_implicit: Self::Implicit,
        _inherited_implication: &impl Implication,
        _source: TransactionSource,
    ) -> ValidateResult<Self::Val, <T as frame_system::Config>::RuntimeCall> {
        let payer = if is_contract_transaction::<T>(call) {
            origin.as_signer().cloned()
        } else {
            None
        };
        Ok((Default::default(), payer, origin))
    }

    fn prepare(
        self,
        val: Self::Val,
        _origin: &DispatchOriginOf<<T as frame_system::Config>::RuntimeCall>,
        _call: &<T as frame_system::Config>::RuntimeCall,
        _info: &DispatchInfoOf<<T as frame_system::Config>::RuntimeCall>,
        _len: usize,
    ) -> Result<Self::Pre, TransactionValidityError> {
        Ok(match val {
            Some(payer) => {
                EvmPayer::<T>::put(payer);
                true
            }
            None => false,
        })
    }

    fn post_dispatch_details(
        pre: Self::Pre,
        _info: &DispatchInfoOf<<T as frame_system::Config>::RuntimeCall>,
        _post_info: &PostDispatchInfoOf<<T as frame_system::Config>::RuntimeCall>,
        _len: usize,
        _result: &DispatchResult,
    ) -> Result<Weight, TransactionValidityError> {
        if pre {
            EvmPayer::<T>::kill();
        }
        Ok(Weight::zero())
    }
}
