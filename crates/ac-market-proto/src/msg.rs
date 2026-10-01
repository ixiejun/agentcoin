//! Messages carried inside sealed channels (SCALE, prefixed with [`PROTOCOL_VERSION`]).
//!
//! Enum indices are wire format: they are spelled out and never change; new variants take new
//! indices.

use ac_crypto::{PqPublicKey, PqSignature};
use ac_primitives::market::work::JobKind;
use ac_primitives::market::{MicroUsd, ModelId, ReceiptBody, SignedReceipt, SignedVoucher};
use parity_scale_codec::{Decode, Encode};
use sp_runtime::AccountId32;

use crate::Error;
use crate::toploc::ToplocProofs;

/// Version byte in front of every message.
pub const PROTOCOL_VERSION: u8 = 2;

/// How a request is paid. Index 1 is reserved for shielded vouchers (β, full plan §11 `Credit`).
#[allow(clippy::large_enum_variant)] // One voucher per message; boxing buys nothing here.
#[derive(Clone, Debug, PartialEq, Eq, Encode, Decode)]
pub enum Payment {
    /// A transparent cumulative channel voucher (D54).
    #[codec(index = 0)]
    Transparent(SignedVoucher),
}

/// Token usage of a completed request.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Encode, Decode)]
pub struct Usage {
    /// Prompt (input) tokens.
    pub prompt_tokens: u32,
    /// Generated (output) tokens.
    pub completion_tokens: u32,
}

/// Machine-readable error codes. Messages accompanying them never echo request content.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Encode, Decode)]
pub enum ErrorCode {
    /// The request is malformed.
    #[codec(index = 0)]
    BadRequest,
    /// No model matches the requested name or ID.
    #[codec(index = 1)]
    ModelNotFound,
    /// The name matches more than one model; use a model ID.
    #[codec(index = 2)]
    AmbiguousModel,
    /// No serviceable provider for the model.
    #[codec(index = 3)]
    NoProvider,
    /// Payment is missing, insufficient or not exactly the billed total.
    #[codec(index = 4)]
    PaymentRequired,
    /// Every candidate provider failed, or the serving provider failed mid-stream.
    #[codec(index = 5)]
    ProviderFailed,
    /// The sender is not an active gateway (provider side).
    #[codec(index = 6)]
    NotGateway,
    /// The job kind is not supported in this phase.
    #[codec(index = 7)]
    UnsupportedJob,
    /// The inference engine failed.
    #[codec(index = 8)]
    EngineFailed,
    /// An internal error.
    #[codec(index = 9)]
    Internal,
}

impl ErrorCode {
    /// The OpenAI-style `code` string.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::BadRequest => "invalid_request",
            Self::ModelNotFound => "model_not_found",
            Self::AmbiguousModel => "ambiguous_model",
            Self::NoProvider => "no_provider",
            Self::PaymentRequired => "payment_required",
            Self::ProviderFailed => "provider_failed",
            Self::NotGateway => "not_gateway",
            Self::UnsupportedJob => "unsupported_job",
            Self::EngineFailed => "engine_failed",
            Self::Internal => "internal_error",
        }
    }

    /// The HTTP status the local proxy answers with.
    #[must_use]
    pub const fn http_status(self) -> u16 {
        match self {
            Self::BadRequest | Self::AmbiguousModel | Self::UnsupportedJob => 400,
            Self::PaymentRequired => 402,
            Self::NotGateway => 403,
            Self::ModelNotFound => 404,
            Self::NoProvider | Self::ProviderFailed | Self::EngineFailed => 503,
            Self::Internal => 500,
        }
    }
}

