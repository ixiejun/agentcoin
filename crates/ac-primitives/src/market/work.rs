//! Work settlement types (m5-work-settlement design D5, D9; spec `market/work-settlement`).

use alloc::vec::Vec;
use parity_scale_codec::{Decode, DecodeWithMemTracking, Encode, MaxEncodedLen};
use scale_info::TypeInfo;
use sp_runtime::{Rounding, helpers_128bit::multiply_by_rational_with_rounding};

use sp_core::ConstU32;
use sp_runtime::BoundedVec;

use super::model::ModelId;
use super::usd::MicroUsd;
use crate::emission::EpochIndex;

/// Basis points in one.
pub const BPS: u32 = 10_000;

/// Most entries in one work report. Each entry costs up to three storage writes; with
/// [`MAX_REPORT_VOUCHERS`] this keeps a full report under half of a normal transaction's weight.
pub const MAX_REPORT_ENTRIES: u32 = 128;

/// Most vouchers in one work report (one ML-DSA-87 verification and redemption each).
pub const MAX_REPORT_VOUCHERS: u32 = 16;

/// Kind of work a receipt or report entry is for (full plan §11).
///
/// Wire-format enum: discriminants are explicit and never reused (AGENT.md §5.6). Only
/// [`JobKind::Inference`] is accepted in M5; the others are reserved for public jobs (M6) and
/// training and storage (full version).
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
    serde::Serialize,
    serde::Deserialize,
)]
#[repr(u8)]
pub enum JobKind {
    /// Paid inference through a gateway (the market).
    #[codec(index = 0)]
    Inference = 0,
    /// Model evaluation (public jobs).
    #[codec(index = 1)]
    Eval = 1,
    /// Data cleaning and deduplication (public jobs).
    #[codec(index = 2)]
    DataClean = 2,
    /// Embedding computation (public jobs).
    #[codec(index = 3)]
    Embed = 3,
    /// Reinforcement learning (full version).
    #[codec(index = 16)]
    Rl = 16,
    /// Fine-tuning (full version).
    #[codec(index = 17)]
    Finetune = 17,
    /// Pre-training (full version).
    #[codec(index = 18)]
    Pretrain = 18,
    /// Storage (full version).
    #[codec(index = 32)]
    Storage = 32,
}

impl JobKind {
    /// Whether this kind can be settled now: only inference in M5.
    #[must_use]
    pub const fn is_supported(self) -> bool {
        matches!(self, Self::Inference)
    }
}

/// One line of a work report: what a provider earned for one model, summed over receipts.
#[derive(
    Clone, Debug, PartialEq, Eq, Encode, Decode, DecodeWithMemTracking, MaxEncodedLen, TypeInfo,
)]
pub struct ReportEntry<AccountId> {
    /// Kind of work; only [`JobKind::Inference`] in M5.
    pub kind: JobKind,
    /// The provider.
    pub provider: AccountId,
    /// The model served.
    pub model: ModelId,
    /// Sum of the receipts' fees; positive.
    pub usd: MicroUsd,
    /// Sum of prompt tokens.
    pub in_tokens: u64,
    /// Sum of generated tokens.
    pub out_tokens: u64,
}

/// Settlement parameters (genesis; guardrails from M8). Ratios in basis points.
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
    serde::Serialize,
    serde::Deserialize,
)]
pub struct WorkParams {
    /// Share of each report's settled amount that is burned (`b`).
    pub burn_bps: u32,
    /// Market work per unit of settled amount (`k`).
    pub emission_bps: u32,
    /// Challenge period in emission epochs (`C`).
    pub challenge_epochs: u32,
    /// Epochs a report stays queryable after it matures.
    pub retention_epochs: u32,
}

impl WorkParams {
    /// Live-chain values: `b` = 20%, `k` = 0.5, two epochs of challenge, 30 days of retention.
    pub const LIVE: Self = Self {
        burn_bps: 2_000,
        emission_bps: 5_000,
        challenge_epochs: 2,
        retention_epochs: 720,
    };

