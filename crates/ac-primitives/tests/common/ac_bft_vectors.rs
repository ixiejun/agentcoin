//! Builds the AC-BFT regression vectors from the development keys. Shared by
//! `examples/gen_ac_bft_vectors.rs` (which prints them) and `tests/ac_bft_vectors.rs` (which
//! compares them with the committed file), so both use exactly the same construction.

use ac_crypto::SigAlg;
use ac_crypto::dev_seed;
use ac_crypto::sig::SigningKey;
use ac_primitives::ac_bft::{
    Authority, BlockRef, ConsensusLog, FinalityProof, Message, ScheduledChange, SignedMessage,
    VOTE_CONTEXT, VersionedFinalityProof, VersionedMessage, VoteKind, signing_payload,
};
use ac_primitives::offences::Evidence;
use parity_scale_codec::Encode;
use sp_core::H256;
use sp_runtime::BoundedVec;

/// `(name, lowercase hex)` pairs, in file order.
pub fn vectors() -> Vec<(&'static str, String)> {
    let keys: Vec<SigningKey> = ["alice", "bob", "charlie", "dave"]
        .iter()
        .map(|n| SigningKey::from_seed(SigAlg::MlDsa65, &dev_seed(n).unwrap()).unwrap())
        .collect();
    let genesis = H256::repeat_byte(0x11);
    let target = BlockRef {
        hash: H256::repeat_byte(0x22),
        number: 5,
    };
    let sign = |index: usize, message: Message| {
        let payload = signing_payload(&genesis, 0, &message).unwrap();
        SignedMessage {
            set_id: 0,
            signer: u16::try_from(index).unwrap(),
            message,
            signature: keys[index]
                .sign_deterministic(&payload, VOTE_CONTEXT)
                .unwrap(),
        }
    };
    let prepare = Message::Vote {
        kind: VoteKind::Prepare,
        round: 3,
        target,
    };
    let commit = Message::Vote {
        kind: VoteKind::Commit,
        round: 3,
        target,
    };
    let commit_payload = signing_payload(&genesis, 0, &commit).unwrap();
    let proof = VersionedFinalityProof::V1(FinalityProof {
        set_id: 0,
        round: 3,
        target,
        commits: BoundedVec::try_from(
            (0..3usize)
                .map(|i| {
                    let sig = keys[i]
                        .sign_deterministic(&commit_payload, VOTE_CONTEXT)
                        .unwrap();
                    (u16::try_from(i).unwrap(), sig)
                })
                .collect::<Vec<_>>(),
        )
        .unwrap(),
    });
    let change = ConsensusLog::ScheduledChange(ScheduledChange {
        set_id: 1,
        authorities: BoundedVec::try_from(
            keys[1..]
                .iter()
                .map(|k| Authority::poa(k.public_key().unwrap()))
                .collect::<Vec<_>>(),
        )
        .unwrap(),
    });
    let other = Message::Vote {
        kind: VoteKind::Prepare,
        round: 3,
        target: BlockRef {
            hash: H256::repeat_byte(0x33),
            number: 5,
        },
    };
    let evidence = Evidence::BftEquivocation {
        first: Box::new(sign(3, prepare.clone())),
        second: Box::new(sign(3, other)),
    };
    let hex = |bytes: &[u8]| bytes.iter().map(|b| format!("{b:02x}")).collect::<String>();
    vec![
        (
            "prepare_payload",
            hex(&signing_payload(&genesis, 0, &prepare).unwrap()),
        ),
        (
            "signed_prepare",
            hex(&VersionedMessage::V1(sign(0, prepare)).encode()),
        ),
        (
            "proposal_payload",
            hex(&signing_payload(
                &genesis,
                0,
                &Message::Proposal {
                    round: 4,
                    target,
                    justify: Some(ac_primitives::ac_bft::CertRef { round: 3, target }),
                },
            )
            .unwrap()),
        ),
        (
            "timeout_payload",
            hex(&signing_payload(
                &genesis,
                0,
                &Message::Timeout {
                    round: 4,
                    high: None,
                },
            )
            .unwrap()),
        ),
        ("finality_proof", hex(&proof.encode())),
        ("scheduled_change_digest", hex(&change.to_digest().encode())),
        ("bft_equivocation_evidence", hex(&evidence.encode())),
    ]
}

/// The vectors as the committed JSON document.
pub fn json() -> String {
    let body = vectors()
        .iter()
        .map(|(name, hex)| format!("  \"{name}\": \"{hex}\""))
        .collect::<Vec<_>>()
        .join(",\n");
    format!("{{\n  \"genesis\": \"{}\",\n{body}\n}}\n", "11".repeat(32))
}
