//! Seals and header checks: pure functions shared by the block author and the import verifier.

use ac_crypto::sig::{SigningKey, verify};
use ac_crypto::{PqPublicKey, PqSignature};
use ac_primitives::aura_pq::{DigestError, ENGINE_ID, SEAL_CONTEXT, Slot, find_slot, slot_author};
use parity_scale_codec::Decode;
use sp_runtime::DigestItem;
use sp_runtime::traits::Header as HeaderT;

/// Why a header fails the Aura-PQ checks.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SealError {
    /// The last digest item is not an Aura-PQ seal.
    Unsealed,
    /// The seal does not decode as a tagged signature.
    BadSeal,
    /// The seal signature does not verify under the slot author's key.
    BadSignature,
    /// No authority is assigned to the slot (empty authority set).
    SlotAuthorNotFound,
    /// The slot digest is missing, duplicated or malformed.
    Digest(DigestError),
    /// The slot lies beyond the allowed clock drift; the header may become valid later.
    FutureSlot(Slot),
    /// The slot is not greater than the parent's slot.
    SlotNotIncreasing {
        /// Slot of the parent block.
        parent: Slot,
        /// Slot claimed by the header.
        slot: Slot,
    },
}

impl core::fmt::Display for SealError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Unsealed => f.write_str("header is not sealed"),
            Self::BadSeal => f.write_str("malformed seal"),
            Self::BadSignature => f.write_str("invalid seal signature"),
            Self::SlotAuthorNotFound => f.write_str("no authority for this slot"),
            Self::Digest(e) => write!(f, "bad slot digest: {e:?}"),
            Self::FutureSlot(s) => write!(f, "slot {s} is too far in the future"),
            Self::SlotNotIncreasing { parent, slot } => {
                write!(f, "slot {slot} does not increase over parent slot {parent}")
            }
        }
    }
}

/// Signs the hash of an unsealed header and returns the seal digest item.
///
/// # Errors
///
/// Returns an `ac-crypto` error if the OS randomness for the hedged signature is unavailable.
pub fn seal(key: &SigningKey, pre_hash: &[u8]) -> Result<DigestItem, ac_crypto::Error> {
    let mut rng = ac_crypto::OsRng::new()?;
    let signature = key.sign(pre_hash, SEAL_CONTEXT, &mut rng)?;
    Ok(ac_primitives::aura_pq::seal_digest(&signature))
}

/// Removes the Aura-PQ seal (the last digest item) and returns it with its decoded signature.
///
/// # Errors
///
/// [`SealError::Unsealed`] or [`SealError::BadSeal`].
pub fn take_seal<H: HeaderT>(header: &mut H) -> Result<(DigestItem, PqSignature), SealError> {
    let item = header.digest_mut().pop().ok_or(SealError::Unsealed)?;
    let signature = match &item {
        DigestItem::Seal(id, data) if *id == ENGINE_ID => {
            PqSignature::decode(&mut &data[..]).map_err(|_| SealError::BadSeal)?
        }
        _ => return Err(SealError::Unsealed),
    };
    Ok((item, signature))
}

/// Result of a successful header check.
#[derive(Debug)]
pub struct CheckedHeader<H> {
    /// The header without its seal.
    pub pre_header: H,
    /// The slot the header claims.
    pub slot: Slot,
    /// The seal digest item, to re-attach as a post-digest.
    pub seal: DigestItem,
    /// The authority that authored the slot.
    pub author: PqPublicKey,
}

