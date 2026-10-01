//! Proof construction, encoding, comparison and commitment (spec `market/toploc`).

use alloc::vec::Vec;
use parity_scale_codec::Encode;

use crate::field::{self, P};
use crate::{Bf16, Error, Params};

/// Hashing context of the TOPLOC commitment carried by receipts.
pub const TOPLOC_COMMIT_CONTEXT: &str = "agentcoin 2026-09 toploc-commit v1";

/// The proof of one chunk: the modulus the chunk's top-k indices are reduced by, and the
/// coefficients (ascending powers, mod 65,497) of the polynomial through
/// `(index mod modulus, bf16 bits)`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProofPoly {
    /// Largest integer ≤ 65,497 that keeps the indices distinct.
    pub modulus: u16,
    /// Polynomial coefficients.
    pub coeffs: Vec<u16>,
}

impl ProofPoly {
    /// The reference encoding: big-endian `u16` modulus, then each coefficient as a big-endian
    /// `u16`.
    #[must_use]
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.coeffs.len().saturating_mul(2).saturating_add(2));
        out.extend_from_slice(&self.modulus.to_be_bytes());
        for c in &self.coeffs {
            out.extend_from_slice(&c.to_be_bytes());
        }
        out
    }

    /// Decodes [`ProofPoly::to_bytes`].
    ///
    /// # Errors
    ///
    /// [`Error::BadEncoding`] for fewer than two bytes or an odd length.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.len() < 2 || !bytes.len().is_multiple_of(2) {
            return Err(Error::BadEncoding);
        }
        let mut words = bytes
            .chunks_exact(2)
            .map(|w| u16::from_be_bytes([*w.first().unwrap_or(&0), *w.get(1).unwrap_or(&0)]));
        let modulus = words.next().ok_or(Error::BadEncoding)?;
        Ok(Self {
            modulus,
            coeffs: words.collect(),
        })
    }
}

/// The largest `m` ≤ 65,497 such that the `indices` are pairwise distinct mod `m`, as the
/// reference searches it (downwards from 65,497).
///
/// # Errors
///
/// [`Error::NoInjectiveModulus`] if none exists (more than 65,497 indices, or duplicates).
pub fn injective_modulus(indices: &[usize]) -> Result<u16, Error> {
    let mut residues = Vec::with_capacity(indices.len());
    for m in (1..=P).rev() {
        let m_usize = usize::try_from(m).map_err(|_| Error::NoInjectiveModulus)?;
        residues.clear();
        residues.extend(indices.iter().map(|i| i.checked_rem(m_usize).unwrap_or(0)));
        residues.sort_unstable();
        if residues.windows(2).all(|w| w.first() != w.get(1)) {
            return u16::try_from(m).map_err(|_| Error::NoInjectiveModulus);
        }
    }
    Err(Error::NoInjectiveModulus)
}

/// `index mod modulus` (a modulus of zero gives zero; callers reject null proofs first).
fn reduce_index(index: usize, modulus: u16) -> u32 {
    index
        .checked_rem(usize::from(modulus))
        .and_then(|r| u32::try_from(r).ok())
        .unwrap_or(0)
}

pub(crate) fn check(params: &Params) -> Result<(usize, usize), Error> {
    let batch = usize::try_from(params.decode_batching_size).map_err(|_| Error::ZeroParameter)?;
    let topk = usize::try_from(params.topk).map_err(|_| Error::ZeroParameter)?;
    if batch == 0 || topk == 0 {
        return Err(Error::ZeroParameter);
    }
    Ok((batch, topk))
}

/// Splits activations into chunks as the reference does: without `skip_prefill` the first
/// activation (the prefill) is its own chunk; the rest are concatenated by
/// `decode_batching_size`.
fn chunks(activations: &[&[Bf16]], params: &Params) -> Result<Vec<Vec<Bf16>>, Error> {
    let (batch, _) = check(params)?;
    let (first, rest) = activations.split_first().ok_or(Error::NoActivations)?;
    let mut out = Vec::new();
    let decode: &[&[Bf16]] = if params.skip_prefill {
        activations
    } else {
        out.push(first.to_vec());
        rest
    };
    for group in decode.chunks(batch) {
        out.push(group.iter().flat_map(|a| a.iter().copied()).collect());
    }
    Ok(out)
}

