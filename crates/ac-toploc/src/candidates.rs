//! Proofs from per-segment top-k candidates (spec `market/toploc` "由候选构造证明").
//!
//! An inference engine plugin does not ship whole activations: for every forward step it
//! keeps, per request, the top-k values of that step's activations (a *segment*). The top-k
//! of a chunk always lies in the union of its segments' top-k — a value outside its segment's
//! top-k has k values at least as large in that segment alone — so merging the candidates
//! gives the same proof as [`build_proofs`](crate::build_proofs) on the whole activations.

use alloc::vec::Vec;

use crate::proof::{check, prove_points};
use crate::{Bf16, Error, Params, ProofPoly};

/// Which part of an inference a segment belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Phase {
    /// Prompt tokens (a prefill split over several steps gives several segments).
    Prefill,
    /// One decode step.
    Decode,
}

/// One top-k candidate: its index within the segment and its value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Candidate {
    /// Index within the segment's flattened activations.
    pub index: u32,
    /// The activation.
    pub value: Bf16,
}

/// The candidates of one forward step of one request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Segment {
    /// Prefill or decode.
    pub phase: Phase,
    /// Number of activations in the segment (tokens × hidden size).
    pub len: u32,
    /// The segment's top `min(k, len)` values by magnitude, in any order.
    pub candidates: Vec<Candidate>,
}

/// Checks one segment's candidates: `min(topk, len)` of them, indices below `len` and
/// distinct, values finite.
fn check_segment(segment: &Segment, topk: usize, n: usize) -> Result<usize, Error> {
    let bad = Error::BadCandidates { segment: n };
    let len = usize::try_from(segment.len).map_err(|_| bad)?;
    if segment.candidates.len() != topk.min(len) {
        return Err(bad);
    }
    let mut indices: Vec<u32> = Vec::with_capacity(segment.candidates.len());
    for c in &segment.candidates {
        if c.index >= segment.len {
            return Err(bad);
        }
        if !c.value.is_finite() {
            return Err(Error::NotFinite);
        }
        indices.push(c.index);
    }
    indices.sort_unstable();
    if indices.windows(2).any(|w| w.first() == w.get(1)) {
        return Err(bad);
    }
    Ok(len)
}

/// Builds one proof per chunk from the segments of one inference, in the order the engine
/// computed them: the prefill segments (together, the prefill activation) and then one
/// segment per decode step. The chunks are those of [`build_proofs`](crate::build_proofs); a
/// segment's indices are offset by the lengths of the segments before it in its chunk.
///
/// # Errors
///
/// [`Error::ZeroParameter`], [`Error::NoActivations`] (no segments),
/// [`Error::SegmentOrder`] (a prefill segment after a decode one),
/// [`Error::BadCandidates`], [`Error::NotFinite`], [`Error::ChunkTooSmall`],
/// [`Error::NoInjectiveModulus`].
pub fn build_proofs_from_candidates(
    segments: &[Segment],
    params: &Params,
) -> Result<Vec<ProofPoly>, Error> {
    let (batch, topk) = check(params)?;
    if segments.is_empty() {
        return Err(Error::NoActivations);
    }
    let prefill = segments
        .iter()
        .take_while(|s| s.phase == Phase::Prefill)
        .count();
    let (pre, dec) = segments.split_at(prefill);
    if dec.iter().any(|s| s.phase == Phase::Prefill) {
        return Err(Error::SegmentOrder);
    }
    // Activations as the reference sees them: the whole prefill, then one per decode step.
    let mut activations: Vec<&[Segment]> = Vec::with_capacity(dec.len().saturating_add(1));
    if !pre.is_empty() {
        activations.push(pre);
    }
    activations.extend(dec.chunks(1));
    let (first, rest) = activations.split_first().ok_or(Error::NoActivations)?;
    let mut chunks: Vec<Vec<&Segment>> = Vec::new();
    let decode: &[&[Segment]] = if params.skip_prefill {
        &activations
    } else {
        chunks.push(first.iter().collect());
        rest
    };
    for group in decode.chunks(batch) {
        chunks.push(group.iter().flat_map(|a| a.iter()).collect());
    }

    let mut n = 0usize;
    chunks
        .iter()
        .enumerate()
        .map(|(c, chunk)| {
            let mut offset = 0usize;
            let mut points: Vec<(usize, Bf16)> = Vec::new();
            for segment in chunk {
                let len = check_segment(segment, topk, n)?;
                n = n.saturating_add(1);
                for cand in &segment.candidates {
                    let index = usize::try_from(cand.index)
                        .ok()
                        .and_then(|i| i.checked_add(offset))
                        .ok_or(Error::BadCandidates { segment: n })?;
                    points.push((index, cand.value));
                }
                offset = offset
                    .checked_add(len)
                    .ok_or(Error::BadCandidates { segment: n })?;
            }
            if offset < topk {
                return Err(Error::ChunkTooSmall { chunk: c });
            }
            // Larger magnitude first, then the lower index (as `build_proofs`).
            points
                .sort_by(|(ia, va), (ib, vb)| vb.magnitude().cmp(&va.magnitude()).then(ia.cmp(ib)));
            points.truncate(topk);
            prove_points(&points)
        })
        .collect()
}

