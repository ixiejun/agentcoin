//! Vectors from the reference implementation (spec `market/toploc`, "参考实现兼容性与测试向量").
//!
//! `tests/vectors/toploc.json` is written by `scripts/gen-toploc-vectors.sh`, which runs the
//! reference at a pinned commit. Activations are regenerated here from the recorded seeds with
//! the same splitmix64 generator; proofs must match byte for byte, comparisons must give the
//! reference's numbers.
//!
//! The reference has two checks. Its batched (C++) check reduces indices by the proof's modulus,
//! as proof construction does, and takes the sorted errors' element n/2 as the median; this
//! crate's [`compare`] agrees with it on every vector. Its list (Python) check evaluates at
//! unreduced indices and takes `statistics.median`; it agrees with its own proofs only when the
//! modulus is the prime 65,497, so those results are compared only for such proofs.
// Integration-test crate: AGENT.md §5.3 permits unwrap/expect/panics in test code.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use ac_toploc::{Bf16, Comparison, FIELD_MODULUS, Params, ProofPoly, build_proofs, compare};
use serde::Deserialize;

#[derive(Deserialize)]
struct Doc {
    source: Source,
    cases: Vec<Case>,
}

#[derive(Deserialize)]
struct Source {
    url: String,
    commit: String,
    archive_sha256: String,
    torch: String,
}

#[derive(Deserialize)]
struct Case {
    name: String,
    seed: u64,
    lengths: Vec<usize>,
    overrides: Vec<(usize, usize, u16)>,
    decode_batching_size: u32,
    topk: u32,
    skip_prefill: bool,
    proofs: Vec<String>,
    same: Vec<Reference>,
    perturbed: Vec<Reference>,
    same_batched: Option<Vec<Reference>>,
    perturbed_batched: Option<Vec<Reference>>,
}

#[derive(Deserialize)]
struct Reference {
    exp_mismatches: u32,
    mant_err_mean: f64,
    mant_err_median: f64,
}

struct SplitMix64(u64);

impl SplitMix64 {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
}

fn activations(case: &Case) -> Vec<Vec<u16>> {
    let mut rng = SplitMix64(case.seed);
    let mut acts: Vec<Vec<u16>> = case
        .lengths
        .iter()
        .map(|n| {
            (0..*n)
                .map(|_| {
                    let r = rng.next();
                    let sign = (r & 1) as u16;
                    let exp = 0x70 + ((r >> 1) % 22) as u16;
                    let mant = ((r >> 8) & 0x7f) as u16;
                    (sign << 15) | (exp << 7) | mant
                })
                .collect()
        })
        .collect();
    for (a, i, bits) in &case.overrides {
        acts[*a][*i] = *bits;
    }
    acts
}

fn perturbed(case: &Case, acts: &[Vec<u16>]) -> Vec<Vec<u16>> {
    let mut rng = SplitMix64(case.seed ^ 0xA5A5_A5A5);
    acts.iter()
        .map(|a| {
            a.iter()
                .map(|b| {
                    let r = rng.next();
                    let kind = r % 16;
                    let mut exp = (b >> 7) & 0xff;
                    let mut mant = i32::from(b & 0x7f);
                    if kind == 0 {
                        exp += 1;
                    } else if kind <= 8 {
                        let delta = 1 + ((r >> 8) % 3) as i32;
                        mant = if (r >> 16) & 1 == 1 {
                            mant + delta
                        } else {
                            mant - delta
                        };
                        mant = mant.clamp(0, 0x7f);
                    }
                    (b & 0x8000) | (exp << 7) | mant as u16
                })
                .collect()
        })
        .collect()
}

fn run(acts: &[Vec<u16>], proofs: &[ProofPoly], params: &Params) -> Vec<Comparison> {
    let bf: Vec<Vec<Bf16>> = acts
        .iter()
        .map(|a| a.iter().copied().map(Bf16).collect())
        .collect();
    let refs: Vec<&[Bf16]> = bf.iter().map(Vec::as_slice).collect();
    compare(&refs, proofs, params).unwrap()
}

const NO_MATCH: f64 = 18_446_744_073_709_551_616.0; // 2^64, the reference's "no statistics"

fn mean(c: &Comparison) -> f64 {
    if c.mant_count == 0 {
        NO_MATCH
    } else {
        f64::from(c.mant_err_sum) / f64::from(c.mant_count)
    }
}