    /// Checks the bounds: `1000 ≤ b ≤ 9000`, `k − b ≤ 5000` (research 03 §1.5), a challenge
    /// period and a retention of at least one epoch.
    ///
    /// # Errors
    ///
    /// The first violated bound.
    pub fn validate(&self) -> Result<(), WorkParamsError> {
        if !(1_000..=9_000).contains(&self.burn_bps) {
            return Err(WorkParamsError::BurnOutOfRange);
        }
        if self.emission_bps.saturating_sub(self.burn_bps) > 5_000 {
            return Err(WorkParamsError::EmissionAboveBurn);
        }
        if self.challenge_epochs == 0 {
            return Err(WorkParamsError::NoChallengePeriod);
        }
        if self.retention_epochs == 0 {
            return Err(WorkParamsError::NoRetention);
        }
        Ok(())
    }
}

impl Default for WorkParams {
    fn default() -> Self {
        Self::LIVE
    }
}

/// A [`WorkParams`] bound that does not hold.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum WorkParamsError {
    /// `b` outside 1,000–9,000 basis points.
    BurnOutOfRange,
    /// `k − b` above 5,000 basis points.
    EmissionAboveBurn,
    /// A challenge period of zero epochs.
    NoChallengePeriod,
    /// A retention of zero epochs.
    NoRetention,
}

impl core::fmt::Display for WorkParamsError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match self {
            Self::BurnOutOfRange => "the burn ratio must be within 1,000-9,000 basis points",
            Self::EmissionAboveBurn => {
                "the emission multiple may exceed the burn ratio by at most 5,000 basis points"
            }
            Self::NoChallengePeriod => "the challenge period must be at least one epoch",
            Self::NoRetention => "reports must be kept for at least one epoch",
        })
    }
}

#[cfg(feature = "std")]
impl std::error::Error for WorkParamsError {}

/// One report entry with what it was allotted.
#[derive(
    Clone, Debug, PartialEq, Eq, Encode, Decode, DecodeWithMemTracking, MaxEncodedLen, TypeInfo,
)]
pub struct ReportLine<AccountId, Balance> {
    /// The entry as submitted.
    pub entry: ReportEntry<AccountId>,
    /// The provider's share of the pool.
    pub share: Balance,
    /// Market work credited to the provider.
    pub work: u128,
}

/// An accepted work report, as stored and queried.
#[derive(
    Clone, Debug, PartialEq, Eq, Encode, Decode, DecodeWithMemTracking, MaxEncodedLen, TypeInfo,
)]
pub struct ReportRecord<AccountId, Balance> {
    /// The submitting gateway.
    pub gateway: AccountId,
    /// Emission epoch of submission.
    pub submitted: EpochIndex,
    /// Epoch whose settlement makes it claimable (`submitted + C`).
    pub matures: EpochIndex,
    /// Merkle root of the receipts.
    pub root: [u8; 32],
    /// Number of receipts under the root.
    pub receipt_count: u32,
    /// `G`: amount redeemed plus the shortfall the gateway covered.
    pub settled: Balance,
    /// Burned at submission.
    pub burned: Balance,
    /// The gateway's fee.
    pub gateway_fee: Balance,
    /// Entries with their allotments.
    pub lines: BoundedVec<ReportLine<AccountId, Balance>, ConstU32<MAX_REPORT_ENTRIES>>,
}

/// What one gateway holds for an account, for one maturity epoch.
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
pub struct Held<Balance> {
    /// Provider shares held for the account.
    pub shares: Balance,
    /// The gateway's own fees (only when the account is the gateway). Kept apart from shares:
    /// jailing a provider voids its shares, never its fees as a gateway.
    pub fees: Balance,
}

/// A provider's market work maturing in one epoch.
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
pub struct ProviderWork {
    /// Market work of the reports maturing then.
    pub work: u128,
    /// Voided by a jail before the epoch was settled: no emission, shares burned on claim.
    pub voided: bool,
    /// The epoch's market emission share was claimed (or skipped because voided).
    pub emission_claimed: bool,
    /// Gateways still holding shares for the provider for this epoch; the record is removed
    /// once this is zero and the emission is claimed.
    pub held: u32,
}

/// Verified market work of an epoch and the emission it earned.
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
pub struct EpochWork<Balance> {
    /// Work of the reports maturing in the epoch, less voided work.
    pub verified: u128,
    /// Market emission minted for the epoch; `None` until the epoch is settled.
    pub market: Option<Balance>,
}

/// An account's work over its lifetime.
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
pub struct LifetimeWork<Balance> {
    /// Work of reports not yet claimed.
    pub pending: u128,
    /// Work claimed after its challenge period.
    pub verified: u128,
    /// Fees and emission claimed.
    pub claimed: Balance,
}