/// User (wallet proxy) → gateway.
#[allow(clippy::large_enum_variant)] // One message per channel.
#[derive(Clone, Debug, PartialEq, Eq, Encode, Decode)]
pub enum UserMsg {
    /// An OpenAI Chat Completions request (JSON bytes) and the payment for everything billed so
    /// far on the channel.
    #[codec(index = 0)]
    Chat {
        /// Request JSON.
        request: Vec<u8>,
        /// Voucher covering exactly the channel's billed total.
        payment: Payment,
    },
    /// A payment on its own, sent after a bill.
    #[codec(index = 1)]
    Pay {
        /// Voucher covering exactly the channel's billed total.
        payment: Payment,
    },
    /// Like [`UserMsg::Chat`], for one named provider only: the gateway forwards it to that
    /// provider if it can serve the model, and never fails over to another (m6-auditor-agent
    /// design D1; open to every user).
    #[codec(index = 2)]
    ChatTo {
        /// The provider that must serve the request.
        provider: AccountId32,
        /// Request JSON.
        request: Vec<u8>,
        /// Voucher covering exactly the channel's billed total.
        payment: Payment,
    },
}

/// Gateway → user.
#[allow(clippy::large_enum_variant)] // Billing is sent once per request.
#[derive(Clone, Debug, PartialEq, Eq, Encode, Decode)]
pub enum GatewayMsg {
    /// One streamed Chat Completions chunk (JSON bytes).
    #[codec(index = 0)]
    Delta(Vec<u8>),
    /// A whole non-streamed Chat Completions response (JSON bytes).
    #[codec(index = 1)]
    Completion(Vec<u8>),
    /// The request's bill: the double-signed receipt, its fee and the channel's new total.
    #[codec(index = 2)]
    Billing {
        /// Receipt signed by provider and gateway.
        receipt: SignedReceipt,
        /// Fee of this request.
        fee: MicroUsd,
        /// The channel's billed total including this request.
        billed_total: MicroUsd,
        /// The TOPLOC proofs the receipt commits to; `None` for an all-zero commitment.
        toploc: Option<ToplocProofs>,
    },
    /// The request failed; nothing is billed.
    #[codec(index = 3)]
    Error {
        /// Error code.
        code: ErrorCode,
        /// Human-readable explanation (never request content).
        message: Vec<u8>,
    },
    /// A payment was accepted.
    #[codec(index = 4)]
    Paid {
        /// The channel's billed total the voucher covers.
        billed_total: MicroUsd,
    },
}

/// Gateway → provider.
#[allow(clippy::large_enum_variant)] // One message per channel.
#[derive(Clone, Debug, PartialEq, Eq, Encode, Decode)]
pub enum ProviderReq {
    /// Run a job.
    #[codec(index = 0)]
    Infer {
        /// Request ID chosen by the gateway (unique among its receipts).
        request_id: [u8; 32],
        /// Job kind; only inference in this phase.
        kind: JobKind,
        /// On-chain model.
        model: ModelId,
        /// Request JSON.
        request: Vec<u8>,
    },
    /// A receipt the gateway has co-signed, for the provider's records.
    #[codec(index = 1)]
    Cosigned(SignedReceipt),
}

/// Provider → gateway.
#[allow(clippy::large_enum_variant)] // The receipt is sent once per request.
#[derive(Clone, Debug, PartialEq, Eq, Encode, Decode)]
pub enum ProviderMsg {
    /// One streamed Chat Completions chunk (JSON bytes).
    #[codec(index = 0)]
    Delta(Vec<u8>),
    /// The receipt, signed by the provider, and the usage it charges for; the gateway adds its
    /// own signature.
    #[codec(index = 1)]
    Receipt {
        /// The receipt.
        body: ReceiptBody,
        /// The provider's account key.
        key: PqPublicKey,
        /// The provider's signature over the receipt payload.
        signature: PqSignature,
        /// Usage reported by the engine.
        usage: Usage,
        /// The TOPLOC proofs the receipt commits to; `None` for an all-zero commitment.
        toploc: Option<ToplocProofs>,
    },
    /// The job failed; no receipt.
    #[codec(index = 2)]
    Error {
        /// Error code.
        code: ErrorCode,
        /// Human-readable explanation (never request content).
        message: Vec<u8>,
    },
    /// Acknowledges [`ProviderReq::Cosigned`].
    #[codec(index = 3)]
    Ack,
}

