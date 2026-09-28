//! Transparent cumulative vouchers (m5-market-registry design D7, spec
//! `market/transparent-credits`).
//!
//! A user escrows ATC for a gateway in a *channel*; each voucher authorizes the gateway to take a
//! cumulative dollar amount from the channel. The chain stores one counter per channel (the
//! amount already redeemed), so replaying an old voucher pays nothing and no nonce set is kept.
//!
//! [`check_voucher`] is the one implementation of the redemption rules: the chain calls it when
//! redeeming and the gateway calls it (through the `MarketApi`) before accepting a request.

use parity_scale_codec::{Decode, DecodeWithMemTracking, Encode, MaxEncodedLen};
use scale_info::TypeInfo;
use sp_core::H256;
use sp_runtime::AccountId32;

use ac_crypto::{PqPublicKey, PqSignature};

use super::usd::{AtcPerUsd, MicroUsd, PriceError, to_atc_payment};

/// Signing context of vouchers.
pub const VOUCHER_CONTEXT: &[u8] = b"agentcoin/voucher/v1";

/// Hashing context of the 32-byte voucher signing payload.
pub const VOUCHER_PAYLOAD_CONTEXT: &str = "agentcoin 2026-09 voucher-payload v1";

/// What a voucher authorizes.
#[derive(
    Clone, Debug, PartialEq, Eq, Encode, Decode, DecodeWithMemTracking, MaxEncodedLen, TypeInfo,
)]
pub struct VoucherBody {
    /// Genesis hash of the chain the voucher is valid on.
    pub genesis: H256,
    /// The paying user.
    pub user: AccountId32,
    /// The gateway allowed to redeem it.
    pub gateway: AccountId32,
    /// Channel number; a channel reset makes every earlier voucher void.
    pub channel: u32,
    /// Total the gateway may take from this channel since it was opened or last reset.
    pub cumulative: MicroUsd,
}

impl VoucherBody {
    /// The 32-byte message that is signed:
    /// `derive("agentcoin 2026-09 voucher-payload v1", SCALE(body))`.
    ///
    /// # Errors
    ///
    /// Only if the published hashing context were rejected, which does not happen.
    pub fn payload(&self) -> Result<[u8; 32], ac_crypto::Error> {
        ac_crypto::hash::derive(VOUCHER_PAYLOAD_CONTEXT, &self.encode())
    }
}

/// A voucher with its ML-DSA signature. The public key travels with it because the chain stores
/// only the key's fingerprint.
#[derive(
    Clone, Debug, PartialEq, Eq, Encode, Decode, DecodeWithMemTracking, MaxEncodedLen, TypeInfo,
)]
pub struct SignedVoucher {
    /// What is authorized.
    pub body: VoucherBody,
    /// The channel's voucher key.
    pub public_key: PqPublicKey,
    /// Signature over [`VoucherBody::payload`] under [`VOUCHER_CONTEXT`].
    pub signature: PqSignature,
}

/// Fingerprint of a voucher key: BLAKE3-256 of the key's canonical (AlgId-tagged) encoding, the
/// same value `pallet-pq-accounts` indexes keys by.
#[must_use]
pub fn key_fingerprint(public_key: &PqPublicKey) -> [u8; 32] {
    ac_crypto::hash::blake3_256(&public_key.to_canonical())
}

/// The part of a channel's state a voucher is checked against.
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
pub struct ChannelView {
    /// Escrow still in the channel, in smallest ATC units.
    pub escrow: u128,
    /// Current channel number.
    pub number: u32,
    /// Cumulative amount already redeemed.
    pub redeemed: MicroUsd,
    /// Fingerprint of the key vouchers must be signed with now.
    pub key: [u8; 32],
}

