//! Algorithm identifiers (AlgId).
//!
//! Each category (signatures, KEMs) has its own 1-byte identifier space. An identifier is
//! also the SCALE enum index of the tagged types, so the canonical encoding, the on-chain
//! encoding and the chain metadata all describe the same bytes (decision D34).
//!
//! Once published, a number is never reused and never changes meaning; new algorithms get
//! new numbers. `0x00` is never allocated and `0xFF` is reserved in every category as an
//! extension marker.

use crate::error::Error;

/// Identifier reserved in every category for a future extension mechanism. Never allocated.
pub const EXTENSION_MARKER: u8 = 0xFF;

/// Signature algorithms.
#[non_exhaustive]
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum SigAlg {
    /// ML-DSA-44 (FIPS 204). Default for user accounts.
    MlDsa44 = 0x01,
    /// ML-DSA-65 (FIPS 204). Validators and high-value accounts.
    MlDsa65 = 0x02,
    /// ML-DSA-87 (FIPS 204).
    MlDsa87 = 0x03,
    /// SLH-DSA-SHA2-128s (FIPS 205). Reserved: hash-based fallback.
    SlhDsaSha2_128s = 0x10,
    /// FN-DSA-512 (Falcon). Reserved: smaller signatures.
    FnDsa512 = 0x20,
    /// XMSS (lean variant). Reserved: consensus signatures with STARK aggregation.
    XmssLean = 0x30,
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
    pub const fn id(self) -> u8 {
        self as u8
    }

    /// Looks up an identifier.
    ///
    /// # Errors
    ///
    /// Returns [`Error::UnknownAlgorithm`] if `id` is not in the signature table.
    pub const fn from_id(id: u8) -> Result<Self, Error> {
        match id {
            0x01 => Ok(Self::MlDsa44),
            0x02 => Ok(Self::MlDsa65),
            0x03 => Ok(Self::MlDsa87),
            0x10 => Ok(Self::SlhDsaSha2_128s),
            0x20 => Ok(Self::FnDsa512),
            0x30 => Ok(Self::XmssLean),
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
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum KemAlg {
    /// X-Wing (ML-KEM-768 + X25519), bound to draft-connolly-cfrg-xwing-kem-06.
    XWing = 0x01,
    /// ML-KEM-1024 (FIPS 203). Reserved.
    MlKem1024 = 0x02,
}

impl KemAlg {
    /// Every KEM algorithm in the published table.
    pub const ALL: [Self; 2] = [Self::XWing, Self::MlKem1024];

    /// The stable numeric identifier.
    #[must_use]
    pub const fn id(self) -> u8 {
        self as u8
    }

    /// Looks up an identifier.
    ///
    /// # Errors
    ///
    /// Returns [`Error::UnknownAlgorithm`] if `id` is not in the KEM table.
    pub const fn from_id(id: u8) -> Result<Self, Error> {
        match id {
            0x01 => Ok(Self::XWing),
            0x02 => Ok(Self::MlKem1024),
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
