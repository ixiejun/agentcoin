//! TOPLOC proofs carried with receipts, their checks (spec `market/toploc` "协议参数",
//! `market/gateway-service` "收据双签与计费回传"), and the audit's pass / fail rule over the
//! comparison of recomputed activations with them (spec `market/toploc` "复核判定规则与阈值").

use ac_primitives::market::ReceiptBody;
use ac_toploc::{Comparison, Params, Phase, ProofPoly, Segment};
use parity_scale_codec::{Decode, Encode};

/// The proof parameters of the market: top-k 128, decode batches of 32, a prefill chunk.
pub const MARKET_PARAMS: Params = Params {
    decode_batching_size: 32,
    topk: 128,
    skip_prefill: false,
};

/// Encoded length of one proof under [`MARKET_PARAMS`]: the modulus and 128 coefficients.
const PROOF_LEN: usize = 2 + 2 * 128;

/// The proofs of one inference, with the parameters they were built with (the parameters
/// are part of the commitment).
#[derive(Clone, Debug, PartialEq, Eq, Encode, Decode)]
pub struct ToplocProofs {
    /// Generated tokens per decode chunk.
    pub decode_batching_size: u32,
    /// Values kept per chunk.
    pub topk: u32,
    /// Whether the prefill is not a chunk of its own.
    pub skip_prefill: bool,
    /// One encoded proof per chunk.
    pub proofs: Vec<Vec<u8>>,
}

impl ToplocProofs {
    /// Wraps built proofs.
    #[must_use]
    pub fn new(params: &Params, proofs: &[ProofPoly]) -> Self {
        Self {
            decode_batching_size: params.decode_batching_size,
            topk: params.topk,
            skip_prefill: params.skip_prefill,
            proofs: proofs.iter().map(ProofPoly::to_bytes).collect(),
        }
    }

    /// The parameters the proofs claim.
    #[must_use]
    pub const fn params(&self) -> Params {
        Params {
            decode_batching_size: self.decode_batching_size,
            topk: self.topk,
            skip_prefill: self.skip_prefill,
        }
    }

    /// The commitment a receipt carries for these proofs.
    ///
    /// # Errors
    ///
    /// [`ToplocError::BadEncoding`] if a proof does not decode.
    pub fn commitment(&self) -> Result<[u8; 32], ToplocError> {
        let proofs = self
            .proofs
            .iter()
            .map(|p| ProofPoly::from_bytes(p))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| ToplocError::BadEncoding)?;
        ac_toploc::commitment(&self.params(), &proofs).map_err(|_| ToplocError::BadEncoding)
    }
}

/// Chunks of an inference that generated `out_tokens` tokens under [`MARKET_PARAMS`]: the
/// prefill, then `out_tokens − 1` decode steps (the last token is not fed back) in batches of
/// 32.
#[must_use]
pub fn expected_chunks(out_tokens: u32) -> usize {
    let steps = out_tokens.saturating_sub(1);
    let batches = steps.div_ceil(MARKET_PARAMS.decode_batching_size);
    usize::try_from(batches).map_or(usize::MAX, |b| b.saturating_add(1))
}

/// Why an engine's segments for a request do not make proofs (spec `market/provider-agent`
/// "接收引擎插件的候选并构造证明"). Never carries request content.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SegmentsError {
    /// No segments, or the prefill values are not the prompt tokens times the hidden size.
    Prefill,
    /// A prefill segment after decode segments: the engine recomputed the request.
    Recomputed,
    /// A decode segment is not exactly one row.
    DecodeRow,
    /// The decode segments are neither the output tokens − 1 nor one more.
    DecodeCount {
        /// Decode segments the output tokens call for.
        expected: usize,
        /// Decode segments given.
        got: usize,
    },
}

impl core::fmt::Display for SegmentsError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Prefill => f.write_str("the prefill segments do not cover the prompt"),
            Self::Recomputed => f.write_str("prefill segments after decode segments"),
            Self::DecodeRow => f.write_str("a decode segment is not one row"),
            Self::DecodeCount { expected, got } => {
                write!(
                    f,
                    "{got} decode segments where the output calls for {expected}"
                )
            }
        }
    }
}

