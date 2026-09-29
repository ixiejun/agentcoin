//! On-chain records of providers, gateways and credit channels (m5-market-registry design D5–D7).
//!
//! The pallets store these types directly and the `MarketApi` returns them, so clients decode
//! exactly what the chain stores.

use parity_scale_codec::{Decode, DecodeWithMemTracking, Encode, MaxEncodedLen};
use scale_info::TypeInfo;
use sp_core::ConstU32;
use sp_runtime::BoundedVec;

use ac_crypto::KemPublicKey;

use super::model::ModelId;
use super::usd::{MicroUsd, PricePerMTok};
use super::voucher::ChannelView;

/// Longest service endpoint, in bytes.
pub const MAX_ENDPOINT_LEN: u32 = 256;
/// Most models one provider serves.
pub const MAX_PROVIDER_MODELS: u32 = 16;
/// Most unbonding chunks; further requests merge into the last one.
pub const MAX_UNLOCKING: u32 = 8;
/// Highest gateway fee, in basis points (plan §8: 5%).
pub const MAX_GATEWAY_FEE_BPS: u16 = 500;

/// A service endpoint (UTF-8, not checked for reachability).
pub type Endpoint = BoundedVec<u8, ConstU32<MAX_ENDPOINT_LEN>>;

/// Provider tier (D3).
///
/// Wire-format enum: discriminants are explicit and never reused.
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
#[repr(u8)]
pub enum Tier {
    /// \[Reserved\] TEE confidential tier (full version); rejected in the MVP.
    #[codec(index = 0)]
    T0 = 0,
    /// Data-center GPUs.
    #[codec(index = 1)]
    T1 = 1,
    /// Consumer GPUs.
    #[codec(index = 2)]
    T2 = 2,
}

/// Lifecycle of a provider.
///
/// Wire-format enum: discriminants are explicit and never reused.
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
#[repr(u8)]
pub enum ProviderStatus {
    /// Registered and able to serve (if staked and heartbeating).
    #[codec(index = 1)]
    Active = 1,
    /// Leaving: the stake is unbonding.
    #[codec(index = 2)]
    Exiting = 2,
    /// Jailed by an audit (M6); never serviceable.
    #[codec(index = 3)]
    Jailed = 3,
}

/// Lifecycle of a gateway.
///
/// Wire-format enum: discriminants are explicit and never reused.
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
#[repr(u8)]
pub enum GatewayStatus {
    /// Registered and accepting escrow.
    #[codec(index = 1)]
    Active = 1,
    /// Leaving: no new escrow, the stake is unbonding.
    #[codec(index = 2)]
    Exiting = 2,
}

/// A model a provider serves and its price.
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
pub struct ModelPrice {
    /// The model.
    pub model: ModelId,
    /// Its price per million tokens.
    pub price: PricePerMTok,
}

/// Stake that is unbonding: still held (and slashable) until `unlock_at`.
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
pub struct Unlocking<Balance, BlockNumber> {
    /// Amount.
    pub amount: Balance,
    /// First block at which it can be withdrawn.
    pub unlock_at: BlockNumber,
}

/// \[Reserved\] Service metrics written by audits (M6); zero until then.
#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    PartialEq,
    Eq,
    Encode,
    Decode,
    DecodeWithMemTracking,
    MaxEncodedLen,
    TypeInfo,
)]
pub struct SlaMetrics {
    /// Median time to first token, in milliseconds.
    pub ttft_p50_ms: u32,
    /// 95th-percentile time to first token, in milliseconds.
    pub ttft_p95_ms: u32,
    /// Output throughput, in tokens per second.
    pub tokens_per_sec: u32,
    /// Error rate, in basis points.
    pub error_bps: u16,
    /// Audit pass rate, in basis points.
    pub audit_pass_bps: u16,
}

/// \[Reserved\] Reference to a TEE attestation (T0, full version); always `None` in the MVP.
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
pub struct AttestationRef(pub [u8; 32]);

/// A registered inference provider.
#[derive(
    Clone, Debug, PartialEq, Eq, Encode, Decode, DecodeWithMemTracking, MaxEncodedLen, TypeInfo,
)]
pub struct ProviderRecord<Balance, BlockNumber> {
    /// Tier (T1 or T2).
    pub tier: Tier,
    /// Where gateways connect.
    pub endpoint: Endpoint,
    /// X-Wing key gateways encrypt requests to.
    pub kem_pk: KemPublicKey,
    /// Served models and prices (1–16, no duplicates).
    pub models: BoundedVec<ModelPrice, ConstU32<MAX_PROVIDER_MODELS>>,
    /// Active stake (held).
    pub stake: Balance,
    /// Unbonding stake (held, slashable).
    pub unlocking: BoundedVec<Unlocking<Balance, BlockNumber>, ConstU32<MAX_UNLOCKING>>,
    /// Lifecycle status.
    pub status: ProviderStatus,
    /// Block of the latest heartbeat.
    pub last_heartbeat: BlockNumber,
    /// \[Reserved\] Audit metrics (M6).
    pub metrics: SlaMetrics,
    /// \[Reserved\] TEE attestation (T0).
    pub attestation: Option<AttestationRef>,
    /// Block of registration.
    pub registered_at: BlockNumber,
}

/// A registered gateway.
#[derive(
    Clone, Debug, PartialEq, Eq, Encode, Decode, DecodeWithMemTracking, MaxEncodedLen, TypeInfo,
)]
pub struct GatewayRecord<Balance, BlockNumber> {
    /// Where users connect.
    pub endpoint: Endpoint,
    /// Gateway fee, in basis points (at most [`MAX_GATEWAY_FEE_BPS`]).
    pub fee_bps: u16,
    /// Active stake (held).
    pub stake: Balance,
    /// Unbonding stake (held).
    pub unlocking: BoundedVec<Unlocking<Balance, BlockNumber>, ConstU32<MAX_UNLOCKING>>,
    /// Lifecycle status.
    pub status: GatewayStatus,
    /// Block of registration.
    pub registered_at: BlockNumber,
}

