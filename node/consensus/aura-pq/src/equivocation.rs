//! Detection of two blocks signed by the same authority in the same slot.
//!
//! M1 only records and reports equivocations; slashing arrives with AC-BFT in M2.

use ac_crypto::PqPublicKey;
use ac_primitives::aura_pq::Slot;
use sc_client_api::backend::AuxStore;
use sp_runtime::traits::Header as HeaderT;

/// Log target of equivocation reports.
pub const LOG_TARGET: &str = "aura-pq";

/// Evidence of an equivocation: both headers of one author in one slot, SCALE encoded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EquivocationReport {
    /// The slot.
    pub slot: Slot,
    /// SCALE encoding of the first header seen.
    pub first_header: Vec<u8>,
    /// SCALE encoding of the conflicting header.
    pub second_header: Vec<u8>,
}

/// Records `header` for `(slot, author)` and reports an equivocation if the author already
/// signed a different header in that slot. Headers must be passed with their seal attached,
/// so the report proves that the author signed both.
///
/// # Errors
///
/// Propagates auxiliary-storage errors.
pub fn check<C: AuxStore, H: HeaderT>(
    backend: &C,
    slot_now: Slot,
    slot: Slot,
    header: &H,
    author: &PqPublicKey,
) -> sp_blockchain::Result<Option<EquivocationReport>> {
    let proof = sc_consensus_slots::check_equivocation(backend, slot_now, slot, header, author)?;
    let Some(proof) = proof else {
        return Ok(None);
    };
    let report = EquivocationReport {
        slot,
        first_header: proof.first_header.encode(),
        second_header: proof.second_header.encode(),
    };
    log::warn!(
        target: LOG_TARGET,
        "equivocation: authority {:?} signed two blocks in slot {}: first header 0x{}, second header 0x{}",
        author,
        slot,
        hex::encode(&report.first_header),
        hex::encode(&report.second_header),
    );
    Ok(Some(report))
}

/// On-chain evidence for `report`, signed (sealed) by `author`. `None` if a header exceeds the
/// evidence size limit.
#[must_use]
pub fn evidence(
    report: &EquivocationReport,
    author: &PqPublicKey,
) -> Option<ac_primitives::offences::Evidence> {
    use ac_primitives::offences::{EncodedHeader, Evidence};
    Some(Evidence::AuraEquivocation {
        offender: author.clone(),
        first: EncodedHeader::try_from(report.first_header.clone()).ok()?,
        second: EncodedHeader::try_from(report.second_header.clone()).ok()?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use ac_crypto::SigAlg;
    use ac_crypto::sig::{SecretSeed, SigningKey};
    use parity_scale_codec::Encode;
    use sp_runtime::testing::H256;
    use sp_runtime::{Digest, generic};
    use std::collections::BTreeMap;
    use std::sync::Mutex;

    type Header = generic::Header<u32, ac_primitives::Blake3Hasher>;

    #[derive(Default)]
    struct MemAux(Mutex<BTreeMap<Vec<u8>, Vec<u8>>>);

    impl AuxStore for MemAux {
        fn insert_aux<
            'a,
            'b: 'a,
            'c: 'a,
            I: IntoIterator<Item = &'a (&'c [u8], &'c [u8])>,
            D: IntoIterator<Item = &'a &'b [u8]>,
        >(
            &self,
            insert: I,
            delete: D,
        ) -> sp_blockchain::Result<()> {
            let mut map = self.0.lock().unwrap();
            for (k, v) in insert {
                map.insert(k.to_vec(), v.to_vec());
            }
            for k in delete {
                map.remove(*k);
            }
            Ok(())
        }

        fn get_aux(&self, key: &[u8]) -> sp_blockchain::Result<Option<Vec<u8>>> {
            Ok(self.0.lock().unwrap().get(key).cloned())
        }
    }

    // Requirement "同一时隙多块检测" / Scenario "双签被记录".
    #[test]
    fn two_headers_in_one_slot_are_reported() {
        let author = SigningKey::from_seed(SigAlg::MlDsa65, &SecretSeed::new([1; 32]))
            .unwrap()
            .public_key()
            .unwrap();
        let aux = MemAux::default();
        let first = Header::new(
            1,
            H256::zero(),
            H256::zero(),
            H256::repeat_byte(1),
            Digest::default(),
        );
        let second = Header::new(
            1,
            H256::zero(),
            H256::zero(),
            H256::repeat_byte(2),
            Digest::default(),
        );
        let slot = Slot::from(7);
        assert_eq!(check(&aux, slot, slot, &first, &author).unwrap(), None);
        // Seeing the same header again is not an equivocation.
        assert_eq!(check(&aux, slot, slot, &first, &author).unwrap(), None);
        let report = check(&aux, slot, slot, &second, &author).unwrap().unwrap();
        assert_eq!(report.first_header, first.encode());
        assert_eq!(report.second_header, second.encode());
        let Some(ac_primitives::offences::Evidence::AuraEquivocation {
            offender,
            first: a,
            second: b,
        }) = evidence(&report, &author)
        else {
            panic!("seal evidence expected");
        };
        assert_eq!(
            (offender, a.into_inner(), b.into_inner()),
            (author, first.encode(), second.encode())
        );
    }
}
