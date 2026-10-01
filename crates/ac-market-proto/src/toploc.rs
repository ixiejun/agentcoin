//! TOPLOC proofs carried with receipts, their checks (spec `market/toploc` "协议参数",
//! `market/gateway-service` "收据双签与计费回传"), and the audit's pass / fail rule over the
//! comparison of recomputed activations with them (spec `market/toploc` "复核判定规则与阈值").

use ac_primitives::market::ReceiptBody;
use ac_toploc::{Comparison, Params, ProofPoly};
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

/// Bounds a chunk's comparison must stay within for an audit to pass it. Versioned: an audit
/// verdict names the version it was judged under, and every change of a bound is a new
/// version, calibrated anew.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Thresholds {
    /// Version of this set of bounds.
    pub version: u16,
    /// Most top-k positions whose exponent may differ from the proof's.
    pub exp_mismatches: u32,
    /// Largest mean mantissa error over the positions whose exponent matches.
    pub mant_mean: u32,
    /// Largest median mantissa error (the sorted errors' element `⌊n/2⌋`).
    pub mant_median: u8,
}

/// The bounds audits judge by. Version 1 is provisional, to be replaced by the values of the
/// calibration run (m6-toploc-verify group 8).
pub const AUDIT_THRESHOLDS: Thresholds = Thresholds {
    version: 1,
    exp_mismatches: 16,
    mant_mean: 10,
    mant_median: 8,
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
        /// The first bound it exceeds, in the order of [`Thresholds`]' fields.
        metric: Metric,
    },
}

/// The bound one chunk exceeds, if any: exponent mismatches, then a matching exponent at
/// all, then the mean (`mant_err_sum ≤ mant_mean × mant_count`, integers only), then the
/// median.
#[must_use]
pub fn judge_chunk(c: &Comparison, t: &Thresholds) -> Option<Metric> {
    if c.exp_mismatches > t.exp_mismatches {
        return Some(Metric::ExpMismatches);
    }
    if c.mant_count == 0 {
        return Some(Metric::NoMatchingExponent);
    }
    let limit = u64::from(t.mant_mean).saturating_mul(u64::from(c.mant_count));
    if u64::from(c.mant_err_sum) > limit {
        return Some(Metric::MantissaMean);
    }
    match c.median_upper {
        Some(m) if m <= t.mant_median => None,
        _ => Some(Metric::MantissaMedian),
    }
}

/// Judges an inference: it passes if and only if every chunk does (and there is one); else it
/// fails on the first chunk out of bounds.
#[must_use]
pub fn judge(comparisons: &[Comparison], t: &Thresholds) -> Judgement {
    if comparisons.is_empty() {
        return Judgement::Fail {
            chunk: 0,
            metric: Metric::NoChunks,
        };
    }
    comparisons
        .iter()
        .enumerate()
        .find_map(|(chunk, c)| judge_chunk(c, t).map(|metric| Judgement::Fail { chunk, metric }))
        .unwrap_or(Judgement::Pass)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ac_primitives::market::{JobKind, MicroUsd, ModelId};
    use ac_toploc::{Bf16, build_proofs};
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

    const T: Thresholds = Thresholds {
        version: 9,
        exp_mismatches: 4,
        mant_mean: 3,
        mant_median: 2,
    };

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
            judge(&[ok, chunk(0, 0, 128, Some(0)), ok], &T),
            Judgement::Pass
        );
    }

    // Scenario "一块指数不一致过多".
    #[test]
    fn one_chunk_with_too_many_exponent_mismatches_fails() {
        let ok = chunk(0, 0, 128, Some(0));
        assert_eq!(
            judge(&[ok, ok, chunk(5, 0, 123, Some(0)), ok], &T),
            Judgement::Fail {
                chunk: 2,
                metric: Metric::ExpMismatches
            }
        );
    }

    // Scenario "没有指数相同的项".
    #[test]
    fn a_chunk_without_a_matching_exponent_fails() {
        let all_wrong = Thresholds {
            exp_mismatches: 128,
            ..T
        };
        assert_eq!(
            judge(&[chunk(128, 0, 0, None)], &all_wrong),
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
            judge(&[chunk(0, 3 * 128, 128, Some(2))], &T),
            Judgement::Pass
        );
        assert_eq!(
            judge(&[chunk(0, 3 * 128 + 1, 128, Some(2))], &T),
            Judgement::Fail {
                chunk: 0,
                metric: Metric::MantissaMean
            }
        );
        assert_eq!(
            judge(&[chunk(0, 0, 128, Some(3))], &T),
            Judgement::Fail {
                chunk: 0,
                metric: Metric::MantissaMedian
            }
        );
        assert_eq!(
            judge(&[], &T),
            Judgement::Fail {
                chunk: 0,
                metric: Metric::NoChunks
            }
        );
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
        assert_eq!(judge(&same, &AUDIT_THRESHOLDS), Judgement::Pass);
        let other = ac_toploc::compare(&[&b, &b], &proofs, &MARKET_PARAMS).unwrap();
        assert!(matches!(
            judge(&other, &AUDIT_THRESHOLDS),
            Judgement::Fail { .. }
        ));
    }

    #[test]
    fn encoding_round_trips() {
        let p = proofs(&MARKET_PARAMS, 5);
        assert_eq!(ToplocProofs::decode(&mut &p.encode()[..]), Ok(p));
    }
}