/// Indices of the `topk` largest magnitudes of `chunk`: larger magnitude first, a lower index
/// first among equal magnitudes (the reference leaves ties to `torch.topk`; only a tie between
/// the k-th and (k+1)-th value changes the result).
fn top_k(chunk: &[Bf16], topk: usize, index: usize) -> Result<Vec<usize>, Error> {
    if chunk.len() < topk {
        return Err(Error::ChunkTooSmall { chunk: index });
    }
    if !chunk.iter().all(|v| v.is_finite()) {
        return Err(Error::NotFinite);
    }
    let mut order: Vec<usize> = (0..chunk.len()).collect();
    order.sort_by(|a, b| {
        let ma = chunk.get(*a).map_or(0, |v| v.magnitude());
        let mb = chunk.get(*b).map_or(0, |v| v.magnitude());
        mb.cmp(&ma).then(a.cmp(b))
    });
    order.truncate(topk);
    Ok(order)
}

/// Builds one proof per chunk of `activations` (spec "证明构造").
///
/// # Errors
///
/// [`Error::ZeroParameter`], [`Error::NoActivations`], [`Error::ChunkTooSmall`],
/// [`Error::NotFinite`], [`Error::NoInjectiveModulus`].
pub fn build_proofs(activations: &[&[Bf16]], params: &Params) -> Result<Vec<ProofPoly>, Error> {
    let (_, topk) = check(params)?;
    chunks(activations, params)?
        .iter()
        .enumerate()
        .map(|(n, chunk)| {
            let idx = top_k(chunk, topk, n)?;
            let points: Vec<(usize, Bf16)> = idx
                .iter()
                .map(|i| (*i, chunk.get(*i).copied().unwrap_or(Bf16(0))))
                .collect();
            prove_points(&points)
        })
        .collect()
}

/// The proof of one chunk from its chosen top-k `(index in chunk, value)` points: the
/// modulus that keeps the indices distinct and the polynomial through
/// `(index mod modulus, bf16 bits)` (the polynomial does not depend on the points' order).
pub(crate) fn prove_points(points: &[(usize, Bf16)]) -> Result<ProofPoly, Error> {
    let idx: Vec<usize> = points.iter().map(|(i, _)| *i).collect();
    let modulus = injective_modulus(&idx)?;
    let x: Vec<u32> = idx.iter().map(|i| reduce_index(*i, modulus)).collect();
    let y: Vec<u32> = points.iter().map(|(_, v)| u32::from(v.0)).collect();
    let coeffs = field::interpolate(&x, &y).ok_or(Error::NoInjectiveModulus)?;
    Ok(ProofPoly {
        modulus,
        coeffs: coeffs
            .into_iter()
            .map(|c| u16::try_from(c).unwrap_or(0))
            .collect(),
    })
}

/// Result of comparing one chunk of recomputed activations with its proof, in integers.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Comparison {
    /// Top-k positions whose bfloat16 exponent differs from the proof's.
    pub exp_mismatches: u32,
    /// Sum of `|mantissa − proof mantissa|` over the positions whose exponent matches.
    pub mant_err_sum: u32,
    /// Number of those positions; the mean mantissa error is `mant_err_sum / mant_count`.
    pub mant_count: u32,
    /// Sorted errors' element `⌊n/2⌋`: the median of the reference's batched (C++) check.
    /// `None` when no exponent matches (the reference reports 2^64).
    pub median_upper: Option<u8>,
    /// Twice the median of the reference's `verify_proofs` (Python `statistics.median`: the
    /// mean of the two middle errors for an even count), so it stays an integer.
    pub median_twice: Option<u16>,
}

/// Compares recomputed `activations` with `proofs` chunk by chunk (spec "校验比对").
///
/// # Errors
///
/// [`Error::ChunkCountMismatch`] if the counts differ, [`Error::NullProof`] for a proof with
/// modulus zero or no coefficients, and the errors of chunking and top-k selection.
pub fn compare(
    activations: &[&[Bf16]],
    proofs: &[ProofPoly],
    params: &Params,
) -> Result<Vec<Comparison>, Error> {
    let (_, topk) = check(params)?;
    let chunks = chunks(activations, params)?;
    if chunks.len() != proofs.len() {
        return Err(Error::ChunkCountMismatch {
            proofs: proofs.len(),
            chunks: chunks.len(),
        });
    }
    chunks
        .iter()
        .zip(proofs)
        .enumerate()
        .map(|(n, (chunk, proof))| {
            let points: Vec<(usize, Bf16)> = top_k(chunk, topk, n)?
                .into_iter()
                .map(|i| (i, chunk.get(i).copied().unwrap_or(Bf16(0))))
                .collect();
            compare_points(&points, proof)
        })
        .collect()
}

