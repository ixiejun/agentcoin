//! The statistical judgment of audits (OpenSpec change m6-audit-sprt design D1–D5; spec
//! `market/audit` "统计判定").
//!
//! A single audit only catches gross deviations; subtler ones (8-bit weights, also only while
//! decoding) show as a shift of the mantissa errors that a CUSUM over a provider's verdicts
//! accumulates. Every verdict judged by the thresholds carries [`AuditStats`]; the chain looks up
//! its log-likelihood ratio in a versioned [`StatsParams`] table and keeps a [`SprtState`] per
//! provider. Everything is integer arithmetic, so anyone can replay a provider's state from the
//! verdicts on chain.

use parity_scale_codec::{Decode, DecodeWithMemTracking, Encode, MaxEncodedLen};
use scale_info::TypeInfo;
use sp_core::ConstU32;
use sp_runtime::BoundedVec;

use super::RoundIndex;

/// Highest value of a statistic: a larger one, or a chunk without a matching exponent, saturates
/// to it.
pub const STAT_MAX: u16 = u16::MAX;

/// Most verdicts a provider's state takes part of: the storage bound of [`SprtState::entries`];
/// a parameter version's `max_entries` is at most this.
pub const MAX_SPRT_ENTRIES: u32 = 128;

/// Mantissa errors of one chunk of a re-check: the sum and the number of terms (the positions
/// whose exponent matches), as `ac-toploc` compares them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChunkMantissa {
    /// Sum of the mantissa errors.
    pub err_sum: u32,
    /// Number of terms in the sum.
    pub count: u32,
}

impl ChunkMantissa {
    /// The chunk's mean mantissa error in hundredths, rounded down; [`STAT_MAX`] when no
    /// exponent matches or the mean is larger.
    #[must_use]
    pub fn mean_centi(&self) -> u16 {
        let scaled = u64::from(self.err_sum).saturating_mul(100);
        scaled
            .checked_div(u64::from(self.count))
            .map_or(STAT_MAX, |m| u16::try_from(m).unwrap_or(STAT_MAX))
    }
}

/// The statistics a verdict judged by the thresholds carries (design D1).
///
/// On-chain encoding type: fields are never reordered or retyped.
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
pub struct AuditStats {
    /// Prompt tokens the re-check rebuilt.
    pub prompt_tokens: u32,
    /// The prefill chunk's mean mantissa error, in hundredths.
    pub prefill_mean_centi: u16,
    /// The decode chunks' mean mantissa errors (each in hundredths, rounded down) averaged,
    /// rounded down; 0 without decode chunks.
    pub decode_mean_centi: u16,
    /// Decode chunks, saturating at [`STAT_MAX`].
    pub decode_chunks: u16,
}

impl AuditStats {
    /// The statistics of a re-check from its prompt token count and its chunks in order (the
    /// prefill chunk first); `None` without chunks.
    #[must_use]
    pub fn from_chunks(prompt_tokens: u32, chunks: &[ChunkMantissa]) -> Option<Self> {
        let (prefill, decode) = chunks.split_first()?;
        let sum = decode
            .iter()
            .fold(0u64, |s, c| s.saturating_add(u64::from(c.mean_centi())));
        let n = u64::try_from(decode.len()).unwrap_or(u64::MAX);
        // The mean of values at most STAT_MAX is at most STAT_MAX.
        let mean = sum
            .checked_div(n)
            .map_or(0, |m| u16::try_from(m).unwrap_or(STAT_MAX));
        Some(Self {
            prompt_tokens,
            prefill_mean_centi: prefill.mean_centi(),
            decode_mean_centi: mean,
            decode_chunks: u16::try_from(decode.len()).unwrap_or(STAT_MAX),
        })
    }

    /// `true` if the statistics can come from a re-check: a prompt of at least one token, and a
    /// decode mean of 0 without decode chunks.
    #[must_use]
    pub fn is_valid(&self) -> bool {
        self.prompt_tokens > 0 && (self.decode_chunks > 0 || self.decode_mean_centi == 0)
    }
}

