//! Unit tests (consensus/offences, chain/pq-transaction-auth "无账户来源的授权调用").
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use ac_crypto::sig::SigningKey;
use ac_crypto::{PqPublicKey, SigAlg, dev_seed};
use ac_primitives::ac_bft::{
    BlockRef, Message, SignedMessage, VOTE_CONTEXT, VoteKind, signing_payload,
};
use ac_primitives::aura_pq::{SEAL_CONTEXT, Slot, pre_digest, seal_digest};
use ac_primitives::offences::{ChainHeader, EncodedHeader, Evidence, OffenceKey};
use frame_support::pallet_prelude::{
    InvalidTransaction, TransactionSource, TransactionValidityError,
};
use frame_support::traits::Hooks;
use parity_scale_codec::Encode;
use sp_core::H256;
use sp_runtime::traits::Header as _;
use sp_runtime::{BuildStorage, Digest};

use crate::mock::{
    AuraPq, Balances, Offences, RuntimeEvent, RuntimeGenesisConfig, RuntimeOrigin, System, Test,
    ValidatorSet,
};
use crate::{Event, Offenders, Reports};

const NAMES: [&str; 4] = ["alice", "bob", "charlie", "dave"];

fn signer(name: &str) -> SigningKey {
    SigningKey::from_seed(SigAlg::MlDsa65, &dev_seed(name).unwrap()).unwrap()
}

fn key(name: &str) -> PqPublicKey {
    signer(name).public_key().unwrap()
}

fn ext() -> sp_io::TestExternalities {
    let config = RuntimeGenesisConfig {
        balances: pallet_balances::GenesisConfig {
            balances: vec![(1, 1_000), (2, 2_000)],
            ..Default::default()
        },
        aura_pq: pallet_aura_pq::GenesisConfig {
            authorities: NAMES.iter().map(|n| key(n)).collect(),
            ..Default::default()
        },
        validator_set: pallet_validator_set::GenesisConfig {
            epoch_length: 8,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut ext: sp_io::TestExternalities = config.build_storage().unwrap().into();
    ext.execute_with(|| run_to(1));
    ext
}

fn run_to(n: u64) {
    let mut next = System::block_number() + 1;
    while next <= n {
        let mut digest = Digest::default();
        digest.push(pre_digest(Slot::from(next)));
        System::reset_events();
        System::initialize(&next, &Default::default(), &digest);
        AuraPq::on_initialize(next);
        ValidatorSet::on_initialize(next);
        Offences::on_initialize(next);
        next += 1;
    }
}

fn sealed(name: &str, slot: u64, root: u8) -> EncodedHeader {
    let mut digest = Digest::default();
    digest.push(pre_digest(Slot::from(slot)));
    let mut header = ChainHeader::new(
        3,
        H256::repeat_byte(root),
        H256::zero(),
        H256::zero(),
        digest,
    );
    let sig = signer(name)
        .sign_deterministic(header.hash().as_ref(), SEAL_CONTEXT)
        .unwrap();
    header.digest_mut().push(seal_digest(&sig));
    EncodedHeader::try_from(header.encode()).unwrap()
}

fn aura_evidence(name: &str, slot: u64) -> Evidence {
    Evidence::AuraEquivocation {
        offender: key(name),
        first: sealed(name, slot, 1),
        second: sealed(name, slot, 2),
    }
}

fn vote(name: &str, index: u16, set_id: u64, round: u64, target: u8) -> SignedMessage {
    let genesis = System::block_hash(0);
    let message = Message::Vote {
        kind: VoteKind::Commit,
        round,
        target: BlockRef {
            hash: H256::repeat_byte(target),
            number: 1,
        },
    };
    let payload = signing_payload(&genesis, set_id, &message).unwrap();
    SignedMessage {
        set_id,
        signer: index,
        message,
        signature: signer(name)
            .sign_deterministic(&payload, VOTE_CONTEXT)
            .unwrap(),
    }
}

fn bft_evidence(name: &str, index: u16, set_id: u64, round: u64) -> Evidence {
    Evidence::BftEquivocation {
        first: Box::new(vote(name, index, set_id, round, 1)),
        second: Box::new(vote(name, index, set_id, round, 2)),
    }
}

/// Authorizes like the transaction pool, then dispatches with the resulting origin.
fn submit(evidence: Evidence) -> Result<(), TransactionValidityError> {
    Offences::authorize_report(TransactionSource::External, &evidence)?;
    Offences::report_equivocation(
        RuntimeOrigin::from(frame_system::RawOrigin::Authorized),
        evidence,
    )
    .unwrap();
    Ok(())
}

fn reported() -> Vec<(PqPublicKey, OffenceKey, u64, u128)> {
    System::events()
        .into_iter()
        .filter_map(|e| match e.event {
            RuntimeEvent::Offences(Event::OffenceReported {
                offender,
                key,
                set_id,
                slashed,
            }) => Some((offender, key, set_id, slashed)),
            _ => None,
        })
        .collect()
}

// Scenarios "有效的出块双签证据" and "证据举报无需签名".
#[test]
fn seal_equivocation_is_recorded_without_signature() {
    ext().execute_with(|| {
        run_to(3);
        submit(aura_evidence("bob", 3)).unwrap();
        let events = reported();
        assert_eq!(
            events,
            vec![(
                key("bob"),
                OffenceKey::Aura {
                    offender: key("bob"),
                    slot: 3
                },
                0,
                0
            )]
        );
        assert_eq!(Offences::offences(0).len(), 1);
    });
}

// Scenario "有效的投票双签证据".
#[test]
fn vote_equivocation_is_recorded() {
    ext().execute_with(|| {
        submit(bft_evidence("charlie", 2, 0, 5)).unwrap();
        let events = reported();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].0, key("charlie"));
        assert!(matches!(
            events[0].1,
            OffenceKey::Bft {
                set_id: 0,
                signer: 2,
                round: 5,
                ..
            }
        ));
    });
}