fn check_python(name: &str, ours: &[Comparison], theirs: &[Reference]) {
    assert_eq!(ours.len(), theirs.len(), "{name}");
    for (i, (c, r)) in ours.iter().zip(theirs).enumerate() {
        assert_eq!(c.exp_mismatches, r.exp_mismatches, "{name} chunk {i}");
        assert_eq!(mean(c), r.mant_err_mean, "{name} chunk {i} mean");
        let median = c.median_twice.map_or(NO_MATCH, |m| f64::from(m) / 2.0);
        assert_eq!(median, r.mant_err_median, "{name} chunk {i} median");
    }
}

fn check_batched(name: &str, ours: &[Comparison], theirs: &[Reference]) {
    assert_eq!(ours.len(), theirs.len(), "{name}");
    for (i, (c, r)) in ours.iter().zip(theirs).enumerate() {
        assert_eq!(c.exp_mismatches, r.exp_mismatches, "{name} chunk {i}");
        assert_eq!(mean(c), r.mant_err_mean, "{name} chunk {i} mean");
        let median = c.median_upper.map_or(NO_MATCH, f64::from);
        assert_eq!(median, r.mant_err_median, "{name} chunk {i} median");
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn doc() -> Doc {
    serde_json::from_str(include_str!("vectors/toploc.json")).unwrap()
}

// The vector file records where it came from (spec: URL, commit, archive SHA-256, versions).
#[test]
fn vectors_record_their_source() {
    let d = doc();
    assert_eq!(d.source.url, "https://github.com/PrimeIntellect-ai/toploc");
    assert_eq!(d.source.commit, "7ab7bcd6a4459ba4400fd41e4636bbeed7438997");
    assert_eq!(d.source.archive_sha256.len(), 64);
    assert!(!d.source.torch.is_empty());
    let names: Vec<&str> = d.cases.iter().map(|c| c.name.as_str()).collect();
    for required in [
        "prefill-and-decode-batches",
        "skip-prefill",
        "short-last-batch",
        "modulus-below-prime",
    ] {
        assert!(names.contains(&required), "{required} missing");
    }
}

// Scenarios "与参考实现一致", "与参考实现的比对结果一致" and "全部向量通过".
#[test]
fn every_vector_matches_the_reference() {
    let mut python_checked = 0;
    let mut batched_checked = 0;
    for case in doc().cases {
        let params = Params {
            decode_batching_size: case.decode_batching_size,
            topk: case.topk,
            skip_prefill: case.skip_prefill,
        };
        let acts = activations(&case);
        let bf: Vec<Vec<Bf16>> = acts
            .iter()
            .map(|a| a.iter().copied().map(Bf16).collect())
            .collect();
        let refs: Vec<&[Bf16]> = bf.iter().map(Vec::as_slice).collect();
        let proofs = build_proofs(&refs, &params).unwrap();
        let ours: Vec<String> = proofs.iter().map(|p| hex(&p.to_bytes())).collect();
        assert_eq!(ours, case.proofs, "{}: proofs", case.name);
        // The reference's encoding decodes to the same proofs.
        for (p, h) in proofs.iter().zip(&case.proofs) {
            let bytes: Vec<u8> = (0..h.len())
                .step_by(2)
                .map(|i| u8::from_str_radix(&h[i..i + 2], 16).unwrap())
                .collect();
            assert_eq!(&ProofPoly::from_bytes(&bytes).unwrap(), p);
        }

        let same = run(&acts, &proofs, &params);
        let pert = run(&perturbed(&case, &acts), &proofs, &params);
        if proofs.iter().all(|p| u32::from(p.modulus) == FIELD_MODULUS) {
            check_python(&format!("{} same", case.name), &same, &case.same);
            check_python(&format!("{} perturbed", case.name), &pert, &case.perturbed);
            python_checked += 1;
        }
        if let Some(r) = &case.same_batched {
            check_batched(&format!("{} same batched", case.name), &same, r);
            batched_checked += 1;
        }
        if let Some(r) = &case.perturbed_batched {
            check_batched(&format!("{} perturbed batched", case.name), &pert, r);
        }
        // The same activations always match exactly.
        assert!(
            same.iter()
                .all(|c| c.exp_mismatches == 0 && c.mant_err_sum == 0),
            "{}",
            case.name
        );
    }
    assert!(
        python_checked >= 4,
        "{python_checked} cases on the list check"
    );
    assert!(
        batched_checked >= 3,
        "{batched_checked} cases on the batched check"
    );
}
