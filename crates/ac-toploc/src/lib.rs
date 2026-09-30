#![doc = include_str!("../README.md")]
#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

mod candidates;
mod field;
mod proof;

pub use candidates::{Candidate, Phase, Segment, build_proofs_from_candidates, top_k_candidates};
pub use proof::{
    Comparison, ProofPoly, TOPLOC_COMMIT_CONTEXT, build_proofs, commitment, compare,
    injective_modulus,
};

/// The prime modulus of the polynomial field.
pub const FIELD_MODULUS: u32 = field::P;

/// A bfloat16 value as its 16-bit pattern (sign, 8 exponent bits, 7 mantissa bits).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Bf16(pub u16);

impl Bf16 {
    const EXP_MASK: u16 = 0x7F80;
    const MANT_MASK: u16 = 0x007F;

    /// The exponent bits.
    #[must_use]
    pub const fn exponent(self) -> u16 {
        (self.0 & Self::EXP_MASK) >> 7
    }

    /// The mantissa bits.
    #[must_use]
    pub const fn mantissa(self) -> u16 {
        self.0 & Self::MANT_MASK
    }

    /// `true` unless the value is infinite or NaN (all exponent bits set).
    #[must_use]
    pub const fn is_finite(self) -> bool {
        self.0 & Self::EXP_MASK != Self::EXP_MASK
    }

    /// Bit pattern of the absolute value; for finite values its order is the order of
    /// magnitudes.
    #[must_use]
    pub const fn magnitude(self) -> u16 {
        self.0 & 0x7FFF
    }
}

/// How activations are chunked and how many values each proof keeps.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Params {
    /// Generated tokens per decode chunk (at least 1).
    pub decode_batching_size: u32,
    /// Values kept per chunk (at least 1).
    pub topk: u32,
    /// Whether the first activation is not a separate prefill chunk.
    pub skip_prefill: bool,
}

/// Why proofs cannot be built, decoded or compared.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Error {
    /// A batching size or top-k of zero.
    ZeroParameter,
    /// No activations.
    NoActivations,
    /// A chunk has fewer values than top-k.
    ChunkTooSmall {
        /// Index of the chunk.
        chunk: usize,
    },
    /// An infinite or NaN activation.
    NotFinite,
    /// No modulus up to 65,497 keeps the chosen indices distinct.
    NoInjectiveModulus,
    /// A proof encoding shorter than two bytes or of odd length.
    BadEncoding,
    /// A proof with modulus zero or no coefficients (the reference's "null proof").
    NullProof,
    /// The number of proofs is not the number of chunks.
    ChunkCountMismatch {
        /// Proofs given.
        proofs: usize,
        /// Chunks of the activations.
        chunks: usize,
    },
    /// A segment's candidates are not its top `min(k, len)`: wrong count, an index out of the
    /// segment or repeated.
    BadCandidates {
        /// Index of the segment.
        segment: usize,
    },
    /// A prefill segment follows a decode segment.
    SegmentOrder,
}

impl core::fmt::Display for Error {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::ZeroParameter => f.write_str("the batching size and top-k must be positive"),
            Self::NoActivations => f.write_str("there are no activations"),
            Self::ChunkTooSmall { chunk } => {
                write!(f, "chunk {chunk} has fewer values than top-k")
            }
            Self::NotFinite => f.write_str("an activation is infinite or NaN"),
            Self::NoInjectiveModulus => {
                f.write_str("no modulus up to 65,497 keeps the indices distinct")
            }
            Self::BadEncoding => f.write_str("a proof encoding is too short or of odd length"),
            Self::NullProof => f.write_str("a proof has modulus zero or no coefficients"),
            Self::ChunkCountMismatch { proofs, chunks } => {
                write!(f, "{proofs} proofs for {chunks} chunks")
            }
            Self::BadCandidates { segment } => {
                write!(f, "segment {segment} does not carry valid top-k candidates")
            }
            Self::SegmentOrder => f.write_str("a prefill segment follows a decode segment"),
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for Error {}