/// Compares one chunk's recomputed top-k `(index in chunk, value)` points with its proof.
pub(crate) fn compare_points(
    points: &[(usize, Bf16)],
    proof: &ProofPoly,
) -> Result<Comparison, Error> {
    if proof.modulus == 0 || proof.coeffs.is_empty() {
        return Err(Error::NullProof);
    }
    let coeffs: Vec<u32> = proof.coeffs.iter().map(|c| u32::from(*c)).collect();
    let mut result = Comparison::default();
    let mut errs: Vec<u8> = Vec::new();
    for (i, actual) in points {
        let x = reduce_index(*i, proof.modulus);
        let claimed = field::evaluate(&coeffs, x).ok_or(Error::NullProof)?;
        // Values are below the prime 65,497, so they fit in 16 bits.
        let claimed = Bf16(u16::try_from(claimed).unwrap_or(0));
        if claimed.exponent() == actual.exponent() {
            let e = claimed.mantissa().abs_diff(actual.mantissa());
            errs.push(u8::try_from(e).unwrap_or(u8::MAX));
        } else {
            result.exp_mismatches = result.exp_mismatches.saturating_add(1);
        }
    }
    errs.sort_unstable();
    result.mant_count = u32::try_from(errs.len()).unwrap_or(u32::MAX);
    result.mant_err_sum = errs
        .iter()
        .fold(0u32, |a, e| a.saturating_add(u32::from(*e)));
    let mid = errs.len() / 2;
    result.median_upper = errs.get(mid).copied();
    let upper = result.median_upper.map(u16::from);
    // Odd count: the middle error twice; even count: the two middle errors.
    let lower = if errs.len() % 2 == 1 {
        upper
    } else {
        mid.checked_sub(1)
            .and_then(|i| errs.get(i))
            .map(|e| u16::from(*e))
    };
    result.median_twice = upper.zip(lower).map(|(u, l)| u.saturating_add(l));
    Ok(result)
}