// Scenarios "伪造签名" and "无效证据被拒绝": invalid evidence never authorizes.
#[test]
fn invalid_evidence_is_rejected() {
    ext().execute_with(|| {
        let Evidence::AuraEquivocation { first, .. } = aura_evidence("bob", 3) else {
            unreachable!()
        };
        let forged = Evidence::AuraEquivocation {
            offender: key("bob"),
            first,
            second: sealed("alice", 3, 2),
        };
        assert_eq!(
            Offences::authorize_report(TransactionSource::External, &forged),
            Err(TransactionValidityError::Invalid(
                InvalidTransaction::BadProof
            ))
        );
        let mut wrong_index = bft_evidence("charlie", 2, 0, 5);
        if let Evidence::BftEquivocation { first, second } = &mut wrong_index {
            first.signer = 1;
            second.signer = 1;
        }
        assert!(Offences::authorize_report(TransactionSource::External, &wrong_index).is_err());
        assert!(reported().is_empty());
    });
}

// Scenarios "重复举报" and "同一集合内的其他违规".
#[test]
fn offences_are_recorded_once() {
    ext().execute_with(|| {
        run_to(4);
        submit(aura_evidence("bob", 3)).unwrap();
        let stale = Err(TransactionValidityError::Invalid(InvalidTransaction::Stale));
        assert_eq!(submit(aura_evidence("bob", 3)), stale);
        assert_eq!(submit(aura_evidence("bob", 4)), stale);
        assert_eq!(submit(bft_evidence("bob", 1, 0, 2)), stale);
        assert_eq!(Offences::offences(0).len(), 1);
        // Another offender in the same set is still recorded.
        submit(aura_evidence("dave", 4)).unwrap();
        assert_eq!(Offences::offences(0).len(), 2);
    });
}

// Scenario "过期证据": evidence from a set outside the history window, or seals older than
// the maximum age, is rejected.
#[test]
fn expired_evidence_is_rejected() {
    ext().execute_with(|| {
        // Old seal slot: current slot 30 > 3 + 24.
        run_to(30);
        assert_eq!(
            Offences::authorize_report(TransactionSource::External, &aura_evidence("bob", 3)),
            Err(TransactionValidityError::Invalid(InvalidTransaction::Stale))
        );
        // Set 0 ends when set 1 starts at epoch 4 (block 33). With a 2-epoch window it is still
        // accepted in epoch 5 (blocks 41–48) and expired from epoch 6 (block 49).
        submit(aura_evidence("dave", 30)).unwrap();
        run_to(48);
        assert_eq!(ValidatorSet::authority_set().0, 1);
        assert!(
            Offences::authorize_report(TransactionSource::External, &bft_evidence("bob", 1, 0, 9))
                .is_ok()
        );
        run_to(49);
        assert_eq!(
            Offences::authorize_report(TransactionSource::External, &bft_evidence("bob", 1, 0, 9)),
            Err(TransactionValidityError::Invalid(
                InvalidTransaction::BadProof
            ))
        );
    });
}

// Scenario "下一纪元生效" (runtime part): the offender leaves the set at the next boundary.
#[test]
fn offender_leaves_at_next_boundary() {
    ext().execute_with(|| {
        run_to(3);
        submit(aura_evidence("alice", 3)).unwrap();
        run_to(8);
        assert_eq!(ValidatorSet::authority_set().1.len(), 4);
        run_to(9);
        let (id, set) = ValidatorSet::authority_set();
        assert_eq!(id, 1);
        assert!(set.iter().all(|a| a.key != key("alice")));
        assert_eq!(AuraPq::authorities().len(), 3);
    });
}

// Scenario "违规不影响余额": recording and disabling change no balance or issuance.
#[test]
fn offences_change_no_balance() {
    ext().execute_with(|| {
        let issuance = Balances::total_issuance();
        let balances = [Balances::free_balance(1), Balances::free_balance(2)];
        run_to(3);
        submit(aura_evidence("bob", 3)).unwrap();
        run_to(9);
        assert_eq!(reported_or_recorded(), 1);
        assert_eq!(Balances::total_issuance(), issuance);
        assert_eq!(
            [Balances::free_balance(1), Balances::free_balance(2)],
            balances
        );
    });
}

fn reported_or_recorded() -> usize {
    Offences::offences(0).len()
}

// Records of expired sets are dropped, one set per block.
#[test]
fn records_of_expired_sets_are_pruned() {
    ext().execute_with(|| {
        run_to(3);
        submit(aura_evidence("bob", 3)).unwrap();
        assert_eq!(Offenders::<Test>::get(0).len(), 1);
        assert_eq!(Reports::<Test>::iter().count(), 1);
        // Set 1 from block 9 (epoch 1); set 0 leaves the window at epoch 1 + 2 + 1.
        run_to(40);
        assert!(Offenders::<Test>::get(0).is_empty());
        assert_eq!(Reports::<Test>::iter().count(), 0);
    });
}