/// The top-k candidates of `activations` (a segment's flattened values) under the same
/// ordering as proof construction: what a plugin sends. For tests, tools and mock engines.
#[must_use]
pub fn top_k_candidates(activations: &[Bf16], topk: usize) -> Vec<Candidate> {
    let mut order: Vec<usize> = (0..activations.len()).collect();
    order.sort_by(|a, b| {
        let ma = activations.get(*a).map_or(0, |v| v.magnitude());
        let mb = activations.get(*b).map_or(0, |v| v.magnitude());
        mb.cmp(&ma).then(a.cmp(b))
    });
    order
        .into_iter()
        .take(topk)
        .filter_map(|i| {
            Some(Candidate {
                index: u32::try_from(i).ok()?,
                value: *activations.get(i)?,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build_proofs;
    use alloc::vec;
    use proptest::prelude::*;

    fn params(batch: u32, topk: u32) -> Params {
        Params {
            decode_batching_size: batch,
            topk,
            skip_prefill: false,
        }
    }

    fn segment(phase: Phase, values: &[Bf16], topk: usize) -> Segment {
        Segment {
            phase,
            len: u32::try_from(values.len()).unwrap(),
            candidates: top_k_candidates(values, topk),
        }
    }

    /// Splits `values` into the segments of a prefill computed in `parts` steps.
    fn prefill(values: &[Bf16], parts: usize, topk: usize) -> Vec<Segment> {
        let step = values.len().div_ceil(parts).max(1);
        values
            .chunks(step)
            .map(|s| segment(Phase::Prefill, s, topk))
            .collect()
    }

    fn values(n: u16) -> Vec<Bf16> {
        (0..n)
            .map(|i| Bf16(0x3f80u16.saturating_add(i.saturating_mul(3))))
            .collect()
    }

    // Scenario "候选下标越界".
    #[test]
    fn an_index_out_of_the_segment_is_an_error() {
        let mut s = segment(Phase::Prefill, &values(8), 4);
        s.candidates[0].index = 8;
        assert_eq!(
            build_proofs_from_candidates(&[s], &params(1, 4)),
            Err(Error::BadCandidates { segment: 0 })
        );
    }

    #[test]
    fn bad_candidate_sets_are_errors() {
        let p = params(1, 4);
        let good = segment(Phase::Prefill, &values(8), 4);
        // A repeated index.
        let mut s = good.clone();
        s.candidates[1].index = s.candidates[0].index;
        assert_eq!(
            build_proofs_from_candidates(&[s], &p),
            Err(Error::BadCandidates { segment: 0 })
        );
        // Too few and too many.
        let mut s = good.clone();
        s.candidates.pop();
        assert_eq!(
            build_proofs_from_candidates(&[s], &p),
            Err(Error::BadCandidates { segment: 0 })
        );
        let mut s = good.clone();
        s.candidates.push(Candidate {
            index: 7,
            value: Bf16(1),
        });
        assert_eq!(
            build_proofs_from_candidates(&[s], &p),
            Err(Error::BadCandidates { segment: 0 })
        );
        // A segment shorter than k sends all its values, and fewer is an error.
        let short = segment(Phase::Decode, &values(2), 4);
        assert_eq!(short.candidates.len(), 2);
        let mut s = short.clone();
        s.candidates.pop();
        assert_eq!(
            build_proofs_from_candidates(&[good.clone(), s], &p),
            Err(Error::BadCandidates { segment: 1 })
        );
        // Infinite values.
        let mut s = good.clone();
        s.candidates[0].value = Bf16(0x7f80);
        assert_eq!(
            build_proofs_from_candidates(&[s], &p),
            Err(Error::NotFinite)
        );
        // A chunk of two decode steps of 2 values each is too small for k = 4... with batch 1.
        assert_eq!(
            build_proofs_from_candidates(&[good.clone(), short], &p),
            Err(Error::ChunkTooSmall { chunk: 1 })
        );
    }

    #[test]
    fn order_and_emptiness() {
        let p = params(1, 2);
        let pre = segment(Phase::Prefill, &values(4), 2);
        let dec = segment(Phase::Decode, &values(4), 2);
        assert_eq!(
            build_proofs_from_candidates(&[], &p),
            Err(Error::NoActivations)
        );
        assert_eq!(
            build_proofs_from_candidates(&[pre.clone(), dec.clone(), pre], &p),
            Err(Error::SegmentOrder)
        );
        assert_eq!(
            build_proofs_from_candidates(&[dec], &params(0, 2)),
            Err(Error::ZeroParameter)
        );
    }

    // A chunk of more than 65,497 values whose top-k indices collide mod 65,497: the modulus
    // drops below the prime, for candidates as for whole activations.
    #[test]
    fn large_prefill_split_in_steps() {
        let mut pre = vec![Bf16(0x3f80); 70_000];
        pre[0] = Bf16(0x4400);
        pre[65_497] = Bf16(0xc480);
        pre[3] = Bf16(0x4500);
        pre[69_999] = Bf16(0x4300);
        let dec = values(16);
        let p = params(2, 4);
        let full = build_proofs(&[&pre, &dec, &dec, &dec], &p).unwrap();
        assert!(full[0].modulus < 65_497);
        let mut segs = prefill(&pre, 3, 4);
        segs.extend((0..3).map(|_| segment(Phase::Decode, &dec, 4)));
        assert_eq!(build_proofs_from_candidates(&segs, &p).unwrap(), full);
    }

    // With skip_prefill the prefill is the first activation of the first chunk.
    #[test]
    fn skip_prefill() {
        let pre = values(12);
        let dec = values(5);
        let p = Params {
            decode_batching_size: 2,
            topk: 3,
            skip_prefill: true,
        };
        let full = build_proofs(&[&pre, &dec, &dec, &dec], &p).unwrap();
        let mut segs = prefill(&pre, 2, 3);
        segs.extend((0..3).map(|_| segment(Phase::Decode, &dec, 3)));
        assert_eq!(build_proofs_from_candidates(&segs, &p).unwrap(), full);
    }

    // A tie between a segment's k-th and (k+1)-th value: the plugin may pick either; the
    // result is still a valid proof with the right number of chunks (design D3).
    #[test]
    fn ties_at_the_boundary_still_prove() {
        let pre = vec![Bf16(0x4000), Bf16(0x3f80), Bf16(0x3f80), Bf16(0x3f80)];
        let p = params(1, 2);
        let mut s = segment(Phase::Prefill, &pre, 2);
        // The engine chose index 3 instead of index 1 for the tied second value.
        s.candidates[1].index = 3;
        let proofs = build_proofs_from_candidates(&[s], &p).unwrap();
        assert_eq!(proofs.len(), 1);
        assert_eq!(proofs[0].coeffs.len(), 2);
        assert_eq!(
            ProofPoly::from_bytes(&proofs[0].to_bytes()),
            Ok(proofs[0].clone())
        );
    }

    /// Distinct magnitudes with random signs: no ties anywhere.
    fn tie_free(n: usize) -> impl Strategy<Value = Vec<Bf16>> {
        (
            proptest::sample::subsequence((1u16..0x7f7f).collect::<Vec<_>>(), n).prop_shuffle(),
            proptest::collection::vec(any::<bool>(), n),
        )
            .prop_map(|(m, neg)| {
                m.into_iter()
                    .zip(neg)
                    .map(|(m, n)| Bf16(if n { m | 0x8000 } else { m }))
                    .collect()
            })
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(64))]

        // Scenario "与全量构造一致": prefill split over steps, decode steps, a last batch
        // that is not full, several k.
        #[test]
        fn candidates_give_the_same_proofs(
            (hidden, prompt, _steps, parts, batch, topk, all) in
                (1usize..24, 1usize..12, 0usize..40, 1usize..4, 1u32..6, 1u32..10)
                    .prop_flat_map(|(h, p, s, parts, b, k)| {
                        (Just(h), Just(p), Just(s), Just(parts), Just(b), Just(k),
                         tie_free(h.saturating_mul(p.saturating_add(s))))
                    }),
        ) {
            let (pre, dec) = all.split_at(hidden.saturating_mul(prompt));
            let decode: Vec<&[Bf16]> = dec.chunks(hidden).collect();
            let mut acts: Vec<&[Bf16]> = vec![pre];
            acts.extend(&decode);
            let p = params(batch, topk);
            let k = usize::try_from(topk).unwrap();
            let mut segs = prefill(pre, parts, k);
            segs.extend(decode.iter().map(|d| segment(Phase::Decode, d, k)));
            prop_assert_eq!(build_proofs_from_candidates(&segs, &p), build_proofs(&acts, &p));
        }
    }
}
