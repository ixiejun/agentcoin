//! Merkle tree over the receipts of a work report (m5-work-settlement design D3; spec
//! `market/work-settlement`, "收据承诺").
//!
//! - leaf = `derive("agentcoin 2026-09 receipt-leaf v1", SCALE(SignedReceipt))`;
//! - node = `derive("agentcoin 2026-09 receipt-node v1", left ‖ right)`;
//! - the last node of a level with an odd count moves up unchanged (it is not paired with a copy
//!   of itself, so two different leaf lists never share a root that way);
//! - an empty tree has no root.
//!
//! Leaves and nodes use different hashing contexts, so a node can never be passed off as a leaf.

use alloc::vec::Vec;
use parity_scale_codec::{Decode, DecodeWithMemTracking, Encode, MaxEncodedLen};
use scale_info::TypeInfo;

use super::receipt::SignedReceipt;

/// Hashing context of receipt leaves.
pub const RECEIPT_LEAF_CONTEXT: &str = "agentcoin 2026-09 receipt-leaf v1";

/// Hashing context of inner nodes.
pub const RECEIPT_NODE_CONTEXT: &str = "agentcoin 2026-09 receipt-node v1";

/// A 32-byte leaf, node or root.
pub type Hash = [u8; 32];

/// Which side of the path the sibling is on.
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
    serde::Serialize,
    serde::Deserialize,
)]
pub enum Side {
    /// The sibling is the left child: `node(sibling, current)`.
    Left,
    /// The sibling is the right child: `node(current, sibling)`.
    Right,
}

/// One level of an inclusion proof, bottom up.
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
    serde::Serialize,
    serde::Deserialize,
)]
pub struct ProofStep {
    /// The sibling node.
    pub sibling: Hash,
    /// Its side.
    pub side: Side,
}

/// Why a tree or proof cannot be built.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum TreeError {
    /// No leaves.
    Empty,
    /// The leaf index is past the last leaf.
    IndexOutOfRange,
    /// A hashing context was rejected (does not happen with the published contexts).
    Hashing,
}

impl core::fmt::Display for TreeError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match self {
            Self::Empty => "a receipt tree needs at least one receipt",
            Self::IndexOutOfRange => "the receipt index is past the last receipt",
            Self::Hashing => "the hashing context was rejected",
        })
    }
}

#[cfg(feature = "std")]
impl std::error::Error for TreeError {}

/// The leaf of a signed receipt.
///
/// # Errors
///
/// [`TreeError::Hashing`] only if the published context were rejected.
pub fn leaf(receipt: &SignedReceipt) -> Result<Hash, TreeError> {
    ac_crypto::hash::derive(RECEIPT_LEAF_CONTEXT, &receipt.encode()).map_err(|_| TreeError::Hashing)
}

fn node(left: &Hash, right: &Hash) -> Result<Hash, TreeError> {
    let mut both = [0u8; 64];
    let (l, r) = both.split_at_mut(32);
    l.copy_from_slice(left);
    r.copy_from_slice(right);
    ac_crypto::hash::derive(RECEIPT_NODE_CONTEXT, &both).map_err(|_| TreeError::Hashing)
}

/// The level above `level`: pairs hashed, an odd last node moved up.
fn up(level: &[Hash]) -> Result<Vec<Hash>, TreeError> {
    level
        .chunks(2)
        .map(|pair| match pair {
            [l, r] => node(l, r),
            [single] => Ok(*single),
            _ => Err(TreeError::Empty),
        })
        .collect()
}

/// Root of the tree over `leaves` (in submission order).
///
/// # Errors
///
/// [`TreeError::Empty`] without leaves.
pub fn root(leaves: &[Hash]) -> Result<Hash, TreeError> {
    let mut level = leaves.to_vec();
    loop {
        match level.as_slice() {
            [] => return Err(TreeError::Empty),
            [only] => return Ok(*only),
            _ => level = up(&level)?,
        }
    }
}

/// Inclusion proof of leaf `index`. A level where the leaf's node moves up unpaired adds no
/// step.
///
/// # Errors
///
/// [`TreeError::Empty`] without leaves, [`TreeError::IndexOutOfRange`] past the last leaf.
pub fn proof(leaves: &[Hash], index: usize) -> Result<Vec<ProofStep>, TreeError> {
    if leaves.is_empty() {
        return Err(TreeError::Empty);
    }
    if index >= leaves.len() {
        return Err(TreeError::IndexOutOfRange);
    }
    let mut steps = Vec::new();
    let mut level = leaves.to_vec();
    let mut i = index;
    while level.len() > 1 {
        let sibling = if i.is_multiple_of(2) {
            level.get(i.saturating_add(1)).map(|s| (*s, Side::Right))
        } else {
            level.get(i.saturating_sub(1)).map(|s| (*s, Side::Left))
        };
        if let Some((sibling, side)) = sibling {
            steps.push(ProofStep { sibling, side });
        }
        level = up(&level)?;
        i /= 2;
    }
    Ok(steps)
}