/// How one report's settled amount `G` is split (design D5).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Allocation {
    /// Burned: `G × b` plus the rounding remainder of the provider pool.
    pub burn: u128,
    /// The gateway's fee: `G × fee`.
    pub gateway_fee: u128,
    /// Per entry, in input order: its share of the provider pool.
    pub shares: Vec<u128>,
    /// Per entry: market work `G_i × k`.
    pub work: Vec<u128>,
}

/// Why an amount cannot be allocated.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum AllocError {
    /// No entries, or entries worth nothing in total.
    NothingToAllocate,
    /// A basis-point ratio above one.
    BadRatio,
    /// The dollar total overflows.
    Overflow,
}

impl core::fmt::Display for AllocError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match self {
            Self::NothingToAllocate => "there is nothing to allocate",
            Self::BadRatio => "a ratio exceeds 10,000 basis points",
            Self::Overflow => "the dollar total overflows",
        })
    }
}

#[cfg(feature = "std")]
impl std::error::Error for AllocError {}

/// `value × num / den`, rounded down; `den` is positive and `num ≤ den` wherever this is used,
/// so the result fits.
fn part(value: u128, num: u128, den: u128) -> u128 {
    multiply_by_rational_with_rounding(value, num, den, Rounding::Down).unwrap_or(0)
}

