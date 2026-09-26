//! Algorithm identifiers (AlgId).
//!
//! Each identifier is a stable `u16`. Once published, a number is never reused and never
//! changes meaning; new algorithms get new numbers.

use crate::error::Error;

/// Identifier range reserved for future proof systems (`0x2000..=0x2FFF`). Nothing is
/// allocated in it yet; identifiers in this range decode as unknown.
pub const PROOF_SYSTEM_RESERVED: core::ops::RangeInclusive<u16> = 0x2000..=0x2FFF;

/// Signature algorithms.
#[non_exhaustive]
#[repr(u16)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum SigAlg {
    /// ML-DSA-44 (FIPS 204). Default for user accounts.
    MlDsa44 = 0x0101,
    /// ML-DSA-65 (FIPS 204). Validators and high-value accounts.
    MlDsa65 = 0x0102,
    /// ML-DSA-87 (FIPS 204).
    MlDsa87 = 0x0103,
    /// SLH-DSA-SHA2-128s (FIPS 205). Reserved: hash-based fallback.
    SlhDsaSha2_128s = 0x0201,
    /// FN-DSA-512 (Falcon). Reserved: smaller signatures.
    FnDsa512 = 0x0301,
    /// XMSS (lean variant). Reserved: consensus signatures with STARK aggregation.
    XmssLean = 0x0401,
}

impl SigAlg {
    /// Every signature algorithm in the published table.
    pub const ALL: [Self; 6] = [
        Self::MlDsa44,
        Self::MlDsa65,
        Self::MlDsa87,
        Self::SlhDsaSha2_128s,
        Self::FnDsa512,
        Self::XmssLean,
    ];

    /// The stable numeric identifier.
    #[must_use]
    pub const fn id(self) -> u16 {
        self as u16
    }

    /// Looks up an identifier.
    ///
    /// # Errors
    ///
    /// Returns [`Error::UnknownAlgorithm`] if `id` is not in the signature table.
    pub const fn from_id(id: u16) -> Result<Self, Error> {
        match id {
            0x0101 => Ok(Self::MlDsa44),
            0x0102 => Ok(Self::MlDsa65),
            0x0103 => Ok(Self::MlDsa87),
            0x0201 => Ok(Self::SlhDsaSha2_128s),
            0x0301 => Ok(Self::FnDsa512),
            0x0401 => Ok(Self::XmssLean),
            other => Err(Error::UnknownAlgorithm(other)),
        }
    }

    /// Whether this crate implements the algorithm (reserved ones are not).
    #[must_use]
    pub const fn is_implemented(self) -> bool {
        matches!(self, Self::MlDsa44 | Self::MlDsa65 | Self::MlDsa87)
    }

    /// Raw public-key length in bytes.
    ///
    /// # Errors
    ///
    /// Returns [`Error::NotImplemented`] for reserved algorithms.
    pub const fn public_key_len(self) -> Result<usize, Error> {
        match self {
            Self::MlDsa44 => Ok(1312),
            Self::MlDsa65 => Ok(1952),
            Self::MlDsa87 => Ok(2592),
            other => Err(Error::NotImplemented(other.id())),
        }
    }

    /// Raw signature length in bytes.
    ///
    /// # Errors
    ///
    /// Returns [`Error::NotImplemented`] for reserved algorithms.
    pub const fn signature_len(self) -> Result<usize, Error> {
        match self {
            Self::MlDsa44 => Ok(2420),
            Self::MlDsa65 => Ok(3309),
            Self::MlDsa87 => Ok(4627),
            other => Err(Error::NotImplemented(other.id())),
        }
    }
}

/// Key-encapsulation algorithms.
#[non_exhaustive]
#[repr(u16)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum KemAlg {
    /// X-Wing (ML-KEM-768 + X25519), bound to draft-connolly-cfrg-xwing-kem-06.
    XWing = 0x1101,
    /// ML-KEM-1024 (FIPS 203). Reserved.
    MlKem1024 = 0x1102,
}

impl KemAlg {
    /// Every KEM algorithm in the published table.
    pub const ALL: [Self; 2] = [Self::XWing, Self::MlKem1024];

    /// The stable numeric identifier.
    #[must_use]
    pub const fn id(self) -> u16 {
        self as u16
    }

    /// Looks up an identifier.
    ///
    /// # Errors
    ///
    /// Returns [`Error::UnknownAlgorithm`] if `id` is not in the KEM table.
    pub const fn from_id(id: u16) -> Result<Self, Error> {
        match id {
            0x1101 => Ok(Self::XWing),
            0x1102 => Ok(Self::MlKem1024),
            other => Err(Error::UnknownAlgorithm(other)),
        }
    }

    /// Whether this crate implements the algorithm.
    #[must_use]
    pub const fn is_implemented(self) -> bool {
        matches!(self, Self::XWing)
    }

    /// Raw encapsulation (public) key length in bytes.
    ///
    /// # Errors
    ///
    /// Returns [`Error::NotImplemented`] for reserved algorithms.
    pub const fn public_key_len(self) -> Result<usize, Error> {
        match self {
            Self::XWing => Ok(1216),
            Self::MlKem1024 => Err(Error::NotImplemented(self.id())),
        }
    }

    /// Raw ciphertext length in bytes.
    ///
    /// # Errors
    ///
    /// Returns [`Error::NotImplemented`] for reserved algorithms.
    pub const fn ciphertext_len(self) -> Result<usize, Error> {
        match self {
            Self::XWing => Ok(1120),
            Self::MlKem1024 => Err(Error::NotImplemented(self.id())),
        }
    }
}