/// Why a voucher is rejected.
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
pub enum VoucherError {
    /// Signed for another chain.
    WrongGenesis,
    /// Made out to another gateway.
    WrongGateway,
    /// The user has no channel with the gateway.
    NoChannel,
    /// The channel number is not the channel's current one.
    WrongChannel,
    /// Signed with a key that is not the channel's voucher key.
    WrongKey,
    /// The signature does not verify.
    BadSignature,
    /// The increment cannot be converted: no reference rate.
    RateNotSet,
    /// The increment cannot be converted: overflow.
    Overflow,
}

impl core::fmt::Display for VoucherError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match self {
            Self::WrongGenesis => "the voucher was signed for another chain",
            Self::WrongGateway => "the voucher is made out to another gateway",
            Self::NoChannel => "the user has no channel with this gateway",
            Self::WrongChannel => "the voucher's channel number is not the current one",
            Self::WrongKey => "the voucher is not signed with the channel's voucher key",
            Self::BadSignature => "the voucher signature does not verify",
            Self::RateNotSet => "the reference rate is not set",
            Self::Overflow => "the ATC amount overflows",
        })
    }
}

#[cfg(feature = "std")]
impl std::error::Error for VoucherError {}

impl From<PriceError> for VoucherError {
    fn from(e: PriceError) -> Self {
        match e {
            PriceError::NotSet => Self::RateNotSet,
            PriceError::Overflow => Self::Overflow,
        }
    }
}

/// What an accepted voucher is worth against the channel.
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
pub struct VoucherCheck {
    /// Cumulative amount minus the amount already redeemed (zero if it did not grow).
    pub increment: MicroUsd,
    /// The increment in smallest ATC units, rounded down.
    pub increment_atc: u128,
    /// Whether the escrow covers `increment_atc`.
    pub covered: bool,
}

/// The expected context of a voucher: which chain, which gateway, which channel state.
#[derive(Clone, Copy, Debug)]
pub struct VoucherContext<'a> {
    /// Genesis hash of this chain.
    pub genesis: &'a H256,
    /// The gateway redeeming (or accepting) the voucher.
    pub gateway: &'a AccountId32,
    /// The user's channel with that gateway, if any.
    pub channel: Option<&'a ChannelView>,
    /// Current reference rate, if set.
    pub rate: Option<AtcPerUsd>,
}

