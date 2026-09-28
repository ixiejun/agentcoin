//! Model manifests, model IDs and model records (m5-market-registry design D4).
//!
//! A model ID is the domain-separated BLAKE3 hash of the SCALE encoding of its weight manifest
//! (name, architecture, quantization, ordered shard hashes). The chain computes it at
//! registration; anyone can recompute it off chain from the same manifest.

use parity_scale_codec::{Decode, DecodeWithMemTracking, Encode, MaxEncodedLen};
use scale_info::TypeInfo;
use sp_core::ConstU32;
use sp_runtime::BoundedVec;

/// Hashing context of model IDs.
pub const MODEL_ID_CONTEXT: &str = "agentcoin 2026-09 model-id v1";

/// Longest model name, in bytes.
pub const MAX_MODEL_NAME_LEN: u32 = 128;
/// Longest architecture name, in bytes.
pub const MAX_ARCH_LEN: u32 = 128;
/// Most weight shards in a manifest.
pub const MAX_SHARDS: u32 = 1_024;
/// Longest licence tag, in bytes.
pub const MAX_LICENSE_TAG_LEN: u32 = 64;

/// A model ID: the BLAKE3-256 hash of the model's manifest.
#[derive(
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Encode,
    Decode,
    DecodeWithMemTracking,
    MaxEncodedLen,
    TypeInfo,
)]
pub struct ModelId(pub [u8; 32]);

impl core::fmt::Debug for ModelId {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("0x")?;
        self.0.iter().try_for_each(|b| write!(f, "{b:02x}"))
    }
}

/// Weight format of a model.
///
/// Wire-format enum: discriminants are explicit and never reused (AGENT.md §5.6); new formats
/// are only appended.
#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    Encode,
    Decode,
    DecodeWithMemTracking,
    MaxEncodedLen,
    TypeInfo,
)]
#[repr(u8)]
pub enum QuantType {
    /// bfloat16.
    #[codec(index = 1)]
    Bf16 = 1,
    /// IEEE half precision.
    #[codec(index = 2)]
    Fp16 = 2,
    /// 8-bit floating point.
    #[codec(index = 3)]
    Fp8 = 3,
    /// 8-bit integers.
    #[codec(index = 4)]
    Int8 = 4,
    /// 4-bit integers (e.g. AWQ, GPTQ, GGUF Q4).
    #[codec(index = 5)]
    Int4 = 5,
}

/// How a model derives from its declared parent (D28).
///
/// Wire-format enum: discriminants are explicit and never reused.
#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    Encode,
    Decode,
    DecodeWithMemTracking,
    MaxEncodedLen,
    TypeInfo,
)]
#[repr(u8)]
pub enum LineageKind {
    /// Fine-tuned from the parent.
    #[codec(index = 1)]
    Finetune = 1,
    /// A quantized copy of the parent.
    #[codec(index = 2)]
    Quantize = 2,
    /// Distilled from the parent.
    #[codec(index = 3)]
    Distill = 3,
    /// Merged from the parent and others.
    #[codec(index = 4)]
    Merge = 4,
}

/// A declared parent model. The protocol records it but does not verify it (D28).
#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    Encode,
    Decode,
    DecodeWithMemTracking,
    MaxEncodedLen,
    TypeInfo,
)]
pub struct Lineage {
    /// The parent model; it must be registered.
    pub parent: ModelId,
    /// How this model derives from it.
    pub kind: LineageKind,
}

/// \[Reserved\] Royalty of community models (D22), enabled in the full version. Registering a
/// model with a royalty fails in the MVP.
#[derive(
    Clone, Debug, PartialEq, Eq, Encode, Decode, DecodeWithMemTracking, MaxEncodedLen, TypeInfo,
)]
pub struct RoyaltySpec<AccountId> {
    /// Receiver of the royalty.
    pub beneficiary: AccountId,
    /// Share of inference fees, in basis points.
    pub bps: u16,
}

