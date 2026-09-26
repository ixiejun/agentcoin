//! Algorithm-tagged byte objects and their canonical wire format.
//!
//! Each tagged type is an enum with one variant per implemented algorithm. The variant's
//! SCALE index is the algorithm's 1-byte AlgId and its payload is the algorithm's
//! fixed-length raw bytes, so:
//!
//! canonical encoding = `AlgId ‖ raw bytes` = SCALE encoding = on-chain encoding,
//!
//! and the derived `TypeInfo` describes exactly those bytes (decision D34).

use alloc::boxed::Box;
use alloc::vec::Vec;

use crate::alg::{KemAlg, SigAlg};
use crate::error::Error;

/// Length of the AlgId prefix in the canonical encoding.
pub const ALG_ID_LEN: usize = 1;

/// Copies `raw` into a boxed fixed-size array, checking the length.
fn boxed<const N: usize>(raw: &[u8]) -> Result<Box<[u8; N]>, Error> {
    let array: [u8; N] = raw.try_into().map_err(|_| Error::InvalidLength {
        expected: N,
        actual: raw.len(),
    })?;
    Ok(Box::new(array))
}

/// Splits a canonical encoding into its AlgId and raw payload.
fn split_prefix(bytes: &[u8]) -> Result<(u8, &[u8]), Error> {
    match bytes {
        [id, rest @ ..] => Ok((*id, rest)),
        [] => Err(Error::InvalidLength {
            expected: ALG_ID_LEN,
            actual: 0,
        }),
    }
}

macro_rules! tagged_enum {
    (
        $(#[$doc:meta])*
        $name:ident, $alg:ident, $len_fn:ident {
            $( $variant:ident = $index:literal, $len:literal; )+
        }
    ) => {
        $(#[$doc])*
        ///
        /// Variants are boxed so that large post-quantum objects do not live on the stack.
        #[non_exhaustive]
        #[derive(Clone, PartialEq, Eq, Hash)]
        #[cfg_attr(
            feature = "scale",
            derive(
                parity_scale_codec::Encode,
                parity_scale_codec::Decode,
                parity_scale_codec::MaxEncodedLen,
                scale_info::TypeInfo
            )
        )]
        pub enum $name {
            $(
                #[doc = concat!("Raw bytes for `", stringify!($alg), "::", stringify!($variant), "`.")]
                #[cfg_attr(feature = "scale", codec(index = $index))]
                $variant(Box<[u8; $len]>),
            )+
        }

        impl $name {
            /// Builds the object from an algorithm and its raw bytes.
            ///
            /// # Errors
            ///
            /// [`Error::NotImplemented`] for reserved algorithms and
            /// [`Error::InvalidLength`] if `raw` has the wrong length.
            pub fn new(alg: $alg, raw: &[u8]) -> Result<Self, Error> {
                match alg {
                    $( $alg::$variant => Ok(Self::$variant(boxed::<$len>(raw)?)), )+
                    other => Err(Error::NotImplemented(other.id())),
                }
            }

            /// The algorithm of this object.
            #[must_use]
            pub const fn alg(&self) -> $alg {
                match self {
                    $( Self::$variant(_) => $alg::$variant, )+
                }
            }

            /// The raw bytes, without the AlgId prefix.
            #[must_use]
            pub fn as_bytes(&self) -> &[u8] {
                match self {
                    $( Self::$variant(bytes) => bytes.as_slice(), )+
                }
            }

            /// Length of the canonical encoding.
            #[must_use]
            pub fn encoded_len(&self) -> usize {
                ALG_ID_LEN.saturating_add(self.as_bytes().len())
            }

            /// Canonical encoding: the 1-byte AlgId followed by the raw bytes.
            #[must_use]
            pub fn to_canonical(&self) -> Vec<u8> {
                let mut out = Vec::with_capacity(self.encoded_len());
                out.push(self.alg().id());
                out.extend_from_slice(self.as_bytes());
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
                Self::new($alg::from_id(id)?, raw)
            }
        }

        impl core::fmt::Debug for $name {
            fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
                f.debug_struct(stringify!($name))
                    .field("alg", &self.alg())
                    .field("len", &self.as_bytes().len())
                    .finish()
            }
        }

        // The AlgId table and the enum indices must never drift apart.
        const _: () = {
            $( assert!($alg::$variant.id() == $index); )+
        };
    };
}

tagged_enum!(
    /// A signature public key tagged with its [`SigAlg`].
    PqPublicKey, SigAlg, public_key_len {
        MlDsa44 = 0x01, 1312;
        MlDsa65 = 0x02, 1952;
        MlDsa87 = 0x03, 2592;
    }
);

tagged_enum!(
    /// A signature tagged with its [`SigAlg`].
    PqSignature, SigAlg, signature_len {
        MlDsa44 = 0x01, 2420;
        MlDsa65 = 0x02, 3309;
        MlDsa87 = 0x03, 4627;
    }
);

tagged_enum!(
    /// A KEM encapsulation (public) key tagged with its [`KemAlg`].
    KemPublicKey, KemAlg, public_key_len {
        XWing = 0x01, 1216;
    }
);

tagged_enum!(
    /// A KEM ciphertext tagged with its [`KemAlg`].
    KemCiphertext, KemAlg, ciphertext_len {
        XWing = 0x01, 1120;
    }
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
        let bytes = [0xEE, 1, 2, 3];
        assert_eq!(
            PqPublicKey::from_canonical(&bytes),
            Err(Error::UnknownAlgorithm(0xEE))
        );
        assert_eq!(
            KemCiphertext::from_canonical(&bytes),
            Err(Error::UnknownAlgorithm(0xEE))
        );
    }

    #[test]
    fn rejects_reserved_alg() {
        assert_eq!(
            PqPublicKey::from_canonical(&[0x10, 0, 0]),
            Err(Error::NotImplemented(0x10))
        );
    }

    #[test]
    fn rejects_empty_input() {
        assert_eq!(
            PqPublicKey::from_canonical(&[]),
            Err(Error::InvalidLength {
                expected: 1,
                actual: 0
            })
        );
    }

    // Scenario "编码再解码得到相同对象".
    #[test]
    fn ml_dsa_65_public_key_round_trip() {
        let raw = alloc::vec![7u8; 1952];
        let pk = PqPublicKey::new(SigAlg::MlDsa65, &raw).unwrap();
        let enc = pk.to_canonical();
        assert_eq!(enc.len(), 1 + 1952);
        assert_eq!(enc.first(), Some(&0x02));
        assert_eq!(PqPublicKey::from_canonical(&enc).unwrap(), pk);
    }

    #[test]
    fn lengths_match_the_alg_table() {
        for alg in [SigAlg::MlDsa44, SigAlg::MlDsa65, SigAlg::MlDsa87] {
            let pk = PqPublicKey::new(alg, &alloc::vec![0; alg.public_key_len().unwrap()]);
            let sig = PqSignature::new(alg, &alloc::vec![0; alg.signature_len().unwrap()]);
            assert!(pk.is_ok() && sig.is_ok());
        }
    }
}
