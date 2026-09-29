//! Inference receipts (m5-work-settlement design D2; spec `market/work-settlement`, "收据").
//!
//! A provider and a gateway both sign one receipt per request. Receipts stay off chain with the
//! gateway until the challenge period ends; the chain only sees their Merkle root in a work
//! report. A receipt never holds the prompt, the output or the paying user (red line 6).
//!
//! [`check_receipt`] is the single implementation of the validity rules, used by wallets,
//! gateways and (from M6) auditors.

use parity_scale_codec::{Decode, DecodeWithMemTracking, Encode, MaxEncodedLen};
use scale_info::TypeInfo;
use sp_core::H256;
use sp_runtime::AccountId32;

use ac_crypto::{PqPublicKey, PqSignature};

use super::model::ModelId;
use super::usd::{MICRO_USD_PER_USD, MicroUsd, PricePerMTok};
use super::voucher::key_fingerprint;
use super::work::JobKind;

/// Signing context of receipts (registered in M0, in use from M5).
pub const RECEIPT_CONTEXT: &[u8] = b"agentcoin/receipt/v1";

/// Hashing context of the 32-byte receipt signing payload.
pub const RECEIPT_PAYLOAD_CONTEXT: &str = "agentcoin 2026-09 receipt-payload v1";

/// What one request cost and how it was served.
#[derive(
    Clone, Debug, PartialEq, Eq, Encode, Decode, DecodeWithMemTracking, MaxEncodedLen, TypeInfo,
)]
pub struct ReceiptBody {
    /// Genesis hash of the chain the receipt is settled on.
    pub genesis: H256,
    /// The gateway that accepted the request.
    pub gateway: AccountId32,
    /// The provider that served it.
    pub provider: AccountId32,
    /// Kind of work; only [`JobKind::Inference`] in M5.
    pub kind: JobKind,
    /// The model served.
    pub model: ModelId,
    /// Chosen by the gateway, unique among its receipts.
    pub request_id: [u8; 32],
    /// Prompt tokens.
    pub in_tokens: u32,
    /// Generated tokens.
    pub out_tokens: u32,
    /// Fee: [`fee_for`] of the provider's price for the model.
    pub fee: MicroUsd,
    /// TOPLOC commitment of the proofs of this request (`ac-toploc`).
    pub toploc_commit: [u8; 32],
    /// Milliseconds from the gateway accepting the request to the first token.
    pub ttft_ms: u32,
    /// Milliseconds from the gateway accepting the request to the last token.
    pub total_ms: u32,
}

impl ReceiptBody {
    /// The 32-byte message both parties sign:
    /// `derive("agentcoin 2026-09 receipt-payload v1", SCALE(body))`.
    ///
    /// # Errors
    ///
    /// Only if the published hashing context were rejected, which does not happen.
    pub fn payload(&self) -> Result<[u8; 32], ac_crypto::Error> {
        ac_crypto::hash::derive(RECEIPT_PAYLOAD_CONTEXT, &self.encode())
    }
}

/// A receipt signed by both parties. The public keys travel with it because the chain stores
/// only key fingerprints.
#[derive(
    Clone, Debug, PartialEq, Eq, Encode, Decode, DecodeWithMemTracking, MaxEncodedLen, TypeInfo,
)]
pub struct SignedReceipt {
    /// The receipt.
    pub body: ReceiptBody,
    /// The provider's account key.
    pub provider_key: PqPublicKey,
    /// The provider's signature over [`ReceiptBody::payload`] under [`RECEIPT_CONTEXT`].
    pub provider_sig: PqSignature,
    /// The gateway's account key.
    pub gateway_key: PqPublicKey,
    /// The gateway's signature over the same payload under the same context.
    pub gateway_sig: PqSignature,
}