/// Encodes a message with the version prefix.
#[must_use]
pub fn encode<T: Encode>(msg: &T) -> Vec<u8> {
    let mut out = Vec::with_capacity(msg.size_hint().saturating_add(1));
    out.push(PROTOCOL_VERSION);
    msg.encode_to(&mut out);
    out
}

/// Decodes a versioned message; unknown versions and trailing bytes are rejected.
///
/// # Errors
///
/// [`Error::UnsupportedVersion`] or [`Error::Decode`].
pub fn decode<T: Decode>(bytes: &[u8]) -> Result<T, Error> {
    let (version, mut rest) = bytes.split_first().ok_or(Error::Decode)?;
    if *version != PROTOCOL_VERSION {
        return Err(Error::UnsupportedVersion(*version));
    }
    let msg = T::decode(&mut rest).map_err(|_| Error::Decode)?;
    if !rest.is_empty() {
        return Err(Error::Decode);
    }
    Ok(msg)
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn wire_indices_are_fixed() {
        assert_eq!(encode(&GatewayMsg::Delta(vec![7])), [2, 0, 4, 7]);
        assert_eq!(
            encode(&GatewayMsg::Paid {
                billed_total: MicroUsd(5)
            })[..2],
            [2, 4]
        );
        assert_eq!(encode(&ProviderMsg::Ack), [2, 3]);
        assert_eq!(
            encode(&ProviderMsg::Error {
                code: ErrorCode::EngineFailed,
                message: vec![]
            }),
            [2, 2, 8, 0]
        );
        assert_eq!(ErrorCode::Internal.encode(), [9]);
        assert_eq!(
            encode(&ProviderReq::Infer {
                request_id: [0; 32],
                kind: JobKind::Inference,
                model: ModelId([0; 32]),
                request: vec![],
            })[..2],
            [2, 0]
        );
    }

    #[test]
    fn unknown_versions_and_trailing_bytes_are_rejected() {
        // Version 1 carried receipts without TOPLOC proofs; it is no longer understood.
        let mut bytes = encode(&ProviderMsg::Ack);
        bytes[0] = 1;
        assert_eq!(
            decode::<ProviderMsg>(&bytes).unwrap_err(),
            Error::UnsupportedVersion(1)
        );
        bytes[0] = 3;
        assert_eq!(
            decode::<ProviderMsg>(&bytes).unwrap_err(),
            Error::UnsupportedVersion(3)
        );
        let mut bytes = encode(&ProviderMsg::Ack);
        bytes.push(0);
        assert_eq!(decode::<ProviderMsg>(&bytes).unwrap_err(), Error::Decode);
        assert_eq!(decode::<ProviderMsg>(&[]).unwrap_err(), Error::Decode);
    }

    proptest! {
        #[test]
        fn messages_round_trip(data in proptest::collection::vec(any::<u8>(), 0..512), n in any::<u128>()) {
            for m in [
                GatewayMsg::Delta(data.clone()),
                GatewayMsg::Completion(data.clone()),
                GatewayMsg::Error { code: ErrorCode::NoProvider, message: data.clone() },
                GatewayMsg::Paid { billed_total: MicroUsd(n) },
            ] {
                prop_assert_eq!(decode::<GatewayMsg>(&encode(&m)).unwrap(), m);
            }
            let m = ProviderMsg::Delta(data.clone());
            prop_assert_eq!(decode::<ProviderMsg>(&encode(&m)).unwrap(), m);
            let u = Usage { prompt_tokens: u32::try_from(n % 1_000_000).unwrap(), completion_tokens: 3 };
            prop_assert_eq!(decode::<Usage>(&encode(&u)).unwrap(), u);
        }
    }
}
