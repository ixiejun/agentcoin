//! Embedding fingerprints (spec `market/public-worker` "嵌入单元的执行"; design D10).
//!
//! A text's embedding is L2-normalized; bit `j` (0–31) of its fingerprint says whether the
//! vector's dot product with direction `j` of the job is positive. Direction `j` has a sign per
//! dimension: dimension `d` takes bit `d mod 256` (least significant bit of each byte first) of
//! [`direction_block`]`(job, j, d / 256)`, set meaning +1. Anyone can rebuild the directions from
//! the job number, and nearby vectors (cosine ≥ 0.999) differ in few bits.

use ac_primitives::market::public::{FINGERPRINT_BITS, JobId, direction_block};

/// The signs of direction `j` of `job` for `dims` dimensions.
#[must_use]
pub fn direction(job: JobId, j: u32, dims: usize) -> Vec<bool> {
    let mut out = Vec::with_capacity(dims);
    let mut block = 0u32;
    while out.len() < dims {
        let bytes = direction_block(job, j, block);
        for byte in bytes {
            for bit in 0..8 {
                if out.len() < dims {
                    out.push(byte >> bit & 1 == 1);
                }
            }
        }
        block = block.saturating_add(1);
    }
    out
}

/// The directions of `job` for vectors of `dims` dimensions.
#[must_use]
pub fn directions(job: JobId, dims: usize) -> Vec<Vec<bool>> {
    (0..FINGERPRINT_BITS)
        .map(|j| direction(job, j, dims))
        .collect()
}

/// The fingerprint of an embedding.
#[must_use]
pub fn fingerprint(directions: &[Vec<bool>], embedding: &[f32]) -> u32 {
    let norm = embedding
        .iter()
        .map(|x| f64::from(*x) * f64::from(*x))
        .sum::<f64>()
        .sqrt();
    let mut print = 0u32;
    for (j, dir) in directions.iter().enumerate() {
        let dot: f64 = embedding
            .iter()
            .zip(dir)
            .map(|(x, plus)| {
                let v = if norm > 0.0 {
                    f64::from(*x) / norm
                } else {
                    0.0
                };
                if *plus { v } else { -v }
            })
            .sum();
        if dot > 0.0 {
            print |= 1 << j;
        }
    }
    print
}

/// The summary of an embedding unit: each text's fingerprint, 4 bytes little endian.
#[must_use]
pub fn summary(directions: &[Vec<bool>], embeddings: &[Vec<f32>]) -> Vec<u8> {
    embeddings
        .iter()
        .flat_map(|e| fingerprint(directions, e).to_le_bytes())
        .collect()
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::indexing_slicing,
        clippy::cast_precision_loss
    )] // Test code.

    use super::*;

    #[test]
    fn directions_are_deterministic_and_differ() {
        let a = direction(3, 0, 600);
        assert_eq!(a.len(), 600);
        assert_eq!(a, direction(3, 0, 600));
        assert_ne!(a, direction(3, 1, 600));
        assert_ne!(a, direction(4, 0, 600));
        let plus = a.iter().filter(|x| **x).count();
        assert!((200..400).contains(&plus), "{plus}");
    }

    // Spec "指纹可复现": a vector and a slightly perturbed copy (cosine ≥ 0.999) differ in at
    // most a few bits; an unrelated vector differs in many.
    #[test]
    fn close_vectors_have_close_fingerprints() {
        let dirs = directions(7, 384);
        let mut far_bits = 0;
        for seed in 0..50u32 {
            let v: Vec<f32> = (0..384)
                .map(|i| ((i as f32 + 1.0) * (seed as f32 + 0.7)).sin())
                .collect();
            let w: Vec<f32> = v
                .iter()
                .enumerate()
                .map(|(i, x)| x + 0.01 * ((i as f32) * 1.3 + seed as f32).cos())
                .collect();
            let u: Vec<f32> = (0..384)
                .map(|i| ((i as f32 + 2.0) * (seed as f32 + 3.1)).cos())
                .collect();
            let near = (fingerprint(&dirs, &v) ^ fingerprint(&dirs, &w)).count_ones();
            assert!(near <= 3, "seed {seed}: {near}");
            far_bits += (fingerprint(&dirs, &v) ^ fingerprint(&dirs, &u)).count_ones();
        }
        assert!(far_bits > 50 * 10, "{far_bits}");
    }
}