/// Fee of a request: `ceil((in × price.input + out × price.output) / 10^6)` micro-dollars.
/// Rounding up makes every request that used tokens cost at least one micro-dollar.
///
/// Returns `None` on overflow, which cannot happen for `u32` token counts and prices below
/// 10^28 micro-dollars per million tokens.
#[must_use]
pub fn fee_for(price: &PricePerMTok, in_tokens: u32, out_tokens: u32) -> Option<MicroUsd> {
    let total = u128::from(in_tokens)
        .checked_mul(price.input.0)?
        .checked_add(u128::from(out_tokens).checked_mul(price.output.0)?)?;
    Some(MicroUsd(total.div_ceil(MICRO_USD_PER_USD)))
}

/// Why a receipt is invalid.
#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    Encode,
    Decode,
    DecodeWithMemTracking,
    MaxEncodedLen,
    TypeInfo,
)]
#[non_exhaustive]
pub enum ReceiptError {
    /// Signed for another chain.
    WrongGenesis,
    /// A job kind that cannot be settled yet.
    UnsupportedJobKind,
    /// The provider does not offer the model.
    ModelNotOffered,
    /// The fee does not follow the provider's price.
    FeeMismatch {
        /// The fee [`fee_for`] gives.
        expected: MicroUsd,
    },
    /// The fee computation overflows.
    Overflow,
    /// The provider key is not the provider's registered key.
    WrongProviderKey,
    /// The gateway key is not the gateway's registered key.
    WrongGatewayKey,
    /// The provider signature does not verify.
    BadProviderSignature,
    /// The gateway signature does not verify.
    BadGatewaySignature,
}

impl core::fmt::Display for ReceiptError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::WrongGenesis => f.write_str("the receipt was signed for another chain"),
            Self::UnsupportedJobKind => f.write_str("the receipt's job kind cannot be settled yet"),
            Self::ModelNotOffered => f.write_str("the provider does not offer the model"),
            Self::FeeMismatch { expected } => write!(
                f,
                "the fee does not follow the provider's price (expected {} micro-USD)",
                expected.0
            ),
            Self::Overflow => f.write_str("the fee computation overflows"),
            Self::WrongProviderKey => f.write_str("the provider key is not the registered one"),
            Self::WrongGatewayKey => f.write_str("the gateway key is not the registered one"),
            Self::BadProviderSignature => f.write_str("the provider signature does not verify"),
            Self::BadGatewaySignature => f.write_str("the gateway signature does not verify"),
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for ReceiptError {}

/// What a receipt is checked against: chain facts a checker reads from the chain.
#[derive(Clone, Copy, Debug)]
pub struct ReceiptContext<'a> {
    /// Genesis hash of this chain.
    pub genesis: &'a H256,
    /// Fingerprint of the provider's registered key.
    pub provider_key: &'a [u8; 32],
    /// Fingerprint of the gateway's registered key.
    pub gateway_key: &'a [u8; 32],
    /// The provider's price for the receipt's model, `None` if it does not offer it.
    pub price: Option<&'a PricePerMTok>,
}