/// A version of the statistical judgment's parameters (design D2–D5), derived from the
/// calibration by `scripts/export-audit-stats.py`.
///
/// A bin table has one more ratio than edges: bin `j` holds the values from edge `j − 1`
/// (included) to edge `j` (excluded), the first bin everything below the first edge and the last
/// everything from the last edge on.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StatsParams {
    /// The version verdicts name.
    pub version: u16,
    /// The re-check thresholds version whose audit length band this version uses.
    pub thresholds_version: u16,
    /// The audit length band, prompt tokens, both ends included: the prefill statistic only
    /// counts for a prompt in it.
    pub band: (u32, u32),
    /// Bin edges of the prefill statistic, in hundredths, strictly increasing.
    pub prefill_edges: &'static [u16],
    /// Log-likelihood ratio of each prefill bin, in thousandths of a nat.
    pub prefill_llr: &'static [i32],
    /// Bin edges of the decode statistic, in hundredths, strictly increasing.
    pub decode_edges: &'static [u16],
    /// Log-likelihood ratio of each decode bin, in thousandths of a nat.
    pub decode_llr: &'static [i32],
    /// Lowest and highest contribution of one verdict, in thousandths of a nat.
    pub clamp: (i32, i32),
    /// Most positive contribution one auditor's verdicts in a provider's state count for.
    pub per_auditor_cap: i64,
    /// The bound: a state at or above it opens a statistical dispute.
    pub bound: i64,
    /// Most verdicts a provider's state keeps.
    pub max_entries: u32,
}

/// Version 1 (provisional until the calibration's CPU cells are in, design D3/D4): bins and
/// ratios from the GPU calibration `gpu-cal-v3-2026-10-09` (int8 against the worst honest cell,
/// pseudo-counts 5 and 0.5, rounded down), contributions clamped to −0.5 and +3.0 nats, the
/// bound at 24.7 nats (a false alarm per provider and year at most 52,560 × e^−24.7 ≈ 10⁻⁶ when
/// the honest `E[e^λ]` is at most 1; it is 0.62 at worst).
pub const STATS_V1: StatsParams = StatsParams {
    version: 1,
    thresholds_version: 4,
    band: (150, 300),
    prefill_edges: &[30, 40, 50, 60, 70, 80, 90, 100, 110, 120, 140, 170, 220],
    prefill_llr: &[
        -1_631, -5_609, -6_378, -6_333, -4_959, -2_988, 172, 2_187, 3_777, 4_518, 4_870, 4_639,
        2_537, -408,
    ],
    decode_edges: &[60, 80, 100, 120, 140, 170, 200, 240, 280, 340, 450],
    decode_llr: &[
        -409, -4_390, -6_248, -6_105, -5_256, -2_276, 990, 3_472, 4_911, 4_597, 3_537, 1_018,
    ],
    clamp: (-500, 3_000),
    per_auditor_cap: 8_233,
    bound: 24_700,
    max_entries: MAX_SPRT_ENTRIES,
};

/// Every published version, oldest first; the runtime accepts only these.
pub const AUDIT_STATS: &[StatsParams] = &[STATS_V1];

/// The latest published version.
pub const CURRENT_STATS: StatsParams = STATS_V1;

/// The published parameters of `version`.
#[must_use]
pub fn stats_params(version: u16) -> Option<&'static StatsParams> {
    AUDIT_STATS.iter().find(|p| p.version == version)
}

/// Log-likelihood ratio of `value`'s bin; 0 for a malformed table.
fn lookup(edges: &[u16], llr: &[i32], value: u16) -> i32 {
    let bin = edges.iter().take_while(|e| value >= **e).count();
    llr.get(bin).copied().unwrap_or(0)
}

impl StatsParams {
    /// A verdict's contribution: the prefill bin's ratio for a prompt in the band, plus the
    /// decode bin's ratio when there are decode chunks, clamped (design D2/D3).
    #[must_use]
    pub fn contribution(&self, stats: &AuditStats) -> i32 {
        let (lo, hi) = self.band;
        let prefill = if (lo..=hi).contains(&stats.prompt_tokens) {
            lookup(
                self.prefill_edges,
                self.prefill_llr,
                stats.prefill_mean_centi,
            )
        } else {
            0
        };
        let decode = if stats.decode_chunks > 0 {
            lookup(self.decode_edges, self.decode_llr, stats.decode_mean_centi)
        } else {
            0
        };
        prefill
            .saturating_add(decode)
            .clamp(self.clamp.0, self.clamp.1)
    }
}

