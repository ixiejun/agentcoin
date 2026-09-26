//! The signature slot of the SDK's legacy `Signed` extrinsic, closed on purpose.
//!
//! AgentCoin authorizes transactions only through the `PqAuthorize` transaction extension on
//! v5 `General` extrinsics (decision D36). The SDK's extrinsic type still needs a type for the
//! legacy `Signed` preamble; `NoClassicSignature` fills it with a type that has no values, so
//! any extrinsic claiming that preamble fails to decode and no classic signature scheme can ever
//! authorize an account.

use parity_scale_codec::{Decode, DecodeWithMemTracking, Encode, Error, Input, Output};
use scale_info::TypeInfo;
use sp_runtime::AccountId32;
use sp_runtime::traits::{IdentifyAccount, Lazy, Verify};

/// An uninhabited signature type: it can never be decoded, constructed or verified.
#[derive(Clone, Debug, PartialEq, Eq, TypeInfo)]
pub enum NoClassicSignature {}

impl Encode for NoClassicSignature {
    fn encode_to<T: Output + ?Sized>(&self, _dest: &mut T) {
        match *self {}
    }
}

impl Decode for NoClassicSignature {
    fn decode<I: Input>(_input: &mut I) -> Result<Self, Error> {
        Err("legacy signed extrinsics are not accepted; use PqAuthorize".into())
    }
}

impl DecodeWithMemTracking for NoClassicSignature {}

/// The signer type paired with [`NoClassicSignature`]; also uninhabited.
#[derive(Clone, Debug, PartialEq, Eq, TypeInfo)]
pub enum NoClassicSigner {}

impl IdentifyAccount for NoClassicSigner {
    type AccountId = AccountId32;

    fn into_account(self) -> AccountId32 {
        match self {}
    }
}

impl Verify for NoClassicSignature {
    type Signer = NoClassicSigner;

    fn verify<L: Lazy<[u8]>>(&self, _msg: L, _signer: &AccountId32) -> bool {
        match *self {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec::Vec;

    // Requirement "只接受 PQ 授权的交易": no byte string decodes to a classic signature.
    #[test]
    fn nothing_decodes() {
        let inputs: [Vec<u8>; 4] = [
            Vec::new(),
            alloc::vec![0u8; 65],
            alloc::vec![1u8; 64],
            (0u8..=255).collect(),
        ];
        for bytes in inputs {
            assert!(NoClassicSignature::decode(&mut &bytes[..]).is_err());
        }
    }

    #[test]
    fn type_info_has_no_variants() {
        let info = NoClassicSignature::type_info();
        match info.type_def {
            scale_info::TypeDef::Variant(v) => assert!(v.variants.is_empty()),
            other => panic!("unexpected type definition {other:?}"),
        }
    }
}
