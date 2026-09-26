//! BLAKE3-256 as the chain hasher (decision D35).
//!
//! Block header hashes, the extrinsics root and the state trie all use this hasher, so every
//! integrity commitment on chain is a 256-bit BLAKE3 output. The digest itself is computed by
//! `ac-crypto`; this module only adapts it to the SDK's hasher traits.

use alloc::vec::Vec;

use hash256_std_hasher::Hash256StdHasher;
use sp_core::H256;
use sp_runtime::StateVersion;
use sp_trie::{LayoutV0, LayoutV1, TrieConfiguration};

/// BLAKE3 with a 32-byte output, usable as `frame_system::Config::Hashing` and as the state
/// trie hasher.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, scale_info::TypeInfo, serde::Serialize, serde::Deserialize,
)]
pub struct Blake3Hasher;

impl hash_db::Hasher for Blake3Hasher {
    type Out = H256;
    type StdHasher = Hash256StdHasher;
    const LENGTH: usize = 32;

    fn hash(data: &[u8]) -> H256 {
        H256(ac_crypto::hash::blake3_256(data))
    }
}

impl sp_runtime::traits::Hash for Blake3Hasher {
    type Output = H256;

    // Computed in the runtime rather than through a host function: the SDK only exposes
    // BLAKE2 and Keccak trie roots as host functions.
    fn ordered_trie_root(input: Vec<Vec<u8>>, state_version: StateVersion) -> H256 {
        match state_version {
            StateVersion::V0 => LayoutV0::<Self>::ordered_trie_root(input),
            StateVersion::V1 => LayoutV1::<Self>::ordered_trie_root(input),
        }
    }

    fn trie_root(input: Vec<(Vec<u8>, Vec<u8>)>, state_version: StateVersion) -> H256 {
        match state_version {
            StateVersion::V0 => LayoutV0::<Self>::trie_root(input),
            StateVersion::V1 => LayoutV1::<Self>::trie_root(input),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;
    use sp_runtime::traits::Hash as _;

    /// Reference hasher built directly on the `blake3` crate, independent of `ac-crypto`.
    #[derive(Debug)]
    struct RefBlake3;

    impl hash_db::Hasher for RefBlake3 {
        type Out = H256;
        type StdHasher = Hash256StdHasher;
        const LENGTH: usize = 32;
        fn hash(data: &[u8]) -> H256 {
            H256(*blake3::hash(data).as_bytes())
        }
    }

    /// BLAKE3 official test-vector input: bytes 0..=250 repeating.
    fn vector_input(len: usize) -> Vec<u8> {
        (0..len).map(|i| u8::try_from(i % 251).unwrap()).collect()
    }

    // Requirement "区块与状态承诺使用 BLAKE3-256": official BLAKE3 vectors (test_vectors.json).
    #[test]
    fn matches_official_blake3_vectors() {
        let cases = [
            (
                0,
                "af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262",
            ),
            (
                1,
                "2d3adedff11b61f14c886e35afa036736dcd87a74d27b5c1510225d0f592e213",
            ),
            (
                1024,
                "42214739f095a406f3fc83deb889744ac00df831c10daa55189b5d121c855af7",
            ),
        ];
        for (len, expected) in cases {
            let out = <Blake3Hasher as hash_db::Hasher>::hash(&vector_input(len));
            assert_eq!(hex::encode(out.0), expected, "input length {len}");
        }
    }

    #[test]
    fn ordered_trie_root_matches_reference() {
        let input: Vec<Vec<u8>> = (0u8..20).map(|i| vec![i; usize::from(i) * 7]).collect();
        for version in [StateVersion::V0, StateVersion::V1] {
            let ours = Blake3Hasher::ordered_trie_root(input.clone(), version);
            let reference = match version {
                StateVersion::V0 => LayoutV0::<RefBlake3>::ordered_trie_root(input.clone()),
                StateVersion::V1 => LayoutV1::<RefBlake3>::ordered_trie_root(input.clone()),
            };
            assert_eq!(ours, reference);
        }
    }

    #[test]
    fn trie_root_matches_reference() {
        let input: Vec<(Vec<u8>, Vec<u8>)> = (0u8..50)
            .map(|i| (vec![i, i.wrapping_mul(3)], vec![i; usize::from(i) + 40]))
            .collect();
        let ours = Blake3Hasher::trie_root(input.clone(), StateVersion::V1);
        assert_eq!(ours, LayoutV1::<RefBlake3>::trie_root(input));
    }

    #[test]
    fn empty_trie_root_is_hash_of_empty_node() {
        let root = Blake3Hasher::trie_root(Vec::new(), StateVersion::V1);
        assert_eq!(root.0, *blake3::hash(&[0u8]).as_bytes());
    }
}