/// A user's credit channel with one gateway.
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
pub struct ChannelRecord<Balance, BlockNumber> {
    /// Escrow of this channel, held on the user's account.
    pub escrow: Balance,
    /// Channel number; incremented when the channel is reset.
    pub number: u32,
    /// Cumulative amount already redeemed.
    pub redeemed: MicroUsd,
    /// Fingerprint of the voucher key in force.
    pub key: [u8; 32],
    /// Pending withdrawal: amount and first block it can be withdrawn.
    pub pending_withdrawal: Option<(Balance, BlockNumber)>,
    /// Pending voucher key: fingerprint and block it takes effect.
    pub pending_key: Option<([u8; 32], BlockNumber)>,
}

impl<Balance: Copy + Into<u128>, BlockNumber: PartialOrd + Copy>
    ChannelRecord<Balance, BlockNumber>
{
    /// The voucher key in force at block `now`: a pending key takes over at its block.
    #[must_use]
    pub fn key_at(&self, now: BlockNumber) -> [u8; 32] {
        match self.pending_key {
            Some((key, at)) if now >= at => key,
            _ => self.key,
        }
    }

    /// The state vouchers are checked against at block `now`.
    #[must_use]
    pub fn view_at(&self, now: BlockNumber) -> ChannelView {
        ChannelView {
            escrow: self.escrow.into(),
            number: self.number,
            redeemed: self.redeemed,
            key: self.key_at(now),
        }
    }
}

/// Unbonding chunks of a provider or gateway.
pub type UnlockingList<BlockNumber> =
    BoundedVec<Unlocking<u128, BlockNumber>, ConstU32<MAX_UNLOCKING>>;

/// Schedules `amount` to unlock at `unlock_at`. When the list is full the amount merges into the
/// last chunk, which then unlocks at `unlock_at` (never earlier than it would have: calls come
/// in block order, so `unlock_at` is the latest).
pub fn schedule_unlock<BlockNumber: Copy>(
    list: &mut UnlockingList<BlockNumber>,
    amount: u128,
    unlock_at: BlockNumber,
) {
    if let Err(chunk) = list.try_push(Unlocking { amount, unlock_at })
        && let Some(last) = list.last_mut()
    {
        last.amount = last.amount.saturating_add(chunk.amount);
        last.unlock_at = unlock_at;
    }
}

/// Removes the chunks due at `now` and returns their total.
pub fn take_due<BlockNumber: Copy + PartialOrd>(
    list: &mut UnlockingList<BlockNumber>,
    now: BlockNumber,
) -> u128 {
    let mut due = 0u128;
    list.retain(|c| {
        if c.unlock_at <= now {
            due = due.saturating_add(c.amount);
            false
        } else {
            true
        }
    });
    due
}

/// Takes up to `target` from `stake` first, then from the chunks in unlock order (earliest
/// first); empty chunks are dropped. Returns the amount taken.
pub fn take_for_slash<BlockNumber: Copy + Ord>(
    stake: &mut u128,
    list: &mut UnlockingList<BlockNumber>,
    target: u128,
) -> u128 {
    let from_stake = target.min(*stake);
    *stake = stake.saturating_sub(from_stake);
    let mut rest = target.saturating_sub(from_stake);
    list.sort_by_key(|c| c.unlock_at);
    for chunk in list.iter_mut() {
        let take = rest.min(chunk.amount);
        chunk.amount = chunk.amount.saturating_sub(take);
        rest = rest.saturating_sub(take);
    }
    list.retain(|c| c.amount > 0);
    target.saturating_sub(rest)
}

/// Total still unbonding.
#[must_use]
pub fn unlocking_total<BlockNumber>(list: &UnlockingList<BlockNumber>) -> u128 {
    list.iter()
        .fold(0u128, |acc, c| acc.saturating_add(c.amount))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list(chunks: &[(u128, u32)]) -> UnlockingList<u32> {
        BoundedVec::truncate_from(
            chunks
                .iter()
                .map(|&(amount, unlock_at)| Unlocking { amount, unlock_at })
                .collect(),
        )
    }

    #[test]
    fn a_full_list_merges_into_the_last_chunk() {
        let mut l = list(&[(1, 1); 8]);
        schedule_unlock(&mut l, 5, 20);
        assert_eq!(l.len(), 8);
        assert_eq!(
            l.last(),
            Some(&Unlocking {
                amount: 6,
                unlock_at: 20
            })
        );
        assert_eq!(unlocking_total(&l), 13);
    }

    #[test]
    fn only_due_chunks_are_taken() {
        let mut l = list(&[(3, 5), (4, 10), (5, 15)]);
        assert_eq!(take_due(&mut l, 10), 7);
        assert_eq!(
            l.into_inner(),
            vec![Unlocking {
                amount: 5,
                unlock_at: 15
            }]
        );
    }

    #[test]
    fn slashing_takes_stake_then_earliest_chunks() {
        let mut stake = 10;
        let mut l = list(&[(5, 30), (5, 20)]);
        assert_eq!(take_for_slash(&mut stake, &mut l, 17), 17);
        assert_eq!(stake, 0);
        assert_eq!(
            l.into_inner(),
            vec![Unlocking {
                amount: 3,
                unlock_at: 30
            }]
        );
        let mut stake = 1;
        let mut l = list(&[(1, 5)]);
        assert_eq!(take_for_slash(&mut stake, &mut l, 10), 2);
    }
}