/// Checks `receipt` against `ctx`.
///
/// # Errors
///
/// The first failing rule, in this order: genesis, job kind, model, fee, provider key, gateway
/// key, provider signature, gateway signature.
pub fn check_receipt(
    receipt: &SignedReceipt,
    ctx: &ReceiptContext<'_>,
) -> Result<(), ReceiptError> {
    let body = &receipt.body;
    if body.genesis != *ctx.genesis {
        return Err(ReceiptError::WrongGenesis);
    }
    if !body.kind.is_supported() {
        return Err(ReceiptError::UnsupportedJobKind);
    }
    let price = ctx.price.ok_or(ReceiptError::ModelNotOffered)?;
    let expected = fee_for(price, body.in_tokens, body.out_tokens).ok_or(ReceiptError::Overflow)?;
    if body.fee != expected {
        return Err(ReceiptError::FeeMismatch { expected });
    }
    if key_fingerprint(&receipt.provider_key) != *ctx.provider_key {
        return Err(ReceiptError::WrongProviderKey);
    }
    if key_fingerprint(&receipt.gateway_key) != *ctx.gateway_key {
        return Err(ReceiptError::WrongGatewayKey);
    }
    let payload = body
        .payload()
        .map_err(|_| ReceiptError::BadProviderSignature)?;
    ac_crypto::sig::verify(
        &receipt.provider_key,
        &payload,
        RECEIPT_CONTEXT,
        &receipt.provider_sig,
    )
    .map_err(|_| ReceiptError::BadProviderSignature)?;
    ac_crypto::sig::verify(
        &receipt.gateway_key,
        &payload,
        RECEIPT_CONTEXT,
        &receipt.gateway_sig,
    )
    .map_err(|_| ReceiptError::BadGatewaySignature)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ac_crypto::{SigAlg, sig::SigningKey};

    fn key(name: &str) -> SigningKey {
        SigningKey::from_seed(SigAlg::MlDsa44, &ac_crypto::dev_seed(name).unwrap()).unwrap()
    }

    fn price() -> PricePerMTok {
        PricePerMTok {
            input: MicroUsd(100_000),
            output: MicroUsd(300_000),
        }
    }

    fn body() -> ReceiptBody {
        ReceiptBody {
            genesis: H256::repeat_byte(0x11),
            gateway: AccountId32::new([0xbb; 32]),
            provider: AccountId32::new([0xcc; 32]),
            kind: JobKind::Inference,
            model: ModelId([0x42; 32]),
            request_id: [0x01; 32],
            in_tokens: 1_000,
            out_tokens: 2_000,
            fee: MicroUsd(700),
            toploc_commit: [0x77; 32],
            ttft_ms: 120,
            total_ms: 3_400,
        }
    }

    fn sign(body: ReceiptBody, provider: &SigningKey, gateway: &SigningKey) -> SignedReceipt {
        let payload = body.payload().unwrap();
        SignedReceipt {
            body,
            provider_key: provider.public_key().unwrap(),
            provider_sig: provider
                .sign_deterministic(&payload, RECEIPT_CONTEXT)
                .unwrap(),
            gateway_key: gateway.public_key().unwrap(),
            gateway_sig: gateway
                .sign_deterministic(&payload, RECEIPT_CONTEXT)
                .unwrap(),
        }
    }

    fn check(r: &SignedReceipt, price: Option<&PricePerMTok>) -> Result<(), ReceiptError> {
        let (bob, charlie) = (key("bob"), key("charlie"));
        let genesis = H256::repeat_byte(0x11);
        let pk = key_fingerprint(&bob.public_key().unwrap());
        let gk = key_fingerprint(&charlie.public_key().unwrap());
        check_receipt(
            r,
            &ReceiptContext {
                genesis: &genesis,
                provider_key: &pk,
                gateway_key: &gk,
                price,
            },
        )
    }

    fn hexs(bytes: &[u8]) -> alloc::string::String {
        bytes.iter().map(|b| alloc::format!("{b:02x}")).collect()
    }

    // Regression vectors (design D2): encoding, payload and a deterministic ML-DSA-44 signature
    // prefix of a fixed receipt. The receipt format is published: never change these.
    #[test]
    fn receipt_vectors() {
        let b = body();
        assert_eq!(hexs(&b.encode()), VECTOR_BODY);
        assert_eq!(hexs(&b.payload().unwrap()), VECTOR_PAYLOAD);
        let r = sign(b, &key("bob"), &key("charlie"));
        let sig = r.provider_sig.to_canonical();
        assert_eq!(hexs(sig.get(..16).unwrap()), VECTOR_PROVIDER_SIG_PREFIX);
    }

    const VECTOR_BODY: &str = "1111111111111111111111111111111111111111111111111111111111111111bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbcccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc0042424242424242424242424242424242424242424242424242424242424242420101010101010101010101010101010101010101010101010101010101010101e8030000d0070000bc020000000000000000000000000000777777777777777777777777777777777777777777777777777777777777777778000000480d0000";
    const VECTOR_PAYLOAD: &str = "1b717587a184e2baf60082c8860a23e0b666b42ddb60da2b09c6470318e194f8";
    const VECTOR_PROVIDER_SIG_PREFIX: &str = "0113968911e3ad0b2b447c839a4f1af9";

    // Scenario "双签收据有效".
    #[test]
    fn a_doubly_signed_receipt_is_valid() {
        let r = sign(body(), &key("bob"), &key("charlie"));
        assert_eq!(check(&r, Some(&price())), Ok(()));
    }

    // Scenario "费用与价格不符": 1,000 × 0.1 + 2,000 × 0.3 = 700 micro-USD, not 600.
    #[test]
    fn a_wrong_fee_is_rejected() {
        let mut b = body();
        b.fee = MicroUsd(600);
        let r = sign(b, &key("bob"), &key("charlie"));
        assert_eq!(
            check(&r, Some(&price())),
            Err(ReceiptError::FeeMismatch {
                expected: MicroUsd(700)
            })
        );
    }

    // Scenario "费用向上取整".
    #[test]
    fn fees_round_up() {
        assert_eq!(fee_for(&price(), 1, 0), Some(MicroUsd(1)));
        assert_eq!(fee_for(&price(), 0, 0), Some(MicroUsd(0)));
        assert_eq!(fee_for(&price(), 10, 0), Some(MicroUsd(1)));
        assert_eq!(fee_for(&price(), 11, 0), Some(MicroUsd(2)));
        assert_eq!(fee_for(&price(), 1_000, 2_000), Some(MicroUsd(700)));
        let huge = PricePerMTok {
            input: MicroUsd(u128::MAX),
            output: MicroUsd(1),
        };
        assert_eq!(fee_for(&huge, 2, 0), None);
    }

    // Scenario "其他链的收据".
    #[test]
    fn another_chain_is_rejected() {
        let mut b = body();
        b.genesis = H256::repeat_byte(0x22);
        let r = sign(b, &key("bob"), &key("charlie"));
        assert_eq!(check(&r, Some(&price())), Err(ReceiptError::WrongGenesis));
    }

    #[test]
    fn reserved_job_kinds_are_rejected() {
        let mut b = body();
        b.kind = JobKind::Eval;
        let r = sign(b, &key("bob"), &key("charlie"));
        assert_eq!(
            check(&r, Some(&price())),
            Err(ReceiptError::UnsupportedJobKind)
        );
    }

    #[test]
    fn a_model_the_provider_does_not_offer_is_rejected() {
        let r = sign(body(), &key("bob"), &key("charlie"));
        assert_eq!(check(&r, None), Err(ReceiptError::ModelNotOffered));
    }

    #[test]
    fn keys_and_signatures_are_checked() {
        let (bob, charlie, dave) = (key("bob"), key("charlie"), key("dave"));
        // Keys that are not the registered ones.
        let r = sign(body(), &dave, &charlie);
        assert_eq!(
            check(&r, Some(&price())),
            Err(ReceiptError::WrongProviderKey)
        );
        let r = sign(body(), &bob, &dave);
        assert_eq!(
            check(&r, Some(&price())),
            Err(ReceiptError::WrongGatewayKey)
        );
        // A signature over another body.
        let mut r = sign(body(), &bob, &charlie);
        let other = sign(
            ReceiptBody {
                in_tokens: 10,
                fee: MicroUsd(1),
                ..body()
            },
            &bob,
            &charlie,
        );
        r.provider_sig = other.provider_sig.clone();
        assert_eq!(
            check(&r, Some(&price())),
            Err(ReceiptError::BadProviderSignature)
        );
        let mut r = sign(body(), &bob, &charlie);
        r.gateway_sig = other.gateway_sig;
        assert_eq!(
            check(&r, Some(&price())),
            Err(ReceiptError::BadGatewaySignature)
        );
        // The same bytes signed under the voucher context are not a receipt signature.
        let b = body();
        let payload = b.payload().unwrap();
        let mut r = sign(b, &bob, &charlie);
        r.provider_sig = bob
            .sign_deterministic(&payload, super::super::voucher::VOUCHER_CONTEXT)
            .unwrap();
        assert_eq!(
            check(&r, Some(&price())),
            Err(ReceiptError::BadProviderSignature)
        );
    }
}