/// The canonical weight manifest a model ID is computed from.
#[derive(
    Clone, Debug, PartialEq, Eq, Encode, Decode, DecodeWithMemTracking, MaxEncodedLen, TypeInfo,
)]
pub struct ModelManifest {
    /// Model name (UTF-8), e.g. `Qwen2.5-0.5B-Instruct`.
    pub name: BoundedVec<u8, ConstU32<MAX_MODEL_NAME_LEN>>,
    /// Architecture name (UTF-8), e.g. `qwen2`.
    pub arch: BoundedVec<u8, ConstU32<MAX_ARCH_LEN>>,
    /// Weight format.
    pub quant: QuantType,
    /// BLAKE3-256 of each weight shard, in load order.
    pub shards: BoundedVec<[u8; 32], ConstU32<MAX_SHARDS>>,
}

/// Why a manifest is not acceptable.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ModelError {
    /// The name or architecture is empty or not UTF-8.
    InvalidText,
    /// The manifest lists no shard.
    NoShards,
    /// A field exceeds its bound.
    TooLong,
    /// The hashing context was rejected (never happens with the published context).
    Hashing,
}

impl core::fmt::Display for ModelError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match self {
            Self::InvalidText => "the name and architecture must be non-empty UTF-8",
            Self::NoShards => "the manifest lists no weight shard",
            Self::TooLong => "a manifest field exceeds its bound",
            Self::Hashing => "the model-ID hashing context was rejected",
        })
    }
}

#[cfg(feature = "std")]
impl std::error::Error for ModelError {}

impl ModelManifest {
    /// Builds a manifest from unbounded parts, checking every bound.
    ///
    /// # Errors
    ///
    /// [`ModelError::TooLong`] if a field exceeds its bound; the checks of
    /// [`ModelManifest::validate`].
    pub fn new(
        name: &[u8],
        arch: &[u8],
        quant: QuantType,
        shards: alloc::vec::Vec<[u8; 32]>,
    ) -> Result<Self, ModelError> {
        let manifest = Self {
            name: name.to_vec().try_into().map_err(|_| ModelError::TooLong)?,
            arch: arch.to_vec().try_into().map_err(|_| ModelError::TooLong)?,
            quant,
            shards: shards.try_into().map_err(|_| ModelError::TooLong)?,
        };
        manifest.validate()?;
        Ok(manifest)
    }

    /// Checks what the bounded types cannot: non-empty UTF-8 text and at least one shard.
    ///
    /// # Errors
    ///
    /// [`ModelError::InvalidText`] or [`ModelError::NoShards`].
    pub fn validate(&self) -> Result<(), ModelError> {
        for text in [&self.name, &self.arch] {
            if text.is_empty() || core::str::from_utf8(text).is_err() {
                return Err(ModelError::InvalidText);
            }
        }
        if self.shards.is_empty() {
            return Err(ModelError::NoShards);
        }
        Ok(())
    }

    /// The model ID: `derive("agentcoin 2026-09 model-id v1", SCALE(manifest))`.
    ///
    /// # Errors
    ///
    /// The checks of [`ModelManifest::validate`].
    pub fn id(&self) -> Result<ModelId, ModelError> {
        self.validate()?;
        ac_crypto::hash::derive(MODEL_ID_CONTEXT, &self.encode())
            .map(ModelId)
            .map_err(|_| ModelError::Hashing)
    }
}

/// A registered model.
#[derive(
    Clone, Debug, PartialEq, Eq, Encode, Decode, DecodeWithMemTracking, MaxEncodedLen, TypeInfo,
)]
pub struct ModelRecord<AccountId, Balance, BlockNumber> {
    /// The account that registered the model and holds its deposit.
    pub owner: AccountId,
    /// The manifest the ID was computed from.
    pub manifest: ModelManifest,
    /// Declared parent model, if any.
    pub lineage: Option<Lineage>,
    /// Informational licence tag (UTF-8); the protocol never acts on it (D6).
    pub license_tag: BoundedVec<u8, ConstU32<MAX_LICENSE_TAG_LEN>>,
    /// \[Reserved\] Always `None` in the MVP.
    pub royalty: Option<RoyaltySpec<AccountId>>,
    /// Storage deposit held on the owner.
    pub deposit: Balance,
    /// Block of registration.
    pub registered_at: BlockNumber,
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    fn shard(n: u8) -> [u8; 32] {
        [n; 32]
    }

    fn hex(id: &ModelId) -> alloc::string::String {
        alloc::format!("{id:?}")
    }

