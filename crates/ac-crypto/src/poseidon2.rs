//! Poseidon2 hash over the Goldilocks field (m4-evm, spec crypto/hashing, design D7 appendix 2).
//!
//! The permutation is Plonky3's default Goldilocks width-12 instance (`p3-goldilocks`,
//! `default_goldilocks_poseidon2_12`): S-box x^7, 8 external and 22 internal rounds, with
//! Plonky3's fixed constants and internal diagonal. It is not the instance of the Poseidon2
//! authors' reference implementation, so it is checked against Plonky3's published
//! known-answer vector rather than the authors' vectors (a user decision, recorded in
//! `docs/decisions.md`).
//!
//! The hash is a sponge with rate 8 and capacity 4 over an all-zero initial state. The input is
//! encoded injectively as field elements: its byte length first, then the bytes followed by
//! `0x01` and zero bytes up to a multiple of 7, as little-endian 7-byte elements, then zero
//! elements up to a multiple of 8. Each group of 8 elements overwrites the rate part of the state
//! before a permutation. The output is the first 4 state elements after the last permutation,
//! as little-endian 8-byte words: 32 bytes (256 bits, 128-bit security).

use alloc::vec::Vec;

use p3_field::PrimeField64;
use p3_goldilocks::{Goldilocks, default_goldilocks_poseidon2_12};
use p3_symmetric::{CryptographicHasher, PaddingFreeSponge, Permutation};

use crate::Error;

/// State width of the permutation.
pub const WIDTH: usize = 12;
/// Elements absorbed per permutation.
pub const RATE: usize = 8;
/// Output elements (4 × 64 bits).
pub const OUTPUT_ELEMENTS: usize = 4;
/// Output length in bytes.
pub const OUTPUT_LEN: usize = 32;
/// Input bytes packed into one field element (below 2^56, so always below the modulus).
pub const BYTES_PER_ELEMENT: usize = 7;
/// Longest input in bytes: the length is itself one element.
pub const MAX_INPUT_LEN: u64 = 0xffff_ffff;

/// Applies the permutation to canonical field elements (each below the Goldilocks modulus
/// 2^64 − 2^32 + 1; larger values are reduced).
pub fn permute(state: &mut [u64; WIDTH]) {
    let mut elements = state.map(Goldilocks::new);
    default_goldilocks_poseidon2_12().permute_mut(&mut elements);
    for (out, element) in state.iter_mut().zip(elements) {
        *out = element.as_canonical_u64();
    }
}

/// The injective encoding of `input` as field elements (a multiple of [`RATE`] of them).
///
/// # Errors
///
/// [`Error::InputTooLong`] for inputs longer than [`MAX_INPUT_LEN`] bytes.
pub fn encode(input: &[u8]) -> Result<Vec<u64>, Error> {
    let length = u64::try_from(input.len())
        .ok()
        .filter(|length| *length <= MAX_INPUT_LEN)
        .ok_or(Error::InputTooLong)?;
    let mut padded = Vec::with_capacity(input.len().saturating_add(BYTES_PER_ELEMENT));
    padded.extend_from_slice(input);
    padded.push(0x01);
    while padded.len() % BYTES_PER_ELEMENT != 0 {
        padded.push(0);
    }
    let mut elements = Vec::with_capacity(padded.len() / BYTES_PER_ELEMENT + RATE + 1);
    elements.push(length);
    for chunk in padded.chunks(BYTES_PER_ELEMENT) {
        let mut word = [0u8; 8];
        for (dst, src) in word.iter_mut().zip(chunk) {
            *dst = *src;
        }
        elements.push(u64::from_le_bytes(word));
    }
    while elements.len() % RATE != 0 {
        elements.push(0);
    }
    Ok(elements)
}

/// Poseidon2 hash of `input` (32 bytes).
///
/// # Errors
///
/// [`Error::InputTooLong`] for inputs longer than [`MAX_INPUT_LEN`] bytes.
pub fn hash(input: &[u8]) -> Result<[u8; OUTPUT_LEN], Error> {
    // Plonky3's sponge: overwrite the rate with each block of 8 elements, then permute; the
    // encoding always yields whole blocks.
    let sponge = PaddingFreeSponge::<_, WIDTH, RATE, OUTPUT_ELEMENTS>::new(
        default_goldilocks_poseidon2_12(),
    );
    let digest: [Goldilocks; OUTPUT_ELEMENTS] =
        sponge.hash_iter(encode(input)?.into_iter().map(Goldilocks::new));
    let mut out = [0u8; OUTPUT_LEN];
    for (bytes, element) in out.chunks_mut(8).zip(digest) {
        bytes.copy_from_slice(&element.as_canonical_u64().to_le_bytes());
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::indexing_slicing)]

    use super::*;

    #[test]
    fn encoding_shape() {
        // Empty input: length 0, then 0x01 padded to one element, then zeros to 8 elements.
        assert_eq!(encode(&[]).unwrap(), [0, 1, 0, 0, 0, 0, 0, 0]);
        // Seven bytes fill one element; the padding byte starts the next.
        let e = encode(&[0xff; 7]).unwrap();
        assert_eq!(e[..3], [7, 0x00ff_ffff_ffff_ffff, 1]);
        assert_eq!(e.len(), 8);
        // 49 bytes: length + 7 full elements + padding element = 9 -> 16.
        assert_eq!(encode(&[0; 49]).unwrap().len(), 16);
        assert!(
            encode(&[0; 49])
                .unwrap()
                .iter()
                .all(|e| *e < 0xffff_ffff_0000_0001)
        );
    }

    #[test]
    fn trailing_zero_changes_the_hash() {
        assert_ne!(hash(b"abc").unwrap(), hash(b"abc\0").unwrap());
        assert_ne!(hash(&[]).unwrap(), hash(&[0]).unwrap());
        assert_ne!(hash(&[0; 6]).unwrap(), hash(&[0; 7]).unwrap());
    }
}