impl std::error::Error for SegmentsError {}

/// The segments an engine's plugin sent for a request (in order; a recomputed request already
/// started over, D71), fitted to the request's usage for [`ac_toploc::build_proofs_from_candidates`]:
/// the prefill segments hold the prompt tokens times `hidden` values, then one decode segment
/// of `hidden` values per output token but the last, which is never fed back. An engine that
/// computed one step more before it knew the request had ended (asynchronous scheduling feeding
/// the end token back, I-022) sent exactly one decode segment more: it is the last output
/// token's row, which no proof covers, and is dropped, so the proofs are those of an engine
/// that did not compute it. Any other count is refused.
///
/// # Errors
///
/// The [`SegmentsError`] describing the first rule the segments break.
pub fn fit_segments(
    mut segments: Vec<Segment>,
    prompt_tokens: u32,
    completion_tokens: u32,
    hidden: u32,
) -> Result<Vec<Segment>, SegmentsError> {
    let prefill_count = segments
        .iter()
        .take_while(|s| s.phase == Phase::Prefill)
        .count();
    let (prefill, decode) = segments.split_at(prefill_count);
    // Sums of u32 lengths in u64 cannot overflow for any realistic count; saturate regardless.
    let prefill_values = prefill
        .iter()
        .fold(0u64, |a, s| a.saturating_add(u64::from(s.len)));
    if prefill.is_empty()
        || prefill_values != u64::from(prompt_tokens).saturating_mul(u64::from(hidden))
    {
        return Err(SegmentsError::Prefill);
    }
    if decode.iter().any(|s| s.phase == Phase::Prefill) {
        return Err(SegmentsError::Recomputed);
    }
    if decode.iter().any(|s| s.len != hidden) {
        return Err(SegmentsError::DecodeRow);
    }
    let expected = usize::try_from(completion_tokens.saturating_sub(1)).unwrap_or(usize::MAX);
    let got = decode.len();
    if got == expected.saturating_add(1) && completion_tokens > 0 {
        // The last output token was fed back: drop its row.
        segments.pop();
    } else if got != expected {
        return Err(SegmentsError::DecodeCount { expected, got });
    }
    Ok(segments)
}

/// Why a receipt's proofs are refused. Never carries request content.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToplocError {
    /// The commitment is not zero but no proofs came with it.
    Missing,
    /// Proofs came with an all-zero commitment.
    Unexpected,
    /// The proofs' parameters are not [`MARKET_PARAMS`].
    WrongParams,
    /// The number of proofs does not fit the output tokens.
    ChunkCount {
        /// Chunks the output tokens call for.
        expected: usize,
        /// Proofs given.
        got: usize,
    },
    /// A proof does not decode or has the wrong length.
    BadEncoding,
    /// The proofs' commitment is not the receipt's.
    CommitmentMismatch,
}

impl core::fmt::Display for ToplocError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Missing => f.write_str("the receipt commits to TOPLOC proofs that are missing"),
            Self::Unexpected => f.write_str("TOPLOC proofs came with an all-zero commitment"),
            Self::WrongParams => {
                f.write_str("TOPLOC proofs use parameters other than the market's")
            }
            Self::ChunkCount { expected, got } => {
                write!(
                    f,
                    "{got} TOPLOC proofs where the output calls for {expected}"
                )
            }
            Self::BadEncoding => f.write_str("a TOPLOC proof is malformed"),
            Self::CommitmentMismatch => {
                f.write_str("the TOPLOC proofs do not match the receipt's commitment")
            }
        }
    }
}

impl std::error::Error for ToplocError {}