/// Checks `voucher` against `ctx` and values its increment. Shared by on-chain redemption and
/// off-chain checks (spec: "链下校验与链上一致").
///
/// A voucher whose cumulative amount did not grow is valid and worth nothing; it needs no rate.
///
/// # Errors
///
/// The first failing rule, in this order: genesis, gateway, channel, channel number, key,
/// signature, then conversion ([`VoucherError::RateNotSet`], [`VoucherError::Overflow`]).
pub fn check_voucher(
    voucher: &SignedVoucher,
    ctx: &VoucherContext<'_>,
) -> Result<VoucherCheck, VoucherError> {
    let body = &voucher.body;
    if body.genesis != *ctx.genesis {
        return Err(VoucherError::WrongGenesis);
    }
    if body.gateway != *ctx.gateway {
        return Err(VoucherError::WrongGateway);
    }
    let channel = ctx.channel.ok_or(VoucherError::NoChannel)?;
    if body.channel != channel.number {
        return Err(VoucherError::WrongChannel);
    }
    if key_fingerprint(&voucher.public_key) != channel.key {
        return Err(VoucherError::WrongKey);
    }
    let payload = body.payload().map_err(|_| VoucherError::BadSignature)?;
    ac_crypto::sig::verify(
        &voucher.public_key,
        &payload,
        VOUCHER_CONTEXT,
        &voucher.signature,
    )
    .map_err(|_| VoucherError::BadSignature)?;

    let increment = body.cumulative.saturating_sub(channel.redeemed);
    let increment_atc = if increment == MicroUsd::ZERO {
        0
    } else {
        to_atc_payment(increment, ctx.rate.ok_or(VoucherError::RateNotSet)?)?
    };
    Ok(VoucherCheck {
        increment,
        increment_atc,
        covered: increment_atc <= channel.escrow,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use ac_crypto::{SigAlg, sig::SigningKey};

    const ONE_ATC_PER_USD: AtcPerUsd = AtcPerUsd(1_000_000_000_000_000_000);

    fn key(name: &str) -> SigningKey {
        SigningKey::from_seed(SigAlg::MlDsa44, &ac_crypto::dev_seed(name).unwrap()).unwrap()
    }

    fn body(cumulative: u128) -> VoucherBody {
        VoucherBody {
            genesis: H256::repeat_byte(0x11),
            user: AccountId32::new([0xaa; 32]),
            gateway: AccountId32::new([0xbb; 32]),
            channel: 0,
            cumulative: MicroUsd(cumulative),
        }
    }

    fn sign(body: VoucherBody, key: &SigningKey, context: &[u8]) -> SignedVoucher {
        let signature = key
            .sign_deterministic(&body.payload().unwrap(), context)
            .unwrap();
        SignedVoucher {
            body,
            public_key: key.public_key().unwrap(),
            signature,
        }
    }

    fn view(key: &SigningKey, redeemed: u128, escrow: u128) -> ChannelView {
        ChannelView {
            escrow,
            number: 0,
            redeemed: MicroUsd(redeemed),
            key: key_fingerprint(&key.public_key().unwrap()),
        }
    }

    fn ctx<'a>(
        channel: Option<&'a ChannelView>,
        genesis: &'a H256,
        gw: &'a AccountId32,
    ) -> VoucherContext<'a> {
        VoucherContext {
            genesis,
            gateway: gw,
            channel,
            rate: Some(ONE_ATC_PER_USD),
        }
    }

    fn hexs(bytes: &[u8]) -> alloc::string::String {
        bytes.iter().map(|b| alloc::format!("{b:02x}")).collect()
    }

    // Regression vectors (design D7): encoding, payload and a deterministic ML-DSA-44 signature
    // prefix of a fixed voucher. The voucher format is published: never change these.
    #[test]
    fn voucher_vectors() {
        let alice = key("alice");
        let b = body(250);
        assert_eq!(hexs(&b.encode()), VECTOR_BODY);
        assert_eq!(hexs(&b.payload().unwrap()), VECTOR_PAYLOAD);
        let v = sign(b, &alice, VOUCHER_CONTEXT);
        let sig = v.signature.to_canonical();
        assert_eq!(hexs(sig.get(..16).unwrap()), VECTOR_SIGNATURE_PREFIX);
    }

    const VECTOR_BODY: &str = "1111111111111111111111111111111111111111111111111111111111111111aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaabbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb00000000fa000000000000000000000000000000";
    const VECTOR_PAYLOAD: &str = "7775c89d54e5f685e365188b8883565562221dc1ce5c39cede02410e84c6d678";
    const VECTOR_SIGNATURE_PREFIX: &str = "01b0c191b299582833118c70bf2c34f5";

    #[test]
    fn a_valid_voucher_is_valued() {
        let alice = key("alice");
        let (g, gw) = (H256::repeat_byte(0x11), AccountId32::new([0xbb; 32]));
        let ch = view(&alice, 100, u128::MAX);
        let v = sign(body(250), &alice, VOUCHER_CONTEXT);
        assert_eq!(
            check_voucher(&v, &ctx(Some(&ch), &g, &gw)),
            Ok(VoucherCheck {
                increment: MicroUsd(150),
                increment_atc: 150_000_000_000_000,
                covered: true,
            })
        );
    }

    #[test]
    fn every_rule_has_its_error() {
        let (alice, bob) = (key("alice"), key("bob"));
        let (g, gw) = (H256::repeat_byte(0x11), AccountId32::new([0xbb; 32]));
        let ch = view(&alice, 0, 10);
        let good = sign(body(1), &alice, VOUCHER_CONTEXT);
        let check = |v: &SignedVoucher, c: &VoucherContext<'_>| check_voucher(v, c).map(|_| ());

        let other_genesis = H256::repeat_byte(0x22);
        assert_eq!(
            check(&good, &ctx(Some(&ch), &other_genesis, &gw)),
            Err(VoucherError::WrongGenesis)
        );
        let other_gw = AccountId32::new([0xcc; 32]);
        assert_eq!(
            check(&good, &ctx(Some(&ch), &g, &other_gw)),
            Err(VoucherError::WrongGateway)
        );
        assert_eq!(
            check(&good, &ctx(None, &g, &gw)),
            Err(VoucherError::NoChannel)
        );
        let reset = ChannelView { number: 1, ..ch };
        assert_eq!(
            check(&good, &ctx(Some(&reset), &g, &gw)),
            Err(VoucherError::WrongChannel)
        );
        let by_bob = sign(body(1), &bob, VOUCHER_CONTEXT);
        assert_eq!(
            check(&by_bob, &ctx(Some(&ch), &g, &gw)),
            Err(VoucherError::WrongKey)
        );
        let mut tampered = good.clone();
        tampered.body.cumulative = MicroUsd(2);
        assert_eq!(
            check(&tampered, &ctx(Some(&ch), &g, &gw)),
            Err(VoucherError::BadSignature)
        );
        let no_rate = VoucherContext {
            rate: None,
            ..ctx(Some(&ch), &g, &gw)
        };
        assert_eq!(check(&good, &no_rate), Err(VoucherError::RateNotSet));
    }

    // Spec market/transparent-credits, Scenario "协议签名不能充当凭证".
    #[test]
    fn a_transaction_signature_is_not_a_voucher() {
        let alice = key("alice");
        let (g, gw) = (H256::repeat_byte(0x11), AccountId32::new([0xbb; 32]));
        let ch = view(&alice, 0, 10);
        let v = sign(body(1), &alice, b"agentcoin/tx/v1");
        assert_eq!(
            check_voucher(&v, &ctx(Some(&ch), &g, &gw)),
            Err(VoucherError::BadSignature)
        );
    }

    #[test]
    fn a_stale_voucher_is_worth_nothing_even_without_a_rate() {
        let alice = key("alice");
        let (g, gw) = (H256::repeat_byte(0x11), AccountId32::new([0xbb; 32]));
        let ch = view(&alice, 500, 0);
        let v = sign(body(400), &alice, VOUCHER_CONTEXT);
        let c = VoucherContext {
            rate: None,
            ..ctx(Some(&ch), &g, &gw)
        };
        assert_eq!(
            check_voucher(&v, &c),
            Ok(VoucherCheck {
                increment: MicroUsd::ZERO,
                increment_atc: 0,
                covered: true
            })
        );
    }

    #[test]
    fn coverage_follows_the_escrow() {
        let alice = key("alice");
        let (g, gw) = (H256::repeat_byte(0x11), AccountId32::new([0xbb; 32]));
        let v = sign(body(1_000_000), &alice, VOUCHER_CONTEXT); // one dollar = 10^18 units
        let short = view(&alice, 0, 999_999_999_999_999_999);
        assert!(
            !check_voucher(&v, &ctx(Some(&short), &g, &gw))
                .unwrap()
                .covered
        );
        let exact = view(&alice, 0, 1_000_000_000_000_000_000);
        assert!(
            check_voucher(&v, &ctx(Some(&exact), &g, &gw))
                .unwrap()
                .covered
        );
    }

    #[test]
    fn the_fingerprint_matches_pq_accounts() {
        // pallet-pq-accounts indexes keys by BLAKE3 of the canonical encoding; so do vouchers.
        let pk = key("alice").public_key().unwrap();
        assert_eq!(
            key_fingerprint(&pk),
            ac_crypto::hash::blake3_256(&pk.to_canonical())
        );
    }
}
