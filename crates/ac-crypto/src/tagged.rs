//! Algorithm-tagged byte objects and their canonical wire format.
//!
//! Canonical encoding: a 2-byte little-endian AlgId followed by exactly the fixed number of
//! raw bytes the algorithm prescribes, and nothing else.

use alloc::vec::Vec;

use crate::alg::{KemAlg, SigAlg};
use crate::error::Error;

/// Length of the AlgId prefix in the canonical encoding.
pub const ALG_ID_LEN: usize = 2;

/// Splits a canonical encoding into its AlgId and raw payload.
fn split_prefix(bytes: &[u8]) -> Result<(u16, &[u8]), Error> {
    match bytes {
        [lo, hi, rest @ ..] => Ok((u16::from_le_bytes([*lo, *hi]), rest)),
        _ => Err(Error::InvalidLength {
            expected: ALG_ID_LEN,
            actual: bytes.len(),
        }),
    }
}

fn check_len(expected: usize, actual: usize) -> Result<(), Error> {
    if expected == actual {
        Ok(())
    } else {
        Err(Error::InvalidLength { expected, actual })
    }
}

macro_rules! tagged_type {
    ($(#[$doc:meta])* $name:ident, $alg:ty, $len_fn:ident) => {
        $(#[$doc])*
        #[derive(Clone, PartialEq, Eq, Hash)]
        pub struct $name {
            alg: $alg,
            bytes: Vec<u8>,
        }

        impl $name {
            /// Builds the object from an algorithm and its raw bytes.
            ///
            /// # Errors
            ///
            /// [`Error::NotImplemented`] for reserved algorithms and
            /// [`Error::InvalidLength`] if `raw` has the wrong length.
            pub fn new(alg: $alg, raw: &[u8]) -> Result<Self, Error> {
                check_len(alg.$len_fn()?, raw.len())?;
                Ok(Self { alg, bytes: raw.to_vec() })
            }

            /// The algorithm of this object.
            #[must_use]
            pub const fn alg(&self) -> $alg {
                self.alg
            }

            /// The raw bytes, without the AlgId prefix.
            #[must_use]
            pub fn as_bytes(&self) -> &[u8] {
                &self.bytes
            }

            /// Length of the canonical encoding.
            #[must_use]
            pub fn encoded_len(&self) -> usize {
                ALG_ID_LEN.saturating_add(self.bytes.len())
            }

            /// Canonical encoding: little-endian AlgId followed by the raw bytes.
            #[must_use]
            pub fn to_canonical(&self) -> Vec<u8> {
                let mut out = Vec::with_capacity(self.encoded_len());
                out.extend_from_slice(&self.alg.id().to_le_bytes());
                out.extend_from_slice(&self.bytes);
                out
            }

            /// Decodes a canonical encoding.
            ///
            /// # Errors
            ///
            /// [`Error::UnknownAlgorithm`], [`Error::NotImplemented`] or
            /// [`Error::InvalidLength`] (including trailing bytes).
            pub fn from_canonical(bytes: &[u8]) -> Result<Self, Error> {
                let (id, raw) = split_prefix(bytes)?;
                Self::new(<$alg>::from_id(id)?, raw)
            }
        }

        impl core::fmt::Debug for $name {
            fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
                f.debug_struct(stringify!($name))
                    .field("alg", &self.alg)
                    .field("len", &self.bytes.len())
                    .finish()
            }
        }

        #[cfg(feature = "scale")]
        impl parity_scale_codec::Encode for $name {
            fn size_hint(&self) -> usize {
                self.encoded_len()
            }

            fn encode_to<T: parity_scale_codec::Output + ?Sized>(&self, dest: &mut T) {
                dest.write(&self.alg.id().to_le_bytes());
                dest.write(&self.bytes);
            }
        }

        #[cfg(feature = "scale")]
        impl parity_scale_codec::Decode for $name {
            fn decode<I: parity_scale_codec::Input>(
                input: &mut I,
            ) -> Result<Self, parity_scale_codec::Error> {
                let mut id = [0u8; ALG_ID_LEN];
                input.read(&mut id)?;
                let alg = <$alg>::from_id(u16::from_le_bytes(id))
                    .map_err(|_| parity_scale_codec::Error::from("unknown algorithm id"))?;
                let len = alg
                    .$len_fn()
                    .map_err(|_| parity_scale_codec::Error::from("algorithm not implemented"))?;
                let mut bytes = alloc::vec![0u8; len];
                input.read(&mut bytes)?;
                Ok(Self { alg, bytes })
            }
        }
    };
}

tagged_type!(
    /// A signature public key tagged with its [`SigAlg`].
    PqPublicKey,
    SigAlg,
    public_key_len
);
tagged_type!(
    /// A signature tagged with its [`SigAlg`].
    PqSignature,
    SigAlg,
    signature_len
);
tagged_type!(
    /// A KEM encapsulation (public) key tagged with its [`KemAlg`].
    KemPublicKey,
    KemAlg,
    public_key_len
);
tagged_type!(
    /// A KEM ciphertext tagged with its [`KemAlg`].
    KemCiphertext,
    KemAlg,
    ciphertext_len
);

#[cfg(test)]
mod tests {
    use super::*;

    // Requirement "带标签的线格式" / Scenario "长度不符被拒绝".
    #[test]
    fn rejects_short_and_long_payloads() {
        let ok = alloc::vec![0u8; 2420];
        assert!(PqSignature::new(SigAlg::MlDsa44, &ok).is_ok());
        assert_eq!(
            PqSignature::new(SigAlg::MlDsa44, &ok[..2419]),
            Err(Error::InvalidLength {
                expected: 2420,
                actual: 2419
            })
        );
        let mut long = PqSignature::new(SigAlg::MlDsa44, &ok)
            .unwrap()
            .to_canonical();
        long.push(0);
        assert_eq!(
            PqSignature::from_canonical(&long),
            Err(Error::InvalidLength {
                expected: 2420,
                actual: 2421
            })
        );
    }

    // Requirement "未知与预留算法的安全处理" / Scenario "未知 AlgId".
    #[test]
    fn rejects_unknown_alg_id() {
        let bytes = [0xFF, 0xFF, 1, 2, 3];
        assert_eq!(
            PqPublicKey::from_canonical(&bytes),
            Err(Error::UnknownAlgorithm(0xFFFF))
        );
        assert_eq!(
            KemCiphertext::from_canonical(&bytes),
            Err(Error::UnknownAlgorithm(0xFFFF))
        );
    }

    #[test]
    fn rejects_reserved_alg() {
        let bytes = [0x01, 0x02, 0, 0];
        assert_eq!(
            PqPublicKey::from_canonical(&bytes),
            Err(Error::NotImplemented(0x0201))
        );
    }

    #[test]
    fn rejects_truncated_prefix() {
        assert_eq!(
            PqPublicKey::from_canonical(&[0x01]),
            Err(Error::InvalidLength {
                expected: 2,
                actual: 1
            })
        );
    }

    // Scenario "编码再解码得到相同对象".
    #[test]
    fn ml_dsa_65_public_key_round_trip() {
        let raw = alloc::vec![7u8; 1952];
        let pk = PqPublicKey::new(SigAlg::MlDsa65, &raw).unwrap();
        let enc = pk.to_canonical();
        assert_eq!(enc.len(), 2 + 1952);
        assert_eq!(enc.get(..2), Some(&[0x02, 0x01][..]));
        assert_eq!(PqPublicKey::from_canonical(&enc).unwrap(), pk);
    }
}