/// Checks a receipt's proofs: an all-zero commitment goes with no proofs; otherwise the proofs
/// use [`MARKET_PARAMS`], number [`expected_chunks`] of the receipt's output tokens, each
/// decodes to a full proof, and they commit to the receipt's commitment.
///
/// # Errors
///
/// The [`ToplocError`] describing the first failed check.
pub fn check(body: &ReceiptBody, proofs: Option<&ToplocProofs>) -> Result<(), ToplocError> {
    let zero = body.toploc_commit == [0; 32];
    let proofs = match (zero, proofs) {
        (true, None) => return Ok(()),
        (true, Some(_)) => return Err(ToplocError::Unexpected),
        (false, None) => return Err(ToplocError::Missing),
        (false, Some(p)) => p,
    };
    if proofs.params() != MARKET_PARAMS {
        return Err(ToplocError::WrongParams);
    }
    let expected = expected_chunks(body.out_tokens);
    if proofs.proofs.len() != expected {
        return Err(ToplocError::ChunkCount {
            expected,
            got: proofs.proofs.len(),
        });
    }
    if proofs.proofs.iter().any(|p| p.len() != PROOF_LEN) {
        return Err(ToplocError::BadEncoding);
    }
    if proofs.commitment()? != body.toploc_commit {
        return Err(ToplocError::CommitmentMismatch);
    }
    Ok(())
}

/// Bounds one chunk's comparison must stay within.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChunkBounds {
    /// Most top-k positions whose exponent may differ from the proof's.
    pub exp_mismatches: u32,
    /// Largest mean mantissa error over the positions whose exponent matches, in hundredths
    /// (`mant_err_sum × 100 ≤ mant_mean_centi × mant_count`).
    pub mant_mean_centi: u32,
    /// Largest median mantissa error (the sorted errors' element `⌊n/2⌋`).
    pub mant_median: u8,
}

impl ChunkBounds {
    /// Whether every bound of `self` is at most the same bound of `wider`.
    #[must_use]
    pub const fn within(&self, wider: &Self) -> bool {
        self.exp_mismatches <= wider.exp_mismatches
            && self.mant_mean_centi <= wider.mant_mean_centi
            && self.mant_median <= wider.mant_median
    }
}

/// The audit length band: prompt token counts from `min` to `max`, both included (spec
/// `market/toploc` "复核判定规则与阈值").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PromptBand {
    /// Fewest prompt tokens in the band.
    pub min: u32,
    /// Most prompt tokens in the band.
    pub max: u32,
}

impl PromptBand {
    /// Whether a prompt of `tokens` tokens is in the band.
    #[must_use]
    pub const fn contains(&self, tokens: u32) -> bool {
        self.min <= tokens && tokens <= self.max
    }
}

/// The bounds audits judge by. The prefill chunk (the prompt, which the auditor knows exactly
/// and recomputes the way the provider computed it) has one set of bounds for prompts in the
/// audit length band and one for the others (they may be equal): on GPUs the prefill varies with
/// the prompt's length and the batch's shape. Decode chunks, whose activations an auditor
/// recomputes in a prefill (a different computation path), have the widest. A single audit only
/// tells gross deviations apart (another model, quantization below 8 bits, a changed prompt);
/// 8-bit quantization is left to the statistical judgment per provider, and the bounds never
/// fail an honest inference to catch it (m6-toploc-gpu-calibration). Versioned: an audit verdict names the version
/// it was judged under, and every change of a bound or of the band is a new version, calibrated
/// anew.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Thresholds {
    /// Version of this set of bounds.
    pub version: u16,
    /// The audit length band.
    pub band: PromptBand,
    /// Bounds of the prefill chunk (chunk 0) of a prompt in the band.
    pub prefill: ChunkBounds,
    /// Bounds of the prefill chunk of a prompt outside the band.
    pub prefill_outside: ChunkBounds,
    /// Bounds of every decode chunk.
    pub decode: ChunkBounds,
}