/// Whether the statistical judgment opens disputes and the parameter version verdicts must use
/// (design D10).
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
pub struct StatsConfig {
    /// The parameter version verdicts must use.
    pub version: u16,
    /// Whether crossing the bound opens a statistical dispute.
    pub enabled: bool,
}

/// One verdict in a provider's state: who submitted it, in which round, and its contribution
/// (clamped, before the per-auditor cap).
#[derive(
    Clone, Debug, PartialEq, Eq, Encode, Decode, DecodeWithMemTracking, MaxEncodedLen, TypeInfo,
)]
pub struct SprtEntry<AccountId> {
    /// The auditor.
    pub auditor: AccountId,
    /// The verdict's round.
    pub round: RoundIndex,
    /// Its contribution, in thousandths of a nat.
    pub contribution: i32,
}

/// A provider's CUSUM (design D3, D5, D6): the cumulative value and the verdicts since it was
/// last 0, oldest first.
#[derive(
    Clone, Debug, PartialEq, Eq, Encode, Decode, DecodeWithMemTracking, MaxEncodedLen, TypeInfo,
)]
pub struct SprtState<AccountId> {
    /// The cumulative value, in thousandths of a nat; never negative.
    pub cumulative: i64,
    /// The verdicts it is made of.
    pub entries: BoundedVec<SprtEntry<AccountId>, ConstU32<MAX_SPRT_ENTRIES>>,
}

impl<A> Default for SprtState<A> {
    fn default() -> Self {
        Self {
            cumulative: 0,
            entries: BoundedVec::new(),
        }
    }
}

impl<A: PartialEq + Clone> SprtState<A> {
    /// What `entry` counts for: a positive contribution only up to what its auditor's earlier
    /// entries leave of the per-auditor cap.
    fn counted(&self, params: &StatsParams, entry: &SprtEntry<A>) -> i64 {
        let c = i64::from(entry.contribution);
        if c <= 0 {
            return c;
        }
        let used = self
            .entries
            .iter()
            .filter(|e| e.auditor == entry.auditor)
            .fold(0i64, |used, e| {
                let room = params.per_auditor_cap.saturating_sub(used).max(0);
                used.saturating_add(i64::from(e.contribution).clamp(0, room))
            });
        c.min(params.per_auditor_cap.saturating_sub(used).max(0))
    }

    /// Adds one verdict without trimming: `S ← max(0, S + counted)`, and a state back at 0
    /// forgets its entries.
    fn push(&mut self, params: &StatsParams, entry: SprtEntry<A>) {
        let next = self
            .cumulative
            .saturating_add(self.counted(params, &entry))
            .max(0);
        if next == 0 {
            self.reset();
            return;
        }
        self.cumulative = next;
        if self.entries.try_push(entry).is_err() {
            // Only reachable with a `max_entries` above the storage bound; the oldest entry
            // gives way as when the list is full.
            self.reset();
        }
    }

    /// Records a verdict's entry (spec "统计判定"): adds its counted contribution and, when the
    /// list is then longer than `max_entries`, drops the oldest entry and replays the rest.
    pub fn record(&mut self, params: &StatsParams, entry: SprtEntry<A>) {
        let max = usize::try_from(params.max_entries.min(MAX_SPRT_ENTRIES)).unwrap_or(0);
        if self.entries.len() >= max && max > 0 {
            // Room for the new entry first: the full list loses its oldest one.
            let kept: alloc::vec::Vec<_> = self.entries.iter().skip(1).cloned().collect();
            *self = Self::replay(params, kept);
        }
        self.push(params, entry);
    }

    /// The state the entries give from 0, in order (spec "复算累计值").
    #[must_use]
    pub fn replay(params: &StatsParams, entries: impl IntoIterator<Item = SprtEntry<A>>) -> Self {
        let mut s = Self::default();
        for e in entries {
            s.push(params, e);
        }
        s
    }