/// Splits `g` (smallest ATC units) between burning (`burn_bps`), the gateway (`fee_bps`) and the
/// providers of `usd` (each entry's dollar amount), all rounded down; the rounding remainder of
/// the provider pool is burned too. Work per entry is `G_i × emission_bps`, where `G_i` is the
/// entry's dollar-proportional part of `g`.
///
/// `burn + gateway_fee + Σ shares == g` always holds.
///
/// # Errors
///
/// [`AllocError::NothingToAllocate`] without entries or with a zero dollar total,
/// [`AllocError::BadRatio`] if `burn_bps + fee_bps` exceeds 10,000, [`AllocError::Overflow`] if
/// the dollar total overflows.
pub fn allocate(
    g: u128,
    burn_bps: u32,
    fee_bps: u32,
    emission_bps: u32,
    usd: &[MicroUsd],
) -> Result<Allocation, AllocError> {
    if burn_bps.saturating_add(fee_bps) > BPS {
        return Err(AllocError::BadRatio);
    }
    let total = usd
        .iter()
        .try_fold(0u128, |a, u| a.checked_add(u.0))
        .ok_or(AllocError::Overflow)?;
    if total == 0 {
        return Err(AllocError::NothingToAllocate);
    }
    let bps = u128::from(BPS);
    let burn_b = part(g, u128::from(burn_bps), bps);
    let gateway_fee = part(g, u128::from(fee_bps), bps);
    // burn_bps + fee_bps ≤ 10,000, so both parts together never exceed g.
    let pool = g.saturating_sub(burn_b).saturating_sub(gateway_fee);
    let shares: Vec<u128> = usd.iter().map(|u| part(pool, u.0, total)).collect();
    let work = usd
        .iter()
        .map(|u| {
            let g_i = part(g, u.0, total);
            // k may exceed one (at most 1.4 under WorkParams' bounds); G_i ≤ g ≤ total issuance,
            // so the product fits and the saturation below is never reached.
            multiply_by_rational_with_rounding(g_i, u128::from(emission_bps), bps, Rounding::Down)
                .unwrap_or(u128::MAX)
        })
        .collect();
    let paid = shares.iter().fold(0u128, |a, s| a.saturating_add(*s));
    let burn = burn_b.saturating_add(pool.saturating_sub(paid));
    Ok(Allocation {
        burn,
        gateway_fee,
        shares,
        work,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn job_kind_discriminants_are_fixed() {
        let all = [
            (JobKind::Inference, 0u8),
            (JobKind::Eval, 1),
            (JobKind::DataClean, 2),
            (JobKind::Embed, 3),
            (JobKind::Rl, 16),
            (JobKind::Finetune, 17),
            (JobKind::Pretrain, 18),
            (JobKind::Storage, 32),
        ];
        for (kind, byte) in all {
            assert_eq!(kind.encode(), [byte]);
            assert_eq!(kind as u8, byte);
            assert_eq!(JobKind::decode(&mut &[byte][..]), Ok(kind));
        }
        assert!(JobKind::decode(&mut &[4u8][..]).is_err());
        assert!(JobKind::Inference.is_supported());
        assert!(!JobKind::Eval.is_supported());
    }

    fn usd(values: &[u128]) -> Vec<MicroUsd> {
        values.iter().copied().map(MicroUsd).collect()
    }

    // Scenario "分配结果".
    #[test]
    fn allocation_example() {
        let a = allocate(1_000_000, 2_000, 500, 5_000, &usd(&[300, 100])).unwrap();
        assert_eq!(a.burn, 200_000);
        assert_eq!(a.gateway_fee, 50_000);
        assert_eq!(a.shares, [562_500, 187_500]);
        // Scenario "工作量计算": G_i = 750,000 → 375,000 at k = 0.5.
        assert_eq!(a.work, [375_000, 125_000]);
    }

    // Scenario "取整余数销毁": a pool of 10 over three equal entries.
    #[test]
    fn the_rounding_remainder_is_burned() {
        // `allocate` only needs b + fee ≤ 1 (the chain's b ≥ 10% is WorkParams' bound): with
        // neither, the pool is exactly g = 10.
        let a = allocate(10, 0, 0, 5_000, &usd(&[1, 1, 1])).unwrap();
        assert_eq!(a.shares, [3, 3, 3]);
        assert_eq!(a.burn, 1);
        assert_eq!(a.gateway_fee, 0);
    }

    #[test]
    fn nothing_to_allocate_and_bad_ratios_are_errors() {
        assert_eq!(
            allocate(10, 2_000, 0, 5_000, &[]),
            Err(AllocError::NothingToAllocate)
        );
        assert_eq!(
            allocate(10, 2_000, 0, 5_000, &usd(&[0, 0])),
            Err(AllocError::NothingToAllocate)
        );
        assert_eq!(
            allocate(10, 9_000, 1_001, 5_000, &usd(&[1])),
            Err(AllocError::BadRatio)
        );
        assert_eq!(
            allocate(10, 2_000, 0, 5_000, &usd(&[u128::MAX, 1])),
            Err(AllocError::Overflow)
        );
    }

    // Scenario "参数越界" and the other bounds.
    #[test]
    fn parameter_bounds() {
        assert_eq!(WorkParams::LIVE.validate(), Ok(()));
        let p = |burn_bps, emission_bps| WorkParams {
            burn_bps,
            emission_bps,
            ..WorkParams::LIVE
        };
        assert_eq!(
            p(1_000, 7_000).validate(),
            Err(WorkParamsError::EmissionAboveBurn)
        );
        assert_eq!(p(1_000, 6_000).validate(), Ok(()));
        assert_eq!(p(999, 0).validate(), Err(WorkParamsError::BurnOutOfRange));
        assert_eq!(p(9_001, 0).validate(), Err(WorkParamsError::BurnOutOfRange));
        assert_eq!(p(9_000, 0).validate(), Ok(()));
        let no_challenge = WorkParams {
            challenge_epochs: 0,
            ..WorkParams::LIVE
        };
        assert_eq!(
            no_challenge.validate(),
            Err(WorkParamsError::NoChallengePeriod)
        );
        let no_retention = WorkParams {
            retention_epochs: 0,
            ..WorkParams::LIVE
        };
        assert_eq!(no_retention.validate(), Err(WorkParamsError::NoRetention));
    }

    proptest::proptest! {
        #[test]
        fn allocation_conserves_and_follows_dollars(
            g in proptest::prelude::any::<u128>(),
            burn_bps in 1_000u32..=9_000,
            fee_bps in 0u32..=500,
            emission_bps in 0u32..=14_000,
            raw in proptest::collection::vec(1u128..=u128::from(u64::MAX), 1..20),
        ) {
            let a = allocate(g, burn_bps, fee_bps, emission_bps, &usd(&raw)).unwrap();
            let total = a
                .shares
                .iter()
                .try_fold(a.burn, |t, s| t.checked_add(*s))
                .and_then(|t| t.checked_add(a.gateway_fee));
            proptest::prop_assert_eq!(total, Some(g));
            // More dollars never earn a smaller share or less work.
            for i in 0..raw.len() {
                for j in 0..raw.len() {
                    if raw[i] > raw[j] {
                        proptest::prop_assert!(a.shares[i] >= a.shares[j]);
                        proptest::prop_assert!(a.work[i] >= a.work[j]);
                    }
                }
            }
        }
    }
}