    // Regression vectors (design D4). The model-ID rule is published: never change these.
    #[test]
    fn model_id_vectors() {
        let one = ModelManifest::new(b"tiny", b"llama", QuantType::Bf16, vec![shard(1)]).unwrap();
        let two = ModelManifest::new(
            b"Qwen2.5-0.5B-Instruct",
            b"qwen2",
            QuantType::Int4,
            vec![shard(1), shard(2)],
        )
        .unwrap();
        let swapped = ModelManifest::new(
            b"Qwen2.5-0.5B-Instruct",
            b"qwen2",
            QuantType::Int4,
            vec![shard(2), shard(1)],
        )
        .unwrap();
        assert_eq!(hex(&one.id().unwrap()), VECTOR_ONE);
        assert_eq!(hex(&two.id().unwrap()), VECTOR_TWO);
        assert_eq!(hex(&swapped.id().unwrap()), VECTOR_SWAPPED);
    }

    const VECTOR_ONE: &str = "0xf55dd61bcbf35b385f77ab778785b63d7488912ac15bbc6405baf245e94b38f1";
    const VECTOR_TWO: &str = "0x226bb1bf7ef7963e7bc8f666002192b287323e1343ab59f9f261cafa0fa8a1ab";
    const VECTOR_SWAPPED: &str =
        "0xcc1e7a74858e1cf586ed9e377174dbf74befe4f60bfcf087a14378e2a62b5fb3";

    // Spec market/model-registry, Scenario "分片顺序影响 ID".
    #[test]
    fn shard_order_changes_the_id() {
        let a = ModelManifest::new(b"m", b"a", QuantType::Fp8, vec![shard(1), shard(2)]).unwrap();
        let b = ModelManifest::new(b"m", b"a", QuantType::Fp8, vec![shard(2), shard(1)]).unwrap();
        assert_ne!(a.id().unwrap(), b.id().unwrap());
    }

    #[test]
    fn the_id_is_the_derived_hash_of_the_encoding() {
        let m = ModelManifest::new(b"m", b"a", QuantType::Int8, vec![shard(9)]).unwrap();
        let expected = ac_crypto::hash::derive(MODEL_ID_CONTEXT, &m.encode()).unwrap();
        assert_eq!(m.id().unwrap(), ModelId(expected));
        // Cross-check with the reference BLAKE3 implementation (derive_key mode).
        assert_eq!(
            m.id().unwrap(),
            ModelId(blake3::derive_key(MODEL_ID_CONTEXT, &m.encode()))
        );
    }

    #[test]
    fn invalid_manifests_are_rejected() {
        assert_eq!(
            ModelManifest::new(&[0xff, 0xfe], b"a", QuantType::Fp16, vec![shard(1)]),
            Err(ModelError::InvalidText)
        );
        assert_eq!(
            ModelManifest::new(b"", b"a", QuantType::Fp16, vec![shard(1)]),
            Err(ModelError::InvalidText)
        );
        assert_eq!(
            ModelManifest::new(b"m", b"a", QuantType::Fp16, vec![]),
            Err(ModelError::NoShards)
        );
        // Spec Scenario "超出上限": 1,025 shards do not fit.
        assert_eq!(
            ModelManifest::new(b"m", b"a", QuantType::Fp16, vec![shard(1); 1_025]),
            Err(ModelError::TooLong)
        );
        assert_eq!(
            ModelManifest::new(&[b'x'; 129], b"a", QuantType::Fp16, vec![shard(1)]),
            Err(ModelError::TooLong)
        );
        assert!(ModelManifest::new(b"m", b"a", QuantType::Fp16, vec![shard(1); 1_024]).is_ok());
    }

    #[test]
    fn oversized_manifests_do_not_decode() {
        // A manifest with 1,025 shards, encoded without bounds, is rejected by the decoder.
        let raw = (
            b"m".to_vec(),
            b"a".to_vec(),
            QuantType::Fp16,
            vec![shard(1); 1_025],
        )
            .encode();
        assert!(ModelManifest::decode(&mut &raw[..]).is_err());
    }

    #[test]
    fn wire_discriminants_are_fixed() {
        assert_eq!(QuantType::Bf16.encode(), [1]);
        assert_eq!(QuantType::Int4.encode(), [5]);
        assert_eq!(LineageKind::Finetune.encode(), [1]);
        assert_eq!(LineageKind::Merge.encode(), [4]);
    }
}