    /// `true` if the state is at or above the bound.
    #[must_use]
    pub fn crossed(&self, params: &StatsParams) -> bool {
        self.cumulative >= params.bound
    }

    /// Back to 0 without entries (after a statistical dispute opens, or on a version change).
    pub fn reset(&mut self) {
        self.cumulative = 0;
        self.entries = BoundedVec::new();
    }
}

#[cfg(test)]
mod tests {
    // Test code: AGENT.md §5.3 permits unwrap, indexing and plain arithmetic.
    #![allow(
        clippy::unwrap_used,
        clippy::indexing_slicing,
        clippy::arithmetic_side_effects
    )]

    use super::*;

    fn chunk(err_sum: u32, count: u32) -> ChunkMantissa {
        ChunkMantissa { err_sum, count }
    }

    #[test]
    fn chunk_means_round_down_and_saturate() {
        assert_eq!(chunk(10, 3).mean_centi(), 333);
        assert_eq!(chunk(2, 3).mean_centi(), 66);
        assert_eq!(chunk(0, 5).mean_centi(), 0);
        // No matching exponent: the maximum.
        assert_eq!(chunk(0, 0).mean_centi(), STAT_MAX);
        // 700 × 100 / 1 is above the maximum.
        assert_eq!(chunk(700, 1).mean_centi(), STAT_MAX);
        assert_eq!(chunk(u32::MAX, 1).mean_centi(), STAT_MAX);
        assert_eq!(chunk(655, 1).mean_centi(), 65_500);
    }

    // Design D1: prefill from chunk 0, decode means averaged rounding down, 0 without decode.
    #[test]
    fn statistics_from_chunks() {
        let s = AuditStats::from_chunks(219, &[chunk(51, 100), chunk(1, 1), chunk(2, 1)]).unwrap();
        assert_eq!(
            s,
            AuditStats {
                prompt_tokens: 219,
                prefill_mean_centi: 51,
                decode_mean_centi: 150,
                decode_chunks: 2,
            }
        );
        // (100 + 101) / 2 rounds down.
        let s = AuditStats::from_chunks(10, &[chunk(0, 1), chunk(1, 1), chunk(101, 100)]).unwrap();
        assert_eq!(s.decode_mean_centi, 100);
        let s = AuditStats::from_chunks(10, &[chunk(3, 1)]).unwrap();
        assert_eq!((s.decode_mean_centi, s.decode_chunks), (0, 0));
        assert!(AuditStats::from_chunks(10, &[]).is_none());
        // A decode chunk without a matching exponent counts at the maximum.
        let s = AuditStats::from_chunks(10, &[chunk(0, 1), chunk(0, 0), chunk(0, 0)]).unwrap();
        assert_eq!(s.decode_mean_centi, STAT_MAX);
    }

    #[test]
    fn decode_chunks_saturate() {
        let chunks = alloc::vec![chunk(1, 1); 70_000];
        let s = AuditStats::from_chunks(1, &chunks).unwrap();
        assert_eq!(s.decode_chunks, STAT_MAX);
        assert_eq!(s.decode_mean_centi, 100);
    }

    #[test]
    fn validity() {
        let ok = AuditStats {
            prompt_tokens: 200,
            prefill_mean_centi: 50,
            decode_mean_centi: 100,
            decode_chunks: 3,
        };
        assert!(ok.is_valid());
        assert!(
            !AuditStats {
                prompt_tokens: 0,
                ..ok
            }
            .is_valid()
        );
        assert!(
            !AuditStats {
                decode_chunks: 0,
                ..ok
            }
            .is_valid()
        );
        assert!(
            AuditStats {
                decode_chunks: 0,
                decode_mean_centi: 0,
                ..ok
            }
            .is_valid()
        );
    }

    #[test]
    fn published_versions_are_well_formed() {
        let mut last = 0;
        for p in AUDIT_STATS {
            assert!(p.version > last, "versions increase");
            last = p.version;
            assert!(p.band.0 <= p.band.1);
            for (edges, llr) in [
                (p.prefill_edges, p.prefill_llr),
                (p.decode_edges, p.decode_llr),
            ] {
                assert!(!edges.is_empty());
                assert!(edges.windows(2).all(|w| w[0] < w[1]), "edges increase");
                assert_eq!(llr.len(), edges.len() + 1);
            }
            assert!(p.clamp.0 <= 0 && 0 < p.clamp.1);
            assert_eq!(p.per_auditor_cap, p.bound / 3);
            assert!(p.per_auditor_cap > i64::from(p.clamp.1));
            assert!(p.max_entries > 0 && p.max_entries <= MAX_SPRT_ENTRIES);
            assert_eq!(stats_params(p.version), Some(p));
        }
        assert_eq!(AUDIT_STATS.last(), Some(&CURRENT_STATS));
        assert_eq!(stats_params(0), None);
    }

    /// Parameters with round numbers for the scenarios.
    const P: StatsParams = StatsParams {
        version: 1,
        thresholds_version: 4,
        band: (150, 300),
        prefill_edges: &[50, 100],
        prefill_llr: &[-400, 1_200, 5_000],
        decode_edges: &[100, 200],
        decode_llr: &[-1_800, 900, 2_000],
        clamp: (-500, 3_000),
        per_auditor_cap: 6_000,
        bound: 18_000,
        max_entries: 4,
    };

    fn stats(prompt_tokens: u32, prefill: u16, decode: u16) -> AuditStats {
        AuditStats {
            prompt_tokens,
            prefill_mean_centi: prefill,
            decode_mean_centi: decode,
            decode_chunks: 3,
        }
    }

    // Spec "统计判定" scenario "贡献按查表计算": +1,200 and +900 under a cap of 3,000 give +2,100.
    #[test]
    fn contribution_is_looked_up() {
        assert_eq!(P.contribution(&stats(200, 60, 150)), 2_100);
        let mut s = SprtState::default();
        s.record(&P, entry(1, 2_100));
        assert_eq!(s.cumulative, 2_100);
    }

    #[test]
    fn bins_include_their_lower_edge() {
        assert_eq!(lookup(P.prefill_edges, P.prefill_llr, 49), -400);
        assert_eq!(lookup(P.prefill_edges, P.prefill_llr, 50), 1_200);
        assert_eq!(lookup(P.prefill_edges, P.prefill_llr, 100), 5_000);
        assert_eq!(lookup(P.prefill_edges, P.prefill_llr, STAT_MAX), 5_000);
        assert_eq!(lookup(&[10], &[], 5), 0);
    }

    // Scenario "区间外的 prompt 不计预填充": one token below the band.
    #[test]
    fn prompt_outside_the_band_counts_decode_only() {
        assert_eq!(P.contribution(&stats(149, 60, 150)), 900);
        assert_eq!(P.contribution(&stats(301, 60, 150)), 900);
        assert_eq!(P.contribution(&stats(150, 60, 150)), 2_100);
        assert_eq!(P.contribution(&stats(300, 60, 150)), 2_100);
        // Without decode chunks only the prefill counts.
        let s = AuditStats {
            decode_chunks: 0,
            decode_mean_centi: 0,
            ..stats(200, 60, 0)
        };
        assert_eq!(P.contribution(&s), 1_200);
    }

    // Scenario "单条贡献截断".
    #[test]
    fn contribution_is_clamped() {
        assert_eq!(P.contribution(&stats(200, 100, 250)), 3_000);
        assert_eq!(P.contribution(&stats(200, 10, 50)), -500);
    }

    fn entry(auditor: u8, contribution: i32) -> SprtEntry<u8> {
        SprtEntry {
            auditor,
            round: 0,
            contribution,
        }
    }

    // Scenario "累计值不低于零": 500 then −2,000 gives 0 and an empty list.
    #[test]
    fn cumulative_never_goes_below_zero() {
        let mut s = SprtState::default();
        s.record(&P, entry(1, 500));
        assert_eq!(s.entries.len(), 1);
        s.record(&P, entry(2, -2_000));
        assert_eq!(s.cumulative, 0);
        assert!(s.entries.is_empty());
        // A negative contribution from 0 leaves no entry either.
        s.record(&P, entry(2, -100));
        assert_eq!(s, SprtState::default());
    }

    // Scenario "单个审计员的贡献有上限".
    #[test]
    fn one_auditor_counts_up_to_the_cap() {
        let mut s = SprtState::default();
        s.record(&P, entry(1, 3_000));
        s.record(&P, entry(1, 2_000));
        s.record(&P, entry(1, 3_000));
        // 3,000 + 2,000 + 1,000 of the last one.
        assert_eq!(s.cumulative, 6_000);
        s.record(&P, entry(1, 3_000));
        assert_eq!(s.cumulative, 6_000);
        assert_eq!(s.entries.len(), 4);
        // Another auditor still counts in full.
        let mut s = SprtState::default();
        s.record(&P, entry(1, 3_000));
        s.record(&P, entry(1, 3_000));
        s.record(&P, entry(2, 3_000));
        assert_eq!(s.cumulative, 9_000);
    }

    // A negative contribution does not free an auditor's cap.
    #[test]
    fn negative_entries_leave_the_cap_used() {
        let mut s = SprtState::default();
        s.record(&P, entry(1, 3_000));
        s.record(&P, entry(1, 3_000));
        s.record(&P, entry(1, -500));
        s.record(&P, entry(1, 3_000));
        assert_eq!(s.cumulative, 5_500);
    }

    #[test]
    fn crossing_needs_three_auditors() {
        let p = StatsParams {
            max_entries: MAX_SPRT_ENTRIES,
            ..P
        };
        let mut s = SprtState::default();
        for a in [1, 2] {
            for _ in 0..5 {
                s.record(&p, entry(a, 3_000));
            }
        }
        assert!(!s.crossed(&p));
        assert_eq!(s.cumulative, 12_000);
        let mut s = SprtState::default();
        for a in [1, 2, 3] {
            for _ in 0..2 {
                s.record(&p, entry(a, 3_000));
            }
        }
        assert!(s.crossed(&p));
        s.reset();
        assert_eq!(s, SprtState::default());
    }

    // A full list drops its oldest entry and replays the rest before taking the new one.
    #[test]
    fn full_list_drops_the_oldest() {
        let mut s = SprtState::default();
        for (a, c) in [(1, 1_000), (2, -500), (3, 2_000), (4, 3_000)] {
            s.record(&P, entry(a, c));
        }
        assert_eq!(s.cumulative, 5_500);
        // The oldest (+1,000) leaves: replaying −500 from 0 forgets it, then +2,000 and +3,000.
        s.record(&P, entry(5, 100));
        assert_eq!(s.cumulative, 5_100);
        assert_eq!(
            s.entries
                .iter()
                .map(|e| e.auditor)
                .collect::<alloc::vec::Vec<_>>(),
            [3, 4, 5]
        );
        assert_eq!(s, SprtState::replay(&P, s.entries.clone()));
    }

    // Spec "复算累计值": the state recorded verdict by verdict equals the replay of its list.
    proptest::proptest! {
        #[test]
        fn recorded_state_equals_its_replay(
            steps in proptest::collection::vec((0u8..5, -600i32..3_200), 0..300),
            max in 1u32..=8,
        ) {
            let p = StatsParams { max_entries: max, ..P };
            let mut s = SprtState::default();
            for (a, c) in steps {
                s.record(&p, entry(a, c.clamp(p.clamp.0, p.clamp.1)));
                proptest::prop_assert!(s.cumulative >= 0);
                proptest::prop_assert!(s.entries.len() <= max as usize);
                proptest::prop_assert_eq!(s.cumulative == 0, s.entries.is_empty());
                proptest::prop_assert_eq!(&s, &SprtState::replay(&p, s.entries.clone()));
            }
        }
    }

    #[test]
    fn encodings_are_stable() {
        let s = AuditStats {
            prompt_tokens: 0x0102_0304,
            prefill_mean_centi: 0x0506,
            decode_mean_centi: 0x0708,
            decode_chunks: 0x090a,
        };
        assert_eq!(s.encode(), [4, 3, 2, 1, 6, 5, 8, 7, 10, 9]);
        assert_eq!(
            StatsConfig {
                version: 1,
                enabled: true
            }
            .encode(),
            [1, 0, 1]
        );
    }
}