/// The bounds audits judge by. Version 4 (m6-toploc-gpu-calibration design D13, D14): above the
/// honest maximum of the GPU and CPU calibration with a margin, below another model, int4 and a
/// changed prompt; the same prefill bounds on both sides of the band, which audit prompts and the
/// statistical judgment still use. Version 3 (6 / 0.85 / 1 in the band, 15 / 5.00 / 4 outside,
/// 20 / 8.00 / 8 decode) was provisional and is kept only to replay the calibration runs judged
/// under it; version 2 bounded every prefill chunk by 2 / 0.50 / 1.
pub const AUDIT_THRESHOLDS: Thresholds = Thresholds {
    version: 4,
    band: PromptBand { min: 150, max: 300 },
    prefill: ChunkBounds {
        exp_mismatches: 20,
        mant_mean_centi: 600,
        mant_median: 5,
    },
    prefill_outside: ChunkBounds {
        exp_mismatches: 20,
        mant_mean_centi: 600,
        mant_median: 5,
    },
    decode: ChunkBounds {
        exp_mismatches: 28,
        mant_mean_centi: 1_200,
        mant_median: 12,
    },
};

/// The bound a chunk exceeded.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Metric {
    /// Too many exponent mismatches.
    ExpMismatches,
    /// No top-k position has the proof's exponent.
    NoMatchingExponent,
    /// The mean mantissa error is too large.
    MantissaMean,
    /// The median mantissa error is too large.
    MantissaMedian,
    /// There was no chunk to judge.
    NoChunks,
}

impl core::fmt::Display for Metric {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match self {
            Self::ExpMismatches => "exponent mismatches",
            Self::NoMatchingExponent => "no matching exponent",
            Self::MantissaMean => "mean mantissa error",
            Self::MantissaMedian => "median mantissa error",
            Self::NoChunks => "no chunks",
        })
    }
}

/// The verdict on the comparison of an inference's recomputed activations with its proofs.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Judgement {
    /// Every chunk is within the bounds.
    Pass,
    /// A chunk is not.
    Fail {
        /// The first chunk out of bounds (0 is the prefill).
        chunk: usize,
        /// The first bound it exceeds, in the order of [`ChunkBounds`]' fields.
        metric: Metric,
    },
}

/// The bound one chunk exceeds, if any: exponent mismatches, then a matching exponent at
/// all, then the mean (`mant_err_sum × 100 ≤ mant_mean_centi × mant_count`, integers only),
/// then the median.
#[must_use]
pub fn judge_chunk(c: &Comparison, b: &ChunkBounds) -> Option<Metric> {
    if c.exp_mismatches > b.exp_mismatches {
        return Some(Metric::ExpMismatches);
    }
    if c.mant_count == 0 {
        return Some(Metric::NoMatchingExponent);
    }
    let limit = u64::from(b.mant_mean_centi).saturating_mul(u64::from(c.mant_count));
    if u64::from(c.mant_err_sum).saturating_mul(100) > limit {
        return Some(Metric::MantissaMean);
    }
    match c.median_upper {
        Some(m) if m <= b.mant_median => None,
        _ => Some(Metric::MantissaMedian),
    }
}