/// Checks a sealed header: seal present and well formed, unique slot digest, slot not beyond
/// `slot_now`, slot above the parent's, author assigned to the slot, and valid seal signature
/// over the hash of the unsealed header.
///
/// # Errors
///
/// The first failed check as a [`SealError`].
pub fn check_header<H: HeaderT>(
    mut header: H,
    parent_slot: Option<Slot>,
    slot_now: Slot,
    authorities: &[PqPublicKey],
) -> Result<CheckedHeader<H>, SealError> {
    let (seal, signature) = take_seal(&mut header)?;
    let slot = find_slot(header.digest()).map_err(SealError::Digest)?;
    if slot > slot_now {
        return Err(SealError::FutureSlot(slot));
    }
    if let Some(parent) = parent_slot
        && slot <= parent
    {
        return Err(SealError::SlotNotIncreasing { parent, slot });
    }
    let author = slot_author(slot, authorities)
        .ok_or(SealError::SlotAuthorNotFound)?
        .clone();
    let pre_hash = header.hash();
    verify(&author, pre_hash.as_ref(), SEAL_CONTEXT, &signature)
        .map_err(|_| SealError::BadSignature)?;
    Ok(CheckedHeader {
        pre_header: header,
        slot,
        seal,
        author,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use ac_crypto::SigAlg;
    use ac_crypto::sig::SecretSeed;
    use ac_primitives::aura_pq::pre_digest;
    use sp_runtime::testing::H256;
    use sp_runtime::traits::Hash as _;
    use sp_runtime::{Digest, generic};

    type Header = generic::Header<u32, ac_primitives::Blake3Hasher>;

    fn keys() -> Vec<SigningKey> {
        (1u8..=3)
            .map(|i| SigningKey::from_seed(SigAlg::MlDsa65, &SecretSeed::new([i; 32])).unwrap())
            .collect()
    }

    fn authorities(keys: &[SigningKey]) -> Vec<PqPublicKey> {
        keys.iter().map(|k| k.public_key().unwrap()).collect()
    }

    /// A header for `slot`, sealed by `signer`.
    fn sealed(slot: u64, signer: &SigningKey) -> Header {
        let mut digest = Digest::default();
        digest.push(pre_digest(Slot::from(slot)));
        let mut header = Header::new(1, H256::zero(), H256::zero(), H256::repeat_byte(1), digest);
        let pre_hash = header.hash();
        header
            .digest_mut()
            .push(seal(signer, pre_hash.as_ref()).unwrap());
        header
    }

    // Requirement "ML-DSA-65 出块封印" / Scenario "封印可独立验证".
    #[test]
    fn seal_verifies_independently() {
        let keys = keys();
        let mut header = sealed(4, &keys[1]);
        let (_, signature) = take_seal(&mut header).unwrap();
        let pre_hash = ac_primitives::Blake3Hasher::hash_of(&header);
        assert_eq!(pre_hash, header.hash());
        assert!(
            verify(
                &keys[1].public_key().unwrap(),
                pre_hash.as_ref(),
                SEAL_CONTEXT,
                &signature
            )
            .is_ok()
        );
        // The same signature does not verify under the transaction context.
        assert!(
            verify(
                &keys[1].public_key().unwrap(),
                pre_hash.as_ref(),
                b"agentcoin/tx/v1",
                &signature
            )
            .is_err()
        );
    }

    #[test]
    fn valid_header_passes() {
        let keys = keys();
        let checked = check_header(
            sealed(4, &keys[1]),
            Some(Slot::from(3)),
            Slot::from(5),
            &authorities(&keys),
        )
        .unwrap();
        assert_eq!(checked.slot, Slot::from(4));
        assert_eq!(checked.author, keys[1].public_key().unwrap());
    }

    // Requirement "导入校验" / Scenario "冒名出块": slot 4 belongs to index 1, signed by index 2.
    #[test]
    fn impostor_is_rejected() {
        let keys = keys();
        let result = check_header(
            sealed(4, &keys[2]),
            None,
            Slot::from(5),
            &authorities(&keys),
        );
        assert_eq!(result.unwrap_err(), SealError::BadSignature);
    }

    // Requirement "导入校验" / Scenario "篡改区块头".
    #[test]
    fn tampered_header_is_rejected() {
        let keys = keys();
        let mut header = sealed(4, &keys[1]);
        header.state_root = H256::repeat_byte(9);
        let result = check_header(header, None, Slot::from(5), &authorities(&keys));
        assert_eq!(result.unwrap_err(), SealError::BadSignature);
    }

    // Requirement "导入校验" / Scenario "未来时隙".
    #[test]
    fn future_slot_is_deferred() {
        let keys = keys();
        let result = check_header(
            sealed(9, &keys[0]),
            None,
            Slot::from(5),
            &authorities(&keys),
        );
        assert_eq!(result.unwrap_err(), SealError::FutureSlot(Slot::from(9)));
    }

    // Requirement "时隙与出块人" / Scenario "时隙不递增".
    #[test]
    fn non_increasing_slot_is_rejected() {
        let keys = keys();
        let result = check_header(
            sealed(4, &keys[1]),
            Some(Slot::from(4)),
            Slot::from(5),
            &authorities(&keys),
        );
        assert_eq!(
            result.unwrap_err(),
            SealError::SlotNotIncreasing {
                parent: Slot::from(4),
                slot: Slot::from(4)
            }
        );
    }

    #[test]
    fn unsealed_and_malformed_seals_are_rejected() {
        let keys = keys();
        let mut header = sealed(4, &keys[1]);
        header.digest_mut().pop();
        assert_eq!(
            check_header(header.clone(), None, Slot::from(5), &authorities(&keys)).unwrap_err(),
            SealError::Unsealed
        );
        header
            .digest_mut()
            .push(DigestItem::Seal(ENGINE_ID, vec![0x10, 1, 2]));
        assert_eq!(
            check_header(header, None, Slot::from(5), &authorities(&keys)).unwrap_err(),
            SealError::BadSeal
        );
    }

    #[test]
    fn empty_authority_set_has_no_author() {
        let keys = keys();
        let result = check_header(sealed(4, &keys[1]), None, Slot::from(5), &[]);
        assert_eq!(result.unwrap_err(), SealError::SlotAuthorNotFound);
    }
}