/// The TOPLOC commitment of a receipt (spec "TOPLOC 承诺"):
/// `derive("agentcoin 2026-09 toploc-commit v1", SCALE(batching size u32, top-k u32,
/// skip_prefill bool, [proof encodings]))`.
///
/// # Errors
///
/// Only if the published hashing context were rejected, which does not happen.
pub fn commitment(params: &Params, proofs: &[ProofPoly]) -> Result<[u8; 32], ac_crypto::Error> {
    let encoded: Vec<Vec<u8>> = proofs.iter().map(ProofPoly::to_bytes).collect();
    let data = (
        params.decode_batching_size,
        params.topk,
        params.skip_prefill,
        encoded,
    )
        .encode();
    ac_crypto::hash::derive(TOPLOC_COMMIT_CONTEXT, &data)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;
    use proptest::prelude::*;

    fn bf(values: &[u16]) -> Vec<Bf16> {
        values.iter().copied().map(Bf16).collect()
    }

    fn params(batch: u32, topk: u32, skip_prefill: bool) -> Params {
        Params {
            decode_batching_size: batch,
            topk,
            skip_prefill,
        }
    }

    // The reference's modulus search on small cases (design D11).
    #[test]
    fn modulus_search_matches_the_reference() {
        // Indices below 65,497 are already distinct: the first modulus tried is kept.
        assert_eq!(injective_modulus(&[0, 1, 2, 65_496]), Ok(65_497));
        // 65,497 and 0 collide mod 65,497, 65,496 does not.
        assert_eq!(injective_modulus(&[0, 65_497]), Ok(65_496));
        assert_eq!(injective_modulus(&[3, 3]), Err(Error::NoInjectiveModulus));
        assert_eq!(injective_modulus(&[7]), Ok(65_497));
    }

    // Scenario "编码往返" on a fixed proof, and the reference's layout.
    #[test]
    fn encoding_layout() {
        let p = ProofPoly {
            modulus: 65_497,
            coeffs: vec![0x1b42, 0x2b67],
        };
        assert_eq!(p.to_bytes(), [0xff, 0xd9, 0x1b, 0x42, 0x2b, 0x67]);
        assert_eq!(ProofPoly::from_bytes(&p.to_bytes()), Ok(p));
    }

    // Scenario "截断输入".
    #[test]
    fn truncated_encodings_are_errors() {
        assert_eq!(ProofPoly::from_bytes(&[]), Err(Error::BadEncoding));
        assert_eq!(ProofPoly::from_bytes(&[1]), Err(Error::BadEncoding));
        assert_eq!(ProofPoly::from_bytes(&[1, 2, 3]), Err(Error::BadEncoding));
        // A lone modulus decodes (a proof without coefficients), and is refused when used.
        let bare = ProofPoly::from_bytes(&[0xff, 0xd9]).unwrap();
        assert!(bare.coeffs.is_empty());
    }

    // Scenario "块太小".
    #[test]
    fn a_chunk_smaller_than_topk_is_an_error() {
        let pre = bf(&[1, 2, 3, 4, 5]);
        let dec = bf(&[1, 2, 3]);
        assert_eq!(
            build_proofs(&[&pre, &dec], &params(1, 4, false)),
            Err(Error::ChunkTooSmall { chunk: 1 })
        );
    }

    #[test]
    fn bad_parameters_and_values_are_errors() {
        let a = bf(&[1, 2, 3, 4]);
        assert_eq!(
            build_proofs(&[&a], &params(0, 1, false)),
            Err(Error::ZeroParameter)
        );
        assert_eq!(
            build_proofs(&[&a], &params(1, 0, false)),
            Err(Error::ZeroParameter)
        );
        assert_eq!(
            build_proofs(&[], &params(1, 1, false)),
            Err(Error::NoActivations)
        );
        // +inf and a NaN.
        for bad in [0x7f80u16, 0xffc1] {
            let v = bf(&[1, 2, bad]);
            assert_eq!(
                build_proofs(&[&v], &params(1, 1, false)),
                Err(Error::NotFinite)
            );
        }
    }

    #[test]
    fn chunking_follows_the_reference() {
        let one = bf(&[1]);
        let acts: Vec<&[Bf16]> = vec![&one; 11];
        // Prefill + 10 decode activations in batches of 3: 1 + 4 chunks.
        let c = chunks(&acts, &params(3, 1, false)).unwrap();
        assert_eq!(c.iter().map(Vec::len).collect::<Vec<_>>(), [1, 3, 3, 3, 1]);
        // Without a prefill chunk: 11 in batches of 3.
        let c = chunks(&acts, &params(3, 1, true)).unwrap();
        assert_eq!(c.iter().map(Vec::len).collect::<Vec<_>>(), [3, 3, 3, 2]);
    }

    #[test]
    fn ties_prefer_the_lower_index() {
        // Magnitudes 5, 5, 5 (one negative): the first two indices are kept.
        let v = bf(&[0x40a0, 0xc0a0, 0x40a0]);
        assert_eq!(top_k(&v, 2, 0), Ok(vec![0, 1]));
    }

    // Scenario "相同激活".
    #[test]
    fn the_same_activations_match_exactly() {
        let pre: Vec<Bf16> = (0..40u16).map(|i| Bf16(0x3f80 + i * 3)).collect();
        let dec: Vec<Bf16> = (0..16u16).map(|i| Bf16(0xbf00 + i * 5)).collect();
        let acts: Vec<&[Bf16]> = vec![&pre, &dec, &dec, &dec];
        let p = params(2, 8, false);
        let proofs = build_proofs(&acts, &p).unwrap();
        assert_eq!(proofs.len(), 3);
        for c in compare(&acts, &proofs, &p).unwrap() {
            assert_eq!(c.exp_mismatches, 0);
            assert_eq!(c.mant_err_sum, 0);
            assert_eq!(c.mant_count, 8);
            assert_eq!((c.median_upper, c.median_twice), (Some(0), Some(0)));
        }
    }

    // Scenario "块数不符".
    #[test]
    fn a_chunk_count_mismatch_is_an_error() {
        let a = bf(&[1, 2, 3, 4]);
        let acts: Vec<&[Bf16]> = vec![&a, &a, &a, &a];
        let p = params(1, 2, false);
        let proofs = build_proofs(&acts, &p).unwrap();
        assert_eq!(
            compare(&acts, &proofs[..3], &p),
            Err(Error::ChunkCountMismatch {
                proofs: 3,
                chunks: 4
            })
        );
    }

    #[test]
    fn null_proofs_are_refused() {
        let a = bf(&[1, 2, 3, 4]);
        let p = params(1, 2, false);
        let null = ProofPoly {
            modulus: 0,
            coeffs: vec![0, 0],
        };
        assert_eq!(compare(&[&a], &[null], &p), Err(Error::NullProof));
        let bare = ProofPoly {
            modulus: 65_497,
            coeffs: vec![],
        };
        assert_eq!(compare(&[&a], &[bare], &p), Err(Error::NullProof));
    }

    // No matching exponent: no mantissa statistics (the reference reports 2^64).
    #[test]
    fn no_matching_exponent_gives_no_median() {
        let a = bf(&[0x3f80, 0x4000]);
        let b = bf(&[0x4080, 0x4100]);
        let p = params(1, 2, false);
        let proofs = build_proofs(&[&a], &p).unwrap();
        let c = compare(&[&b], &proofs, &p).unwrap();
        assert_eq!(c[0].exp_mismatches, 2);
        assert_eq!(
            (c[0].mant_count, c[0].median_upper, c[0].median_twice),
            (0, None, None)
        );
    }

    // Both medians on an even count: errors 1, 2, 3, 4 give ⌊n/2⌋-th = 3 and 2 × 2.5 = 5.
    #[test]
    fn both_medians() {
        let a = bf(&[0x3f80, 0x3f80, 0x3f80, 0x3f80]);
        let b = bf(&[0x3f81, 0x3f82, 0x3f83, 0x3f84]);
        let p = params(1, 4, false);
        let proofs = build_proofs(&[&a], &p).unwrap();
        let c = compare(&[&b], &proofs, &p).unwrap()[0];
        assert_eq!((c.mant_err_sum, c.mant_count), (10, 4));
        assert_eq!((c.median_upper, c.median_twice), (Some(3), Some(5)));
    }

    // Scenarios "承诺可复算" and "参数参与承诺".
    #[test]
    fn commitments() {
        let a = bf(&[1, 2, 3, 4]);
        let p = params(1, 2, false);
        let proofs = build_proofs(&[&a], &p).unwrap();
        let c = commitment(&p, &proofs).unwrap();
        assert_eq!(commitment(&p, &proofs).unwrap(), c);
        assert_ne!(commitment(&params(1, 3, false), &proofs).unwrap(), c);
        assert_ne!(commitment(&params(2, 2, false), &proofs).unwrap(), c);
        assert_ne!(commitment(&params(1, 2, true), &proofs).unwrap(), c);
        assert_ne!(commitment(&p, &[]).unwrap(), c);
    }

    proptest! {
        // Scenario "编码往返".
        #[test]
        fn encoding_round_trips(modulus in any::<u16>(), coeffs in proptest::collection::vec(any::<u16>(), 0..64)) {
            let p = ProofPoly { modulus, coeffs };
            prop_assert_eq!(ProofPoly::from_bytes(&p.to_bytes()), Ok(p));
        }

        #[test]
        fn odd_lengths_never_decode(bytes in proptest::collection::vec(any::<u8>(), 0..64)) {
            if bytes.len() < 2 || bytes.len() % 2 == 1 {
                prop_assert_eq!(ProofPoly::from_bytes(&bytes), Err(Error::BadEncoding));
            }
        }

        // Any finite activations prove and match themselves.
        #[test]
        fn proofs_match_their_own_activations(
            raw in proptest::collection::vec(0u16..0x7f80, 8..200),
            negate in proptest::collection::vec(any::<bool>(), 200),
            topk in 1u32..8,
        ) {
            let v: Vec<Bf16> = raw
                .iter()
                .zip(&negate)
                .map(|(m, n)| Bf16(if *n { m | 0x8000 } else { *m }))
                .collect();
            let p = params(1, topk, true);
            let proofs = build_proofs(&[&v], &p).unwrap();
            let c = compare(&[&v], &proofs, &p).unwrap();
            prop_assert_eq!(c[0].exp_mismatches, 0);
            prop_assert_eq!(c[0].mant_err_sum, 0);
        }
    }
}