/// Judges an inference of a `prompt_tokens`-token prompt under [`MARKET_PARAMS`] (chunk 0 is
/// the prefill, judged by the bounds of the prompt's side of the audit length band): it passes
/// if and only if every chunk is within its bounds (and there is one); else it fails on the
/// first chunk out of bounds.
#[must_use]
pub fn judge(comparisons: &[Comparison], t: &Thresholds, prompt_tokens: u32) -> Judgement {
    let prefill = if t.band.contains(prompt_tokens) {
        &t.prefill
    } else {
        &t.prefill_outside
    };
    if comparisons.is_empty() {
        return Judgement::Fail {
            chunk: 0,
            metric: Metric::NoChunks,
        };
    }
    comparisons
        .iter()
        .enumerate()
        .find_map(|(chunk, c)| {
            let bounds = if chunk == 0 { prefill } else { &t.decode };
            judge_chunk(c, bounds).map(|metric| Judgement::Fail { chunk, metric })
        })
        .unwrap_or(Judgement::Pass)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ac_primitives::market::{JobKind, MicroUsd, ModelId};
    use ac_toploc::{Bf16, build_proofs, build_proofs_from_candidates, top_k_candidates};
    use proptest::prelude::*;
    use sp_core::H256;
    use sp_runtime::AccountId32;

    fn body(out_tokens: u32, commit: [u8; 32]) -> ReceiptBody {
        ReceiptBody {
            genesis: H256::repeat_byte(1),
            gateway: AccountId32::new([2; 32]),
            provider: AccountId32::new([3; 32]),
            kind: JobKind::Inference,
            model: ModelId([4; 32]),
            request_id: [5; 32],
            in_tokens: 10,
            out_tokens,
            fee: MicroUsd(1),
            toploc_commit: commit,
            ttft_ms: 1,
            total_ms: 2,
        }
    }

    /// Proofs under `params` for a prefill and `out − 1` decode steps.
    fn proofs(params: &Params, out: u32) -> ToplocProofs {
        let n = |seed: u16, len: u16| -> Vec<Bf16> {
            (0..len)
                .map(|i| Bf16(i.wrapping_mul(31).wrapping_add(seed) % 0x7f00))
                .collect()
        };
        let pre = n(1, 400);
        let dec: Vec<Vec<Bf16>> = (0..out.saturating_sub(1))
            .map(|s| n(u16::try_from(s).unwrap_or(0).wrapping_add(2), 200))
            .collect();
        let mut acts: Vec<&[Bf16]> = vec![&pre];
        acts.extend(dec.iter().map(Vec::as_slice));
        ToplocProofs::new(params, &build_proofs(&acts, params).unwrap())
    }

    // Scenario "块数".
    #[test]
    fn chunk_counts() {
        assert_eq!(expected_chunks(65), 3);
        assert_eq!(expected_chunks(1), 1);
        assert_eq!(expected_chunks(0), 1);
        assert_eq!(expected_chunks(33), 2);
        assert_eq!(expected_chunks(34), 3);
    }

    #[test]
    fn valid_proofs_pass() {
        let p = proofs(&MARKET_PARAMS, 40);
        assert_eq!(p.proofs.len(), 3);
        let b = body(40, p.commitment().unwrap());
        assert_eq!(check(&b, Some(&p)), Ok(()));
        // No proofs with a zero commitment.
        assert_eq!(check(&body(40, [0; 32]), None), Ok(()));
    }

    // Scenario "参数不符".
    #[test]
    fn other_parameters_are_refused() {
        let params = Params {
            topk: 16,
            ..MARKET_PARAMS
        };
        let p = proofs(&params, 40);
        let b = body(40, p.commitment().unwrap());
        assert_eq!(check(&b, Some(&p)), Err(ToplocError::WrongParams));
    }

    #[test]
    fn mismatches_are_refused() {
        let p = proofs(&MARKET_PARAMS, 40);
        let commit = p.commitment().unwrap();
        // Another receipt's commitment.
        let mut other = commit;
        other[0] ^= 1;
        assert_eq!(
            check(&body(40, other), Some(&p)),
            Err(ToplocError::CommitmentMismatch)
        );
        // Proofs with a zero commitment, a commitment without proofs.
        assert_eq!(
            check(&body(40, [0; 32]), Some(&p)),
            Err(ToplocError::Unexpected)
        );
        assert_eq!(check(&body(40, commit), None), Err(ToplocError::Missing));
        // Output tokens that call for another number of chunks.
        assert_eq!(
            check(&body(70, commit), Some(&p)),
            Err(ToplocError::ChunkCount {
                expected: 4,
                got: 3
            })
        );
        // A truncated proof.
        let mut bad = p.clone();
        bad.proofs[1].truncate(10);
        assert_eq!(
            check(&body(40, commit), Some(&bad)),
            Err(ToplocError::BadEncoding)
        );
    }

    const B: ChunkBounds = ChunkBounds {
        exp_mismatches: 4,
        mant_mean_centi: 300,
        mant_median: 2,
    };
    /// The same bounds for every chunk; prompts of 10 to 20 tokens are in the band.
    const T: Thresholds = Thresholds {
        version: 9,
        band: PromptBand { min: 10, max: 20 },
        prefill: B,
        prefill_outside: B,
        decode: B,
    };
    /// A prompt in the band of [`T`].
    const IN: u32 = 10;

    fn chunk(exp: u32, sum: u32, count: u32, median: Option<u8>) -> Comparison {
        Comparison {
            exp_mismatches: exp,
            mant_err_sum: sum,
            mant_count: count,
            median_upper: median,
            median_twice: median.map(|m| u16::from(m) * 2),
        }
    }

    // Scenario "全部块在阈值内".
    #[test]
    fn chunks_within_the_bounds_pass() {
        let ok = chunk(4, 30, 124, Some(2));
        assert_eq!(
            judge(&[ok, chunk(0, 0, 128, Some(0)), ok], &T, IN),
            Judgement::Pass
        );
    }

    // Scenario "一块指数不一致过多".
    #[test]
    fn one_chunk_with_too_many_exponent_mismatches_fails() {
        let ok = chunk(0, 0, 128, Some(0));
        assert_eq!(
            judge(&[ok, ok, chunk(5, 0, 123, Some(0)), ok], &T, IN),
            Judgement::Fail {
                chunk: 2,
                metric: Metric::ExpMismatches
            }
        );
    }

    // Scenario "没有指数相同的项".
    #[test]
    fn a_chunk_without_a_matching_exponent_fails() {
        let lax = ChunkBounds {
            exp_mismatches: 128,
            ..B
        };
        let all_wrong = Thresholds {
            prefill: lax,
            prefill_outside: lax,
            decode: lax,
            ..T
        };
        assert_eq!(
            judge(&[chunk(128, 0, 0, None)], &all_wrong, IN),
            Judgement::Fail {
                chunk: 0,
                metric: Metric::NoMatchingExponent
            }
        );
    }

    // Scenario "均值恰好等于上限".
    #[test]
    fn a_mean_equal_to_the_bound_passes() {
        assert_eq!(
            judge(&[chunk(0, 3 * 128, 128, Some(2))], &T, IN),
            Judgement::Pass
        );
        assert_eq!(
            judge(&[chunk(0, 3 * 128 + 1, 128, Some(2))], &T, IN),
            Judgement::Fail {
                chunk: 0,
                metric: Metric::MantissaMean
            }
        );
        assert_eq!(
            judge(&[chunk(0, 0, 128, Some(3))], &T, IN),
            Judgement::Fail {
                chunk: 0,
                metric: Metric::MantissaMedian
            }
        );
        assert_eq!(
            judge(&[], &T, IN),
            Judgement::Fail {
                chunk: 0,
                metric: Metric::NoChunks
            }
        );
    }

    // The prefill chunk is judged by its own, stricter bounds.
    #[test]
    fn the_prefill_has_its_own_bounds() {
        let t = Thresholds {
            prefill: ChunkBounds {
                exp_mismatches: 0,
                mant_mean_centi: 50,
                mant_median: 0,
            },
            ..T
        };
        let slightly_off = chunk(0, 100, 128, Some(0)); // mean 0.78
        assert_eq!(
            judge(&[slightly_off, slightly_off], &t, IN),
            Judgement::Fail {
                chunk: 0,
                metric: Metric::MantissaMean
            }
        );
        let exact = chunk(0, 0, 128, Some(0));
        assert_eq!(judge(&[exact, slightly_off], &t, IN), Judgement::Pass);
        // Half a unit is the bound in hundredths: 64 / 128 = 0.50 passes, 65 / 128 does not.
        assert_eq!(
            judge(&[chunk(0, 64, 128, Some(0))], &t, IN),
            Judgement::Pass
        );
        assert!(matches!(
            judge(&[chunk(0, 65, 128, Some(0))], &t, IN),
            Judgement::Fail { chunk: 0, .. }
        ));
    }

    // Spec "预填充块按严格阈值判定", "区间外的 prompt 用宽松的预填充阈值" and "区间的界含两端"
    // (m6-toploc-gpu-calibration 4.1): a prefill mean between the two prefill bounds fails in
    // the band, both ends included, and passes one token outside it on either side.
    #[test]
    fn the_prefill_bounds_depend_on_the_prompt_length() {
        let t = Thresholds {
            prefill: ChunkBounds {
                exp_mismatches: 0,
                mant_mean_centi: 50,
                mant_median: 1,
            },
            ..T
        };
        let between = chunk(0, 100, 128, Some(1)); // mean 0.78: over 0.50, under 3.00
        let ok = chunk(0, 0, 128, Some(0));
        for tokens in [t.band.min, 15, t.band.max] {
            assert_eq!(
                judge(&[between, ok], &t, tokens),
                Judgement::Fail {
                    chunk: 0,
                    metric: Metric::MantissaMean
                },
                "{tokens} tokens"
            );
        }
        for tokens in [t.band.min - 1, t.band.max + 1, 0] {
            assert_eq!(
                judge(&[between, ok], &t, tokens),
                Judgement::Pass,
                "{tokens} tokens"
            );
        }
        // Decode chunks have their own bounds, whatever the prompt's length.
        let wide = chunk(5, 0, 123, Some(0));
        assert!(matches!(
            judge(&[ok, wide], &t, 15),
            Judgement::Fail { chunk: 1, .. }
        ));
        assert!(matches!(
            judge(&[ok, wide], &t, 9),
            Judgement::Fail { chunk: 1, .. }
        ));
    }

    // The bounds nest: the band's prefill bounds within those outside it, those within the
    // decode bounds (spec "复核判定规则与阈值").
    #[test]
    fn the_market_bounds_nest() {
        let t = AUDIT_THRESHOLDS;
        assert!(t.prefill.within(&t.prefill_outside));
        assert!(t.prefill_outside.within(&t.decode));
        assert!(t.band.min <= t.band.max);
        assert_eq!(t.version, 4);
    }

    // Recomputing the very activations passes; another model's do not.
    #[test]
    fn judging_real_comparisons() {
        let n = |seed: u16| -> Vec<Bf16> {
            (0..600u16)
                .map(|i| Bf16(0x3c00 + (i.wrapping_mul(seed) % 0x0800)))
                .collect()
        };
        let (a, b) = (n(7), n(13));
        let proofs = build_proofs(&[&a, &a], &MARKET_PARAMS).unwrap();
        let same = ac_toploc::compare(&[&a, &a], &proofs, &MARKET_PARAMS).unwrap();
        assert_eq!(
            judge(&same, &AUDIT_THRESHOLDS, AUDIT_THRESHOLDS.band.min),
            Judgement::Pass
        );
        let other = ac_toploc::compare(&[&b, &b], &proofs, &MARKET_PARAMS).unwrap();
        assert!(matches!(
            judge(&other, &AUDIT_THRESHOLDS, AUDIT_THRESHOLDS.band.min),
            Judgement::Fail { .. }
        ));
    }

    #[test]
    fn encoding_round_trips() {
        let p = proofs(&MARKET_PARAMS, 5);
        assert_eq!(ToplocProofs::decode(&mut &p.encode()[..]), Ok(p));
    }

    // At least top-k (128) values per row, so that a chunk of one decode row makes a proof.
    const HIDDEN: u32 = 256;

    /// A segment of `rows` rows of distinct, seed-dependent values.
    fn seg(phase: Phase, rows: u32, seed: u16) -> Segment {
        let len = rows.saturating_mul(HIDDEN);
        let values: Vec<Bf16> = (0..len)
            .map(|i| {
                Bf16(
                    u16::try_from(i)
                        .unwrap()
                        .wrapping_mul(97)
                        .wrapping_add(seed.wrapping_mul(13))
                        % 0x7f00,
                )
            })
            .collect();
        Segment {
            phase,
            len,
            candidates: top_k_candidates(&values, 128),
        }
    }

    /// A prompt of `prompt` tokens in two prefill segments, then `decode` decode segments.
    fn request(prompt: u32, decode: u32) -> Vec<Segment> {
        let half = prompt / 2;
        let mut out = vec![
            seg(Phase::Prefill, half, 1),
            seg(Phase::Prefill, prompt.saturating_sub(half), 2),
        ];
        out.extend((0..decode).map(|d| {
            seg(
                Phase::Decode,
                1,
                u16::try_from(d).unwrap().saturating_add(3),
            )
        }));
        out
    }

    // The encoded size scripts/check-vllm-plugin.py reads the proof count from: 9 bytes of
    // parameters, the count, then each proof's length and its 258 bytes.
    #[test]
    fn the_encoded_size_of_proofs() {
        for chunks in [1usize, 3, 4] {
            let p = ToplocProofs {
                decode_batching_size: 32,
                topk: 128,
                skip_prefill: false,
                proofs: vec![vec![0u8; PROOF_LEN]; chunks],
            };
            assert_eq!(p.encode().len(), 10 + 260 * chunks);
        }
    }

    // Spec market/provider-agent "带证明的收据": n − 1 decode segments are kept as they are.
    #[test]
    fn output_tokens_minus_one_decode_segments_are_kept() {
        let segs = request(10, 19);
        assert_eq!(fit_segments(segs.clone(), 10, 20, HIDDEN), Ok(segs));
    }

    // Spec market/provider-agent "结束符被喂回": exactly one decode segment more drops the last.
    #[test]
    fn one_decode_segment_more_drops_the_last() {
        let segs = request(10, 20);
        let fitted = fit_segments(segs.clone(), 10, 20, HIDDEN).unwrap();
        assert_eq!(fitted.len(), segs.len() - 1);
        assert_eq!(fitted.as_slice(), &segs[..segs.len() - 1]);
        let proofs = build_proofs_from_candidates(&fitted, &MARKET_PARAMS).unwrap();
        assert_eq!(proofs.len(), expected_chunks(20));
    }

    // Spec market/provider-agent "多出不止一个解码段" and "步骤数不符".
    #[test]
    fn other_decode_counts_are_refused() {
        assert_eq!(
            fit_segments(request(10, 11), 10, 10, HIDDEN),
            Err(SegmentsError::DecodeCount {
                expected: 9,
                got: 11
            })
        );
        assert_eq!(
            fit_segments(request(10, 5), 10, 10, HIDDEN),
            Err(SegmentsError::DecodeCount {
                expected: 9,
                got: 5
            })
        );
    }

    #[test]
    fn prefill_rows_and_order_are_checked() {
        assert_eq!(
            fit_segments(request(10, 9), 11, 10, HIDDEN),
            Err(SegmentsError::Prefill)
        );
        assert_eq!(
            fit_segments(Vec::new(), 0, 1, HIDDEN),
            Err(SegmentsError::Prefill)
        );
        let mut recomputed = request(10, 3);
        recomputed.push(seg(Phase::Prefill, 10, 9));
        assert_eq!(
            fit_segments(recomputed, 10, 4, HIDDEN),
            Err(SegmentsError::Recomputed)
        );
        let mut wide = request(10, 3);
        wide.push(seg(Phase::Decode, 2, 9));
        assert_eq!(
            fit_segments(wide, 10, 5, HIDDEN),
            Err(SegmentsError::DecodeRow)
        );
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(32))]
        // m6-toploc-async-stop 1.2: with the end token's row fed back, the fitted segments make
        // the proofs of an engine that did not compute it, byte for byte.
        #[test]
        fn a_fed_back_row_changes_no_proof(prompt in 1u32..40, out in 1u32..80) {
            let plain = request(prompt.max(2), out.saturating_sub(1));
            let mut fed = plain.clone();
            fed.push(seg(Phase::Decode, 1, 999));
            let a = build_proofs_from_candidates(&fit_segments(plain, prompt.max(2), out, HIDDEN).unwrap(), &MARKET_PARAMS).unwrap();
            let b = build_proofs_from_candidates(&fit_segments(fed, prompt.max(2), out, HIDDEN).unwrap(), &MARKET_PARAMS).unwrap();
            prop_assert_eq!(a.len(), expected_chunks(out));
            prop_assert_eq!(a, b);
        }
    }
}
