//! Error type shared by every operation in this crate.

use core::fmt;

/// Errors returned by `ac-crypto`.
///
/// Every failure is reported through this type; no operation in this crate panics.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// The algorithm identifier is not in the published AlgId table.
    UnknownAlgorithm(u16),
    /// The algorithm identifier is reserved but not implemented yet.
    NotImplemented(u16),
    /// A byte string has the wrong length for its algorithm.
    InvalidLength {
        /// Length required by the algorithm.
        expected: usize,
        /// Length that was supplied.
        actual: usize,
    },
    /// A public key and a signature (or ciphertext) use different algorithms.
    AlgorithmMismatch,
    /// A signing context string is longer than 255 bytes.
    ContextTooLong,
    /// A domain-separation context string does not follow
    /// `agentcoin <YYYY-MM> <purpose> v<version>`.
    InvalidContext,
    /// A signature did not verify.
    InvalidSignature,
    /// Key bytes were rejected by the algorithm.
    InvalidKey,
    /// The random number generator failed.
    Randomness,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownAlgorithm(id) => write!(f, "unknown algorithm id {id:#06x}"),
            Self::NotImplemented(id) => {
                write!(f, "algorithm id {id:#06x} is reserved but not implemented")
            }
            Self::InvalidLength { expected, actual } => {
                write!(f, "invalid length: expected {expected} bytes, got {actual}")
            }
            Self::AlgorithmMismatch => f.write_str("algorithm mismatch"),
            Self::ContextTooLong => f.write_str("signing context longer than 255 bytes"),
            Self::InvalidContext => f.write_str("invalid domain-separation context"),
            Self::InvalidSignature => f.write_str("invalid signature"),
            Self::InvalidKey => f.write_str("invalid key"),
            Self::Randomness => f.write_str("random number generator failure"),
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for Error {}