/// Whether `proof` shows `leaf` under `root`.
#[must_use]
pub fn verify_proof(leaf: &Hash, proof: &[ProofStep], root: &Hash) -> bool {
    let mut current = *leaf;
    for step in proof {
        let next = match step.side {
            Side::Left => node(&step.sibling, &current),
            Side::Right => node(&current, &step.sibling),
        };
        match next {
            Ok(n) => current = n,
            Err(_) => return false,
        }
    }
    current == *root
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn leaves(n: u8) -> Vec<Hash> {
        (0..n).map(|i| [i; 32]).collect()
    }

    fn hexs(bytes: &[u8]) -> alloc::string::String {
        bytes.iter().map(|b| alloc::format!("{b:02x}")).collect()
    }

    // Regression vectors (design D3): roots over leaves `[i; 32]` for i in 0..n. The tree rule
    // is published: never change these.
    #[test]
    fn root_vectors() {
        for (n, expected) in ROOT_VECTORS {
            assert_eq!(hexs(&root(&leaves(n)).unwrap()), expected, "{n} leaves");
        }
        // A single leaf is its own root.
        assert_eq!(root(&leaves(1)).unwrap(), [0; 32]);
    }

    const ROOT_VECTORS: [(u8, &str); 5] = [
        (
            1,
            "0000000000000000000000000000000000000000000000000000000000000000",
        ),
        (
            2,
            "beb38b4f401334b847175467c733af16fa621cf9410ea75aad2693f585bf8455",
        ),
        (
            3,
            "5dbcdf931dc6a5a3cf03f2c29a1099e47305b68319e629b3ad944410c2b43ac1",
        ),
        (
            5,
            "905ea411cae2e935e71fb165e947ddc7f1ff52623d53b43ab2643a158e015ce5",
        ),
        (
            8,
            "281c58737004e25964232cf1a0c20a1643201359854e48da25b6927c2e56219f",
        ),
    ];

    // Proof shape vectors: sides and sibling count per leaf for 5 leaves.
    #[test]
    fn proof_vectors() {
        let l = leaves(5);
        let shape = |i| {
            proof(&l, i)
                .unwrap()
                .iter()
                .map(|s| s.side)
                .collect::<Vec<_>>()
        };
        use Side::{Left, Right};
        assert_eq!(shape(0), [Right, Right, Right]);
        assert_eq!(shape(3), [Left, Left, Right]);
        // The fifth leaf moves up unpaired twice, then meets the four-leaf subtree.
        assert_eq!(shape(4), [Left]);
        let n01 = node(&l[0], &l[1]).unwrap();
        let n23 = node(&l[2], &l[3]).unwrap();
        let n0123 = node(&n01, &n23).unwrap();
        assert_eq!(proof(&l, 4).unwrap()[0].sibling, n0123);
        assert_eq!(root(&l).unwrap(), node(&n0123, &l[4]).unwrap());
    }

    // Scenario "包含证明": the fifth of five receipts.
    #[test]
    fn inclusion_proof_of_the_fifth_receipt() {
        let l = leaves(5);
        let r = root(&l).unwrap();
        let p = proof(&l, 4).unwrap();
        assert!(verify_proof(&l[4], &p, &r));
        assert!(!verify_proof(&[9; 32], &p, &r));
    }

    // Scenario "顺序影响根".
    #[test]
    fn order_changes_the_root() {
        let mut l = leaves(3);
        let before = root(&l).unwrap();
        l.swap(0, 1);
        assert_ne!(root(&l).unwrap(), before);
    }

    #[test]
    fn empty_trees_and_bad_indices_are_rejected() {
        assert_eq!(root(&[]), Err(TreeError::Empty));
        assert_eq!(proof(&[], 0), Err(TreeError::Empty));
        assert_eq!(proof(&leaves(3), 3), Err(TreeError::IndexOutOfRange));
    }

    // An odd last node is not duplicated: [a, b, c] and [a, b, c, c] have different roots.
    #[test]
    fn a_duplicated_last_leaf_changes_the_root() {
        let mut l = leaves(3);
        let three = root(&l).unwrap();
        l.push(l[2]);
        assert_ne!(root(&l).unwrap(), three);
    }

    proptest! {
        #[test]
        fn every_leaf_proves_and_substitutes_fail(
            raw in proptest::collection::vec(any::<[u8; 32]>(), 1..40),
            pick in any::<prop::sample::Index>(),
            other in any::<[u8; 32]>(),
        ) {
            let r = root(&raw).unwrap();
            let i = pick.index(raw.len());
            let p = proof(&raw, i).unwrap();
            prop_assert!(verify_proof(&raw[i], &p, &r));
            if other != raw[i] {
                prop_assert!(!verify_proof(&other, &p, &r));
            }
        }
    }
}
