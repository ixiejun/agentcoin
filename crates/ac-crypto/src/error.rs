//! Error type shared by every operation in this crate.

use core::fmt;

/// Errors returned by `ac-crypto`.
///
/// Every failure is reported through this type; no operation in this crate panics.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// The algorithm identifier is not in the published AlgId table.
    UnknownAlgorithm(u8),
    /// The algorithm identifier is reserved but not implemented yet.
    NotImplemented(u8),
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
    /// A mnemonic could not be decoded.
    InvalidMnemonic(MnemonicError),
    /// Decryption of an encrypted secret failed: wrong passphrase or tampered data.
    DecryptionFailed,
    /// An encrypted secret file uses KDF parameters below the accepted minimum.
    WeakKdfParams,
    /// An encrypted secret file is malformed or uses an unsupported version.
    InvalidKeystore,
    /// A hash input exceeds the length the encoding supports.
    InputTooLong,
    /// A sealed-channel handshake or chunk was rejected.
    Sealed(SealedError),
}

/// Why a sealed-channel handshake or chunk was rejected.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SealedError {
    /// The encoding is malformed.
    Malformed,
    /// The handshake names another recipient key.
    WrongRecipient,
    /// The handshake's creation time is outside the validity window.
    Expired,
    /// The sender account has no registered key.
    UnknownSender,
    /// The handshake's key is not the sender account's current key.
    KeyMismatch,
    /// This handshake was already accepted.
    Replayed,
    /// Too many live handshakes to remember; try again later.
    ReplayCacheFull,
    /// A chunk failed authentication (tampered, reordered or foreign).
    Authentication,
    /// A chunk arrived after the last one.
    AfterFinal,
    /// The stream ended before its last chunk.
    Truncated,
    /// A chunk's plaintext exceeds the maximum size.
    ChunkTooLarge,
}

impl From<SealedError> for Error {
    fn from(e: SealedError) -> Self {
        Self::Sealed(e)
    }
}

/// Why a mnemonic was rejected.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MnemonicError {
    /// The phrase does not have the required number of words.
    WordCount,
    /// A word is not in the English word list.
    UnknownWord,
    /// The checksum does not match.
    Checksum,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownAlgorithm(id) => write!(f, "unknown algorithm id {id:#04x}"),
            Self::NotImplemented(id) => {
                write!(f, "algorithm id {id:#04x} is reserved but not implemented")
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
            Self::InvalidMnemonic(MnemonicError::WordCount) => {
                f.write_str("mnemonic must have 24 words")
            }
            Self::InvalidMnemonic(MnemonicError::UnknownWord) => {
                f.write_str("mnemonic contains an unknown word")
            }
            Self::InvalidMnemonic(MnemonicError::Checksum) => {
                f.write_str("mnemonic checksum mismatch")
            }
            Self::DecryptionFailed => f.write_str("decryption failed"),
            Self::WeakKdfParams => f.write_str("KDF parameters below the accepted minimum"),
            Self::InvalidKeystore => f.write_str("malformed or unsupported encrypted secret file"),
            Self::InputTooLong => f.write_str("hash input too long"),
            Self::Sealed(e) => write!(f, "sealed channel: {e}"),
        }
    }
}

impl fmt::Display for SealedError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Malformed => "malformed input",
            Self::WrongRecipient => "handshake is for another recipient",
            Self::Expired => "handshake outside the validity window (check the clock)",
            Self::UnknownSender => "sender account has no registered key",
            Self::KeyMismatch => "handshake key is not the sender's current key",
            Self::Replayed => "handshake already used",
            Self::ReplayCacheFull => "too many recent handshakes, try again later",
            Self::Authentication => "chunk failed authentication",
            Self::AfterFinal => "data after the last chunk",
            Self::Truncated => "stream truncated before its last chunk",
            Self::ChunkTooLarge => "chunk too large",
        })
    }
}

#[cfg(feature = "std")]
impl std::error::Error for Error {}
