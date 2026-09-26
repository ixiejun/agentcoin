//! Unit tests (chain/randomness).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use ac_crypto::sig::SigningKey;
use ac_crypto::{PqPublicKey, SigAlg, dev_seed, randomness_commit, randomness_secret};
use ac_primitives::aura_pq::{Slot, pre_digest};
use ac_primitives::randomness::{
    INHERENT_IDENTIFIER, InherentSecrets, epoch_randomness, subject_value,
};
use frame_support::pallet_prelude::ProvideInherent;
use frame_support::traits::Hooks;
use sp_core::H256;
use sp_inherents::InherentData;
use sp_runtime::{BuildStorage, Digest};

use crate::mock::{
    AuraPq, RandomnessCr, RuntimeGenesisConfig, RuntimeOrigin, System, Test, ValidatorSet,
};
use crate::{Commits, MissedReveals, Reveals};

const NAMES: [&str; 4] = ["alice", "bob", "charlie", "dave"];
const EPOCH: u64 = 8;

fn key(name: &str) -> PqPublicKey {
    SigningKey::from_seed(SigAlg::MlDsa65, &dev_seed(name).unwrap())
        .unwrap()
        .public_key()
        .unwrap()
}

fn account(name: &str) -> [u8; 32] {
    *ac_crypto::account_id(&key(name)).as_bytes()
}

fn secret(name: &str, epoch: u64) -> [u8; 32] {
    let genesis = System::block_hash(0);
    *randomness_secret(&dev_seed(name).unwrap(), genesis.as_fixed_bytes(), epoch)
        .unwrap()
        .expose()
}

fn ext() -> sp_io::TestExternalities {
    RuntimeGenesisConfig {
        aura_pq: pallet_aura_pq::GenesisConfig {
            authorities: NAMES.iter().map(|n| key(n)).collect(),
            ..Default::default()
        },
        validator_set: pallet_validator_set::GenesisConfig {
            epoch_length: EPOCH,
            ..Default::default()
        },
        ..Default::default()
    }
    .build_storage()
    .unwrap()
    .into()
}

/// Author of slot `n` (slot = block number here).
fn author(n: u64) -> &'static str {
    NAMES[usize::try_from(n % 4).unwrap()]
}

/// Runs block `n`; its author, unless offline, adds the inherent like its node would.
fn block(n: u64, offline: &[&str]) {
    let mut digest = Digest::default();
    digest.push(pre_digest(Slot::from(n)));
    System::reset_events();
    System::initialize(&n, &Default::default(), &digest);
    AuraPq::on_initialize(n);
    ValidatorSet::on_initialize(n);
    RandomnessCr::on_initialize(n);
    let name = author(n);
    if offline.contains(&name) {
        return;
    }
    let epoch = ValidatorSet::current_epoch();
    let secrets = InherentSecrets {
        epoch,
        current: secret(name, epoch),
        previous: epoch.checked_sub(1).map(|e| secret(name, e)),
    };
    let mut data = InherentData::new();
    data.put_data(INHERENT_IDENTIFIER, &secrets).unwrap();
    if let Some(call) = RandomnessCr::create_inherent(&data) {
        assert!(RandomnessCr::is_inherent(&call));
        let crate::Call::note_randomness { commit, reveal } = call;
        RandomnessCr::note_randomness(RuntimeOrigin::none(), commit, reveal).unwrap();
    }
}

fn run(from: u64, to: u64, offline: &[&str]) {
    for n in from..=to {
        block(n, offline);
    }
}

// Scenario "每个验证人提交承诺".
#[test]
fn every_validator_commits() {
    ext().execute_with(|| {
        run(1, EPOCH, &[]);
        for name in NAMES {
            assert_eq!(
                Commits::<Test>::get(0, account(name)),
                Some(randomness_commit(&secret(name, 0)).unwrap())
            );
        }
    });
}

// Scenario "同一纪元重复承诺": the first commitment stays.
#[test]
fn second_commitment_is_ignored() {
    ext().execute_with(|| {
        run(1, 5, &[]);
        let before = Commits::<Test>::get(0, account(author(6)));
        assert!(before.is_some());
        block(6, &[author(6)]);
        RandomnessCr::note_randomness(RuntimeOrigin::none(), Some([9; 32]), None).unwrap();
        assert_eq!(Commits::<Test>::get(0, account(author(6))), before);
    });
}

// Scenarios "揭示与承诺一致" and "揭示值不符".
#[test]
fn reveals_must_match_commitments() {
    ext().execute_with(|| {
        run(1, EPOCH, &[]);
        // Block 9 (epoch 1) by bob: a wrong reveal is ignored…
        block(9, &["bob"]);
        RandomnessCr::note_randomness(RuntimeOrigin::none(), None, Some([7; 32])).unwrap();
        assert_eq!(Reveals::<Test>::get(0, account("bob")), None);
        // …and the right one, in bob's next block, is recorded.
        run(10, 13, &[]);
        let revealed = Reveals::<Test>::get(0, account("bob")).unwrap();
        assert_eq!(revealed, secret("bob", 0));
        assert_eq!(
            randomness_commit(&revealed).unwrap(),
            Commits::<Test>::get(0, account("bob")).unwrap()
        );
        // Nobody can reveal twice.
        block(14, &["charlie"]);
        RandomnessCr::note_randomness(RuntimeOrigin::none(), None, Some(secret("charlie", 0)))
            .unwrap();
        run(15, 16, &[]);
        assert_eq!(RandomnessCr::reveals(0).len(), 4);
    });
}

// Scenarios "独立复算", "各纪元不同" and "按主题派生".
#[test]
fn randomness_is_published_and_recomputable() {
    ext().execute_with(|| {
        assert_eq!(RandomnessCr::latest(), None, "Scenario 创世初期");
        assert_eq!(RandomnessCr::random(b"audit"), None);
        assert_eq!(
            <RandomnessCr as frame_support::traits::Randomness<_, _>>::random(b"x").0,
            H256::zero()
        );
        let mut values = Vec::new();
        for epoch in 0..3u64 {
            let end = (epoch + 2) * EPOCH;
            run(System::block_number() + 1, end, &[]);
            let reveals = RandomnessCr::reveals(epoch);
            assert_eq!(reveals.len(), 4);
            block(end + 1, &[]);
            let expected = epoch_randomness(epoch, &reveals).unwrap().unwrap();
            assert_eq!(RandomnessCr::epoch_randomness(epoch), Some(expected));
            assert_eq!(RandomnessCr::latest(), Some((epoch, expected)));
            assert!(
                RandomnessCr::reveals(epoch).is_empty(),
                "cleared after publishing"
            );
            values.push(expected);
        }
        assert!(values[0] != values[1] && values[1] != values[2] && values[0] != values[2]);
        let (epoch, audit) = RandomnessCr::random(b"audit").unwrap();
        assert_eq!(epoch, 2);
        assert_eq!(audit, subject_value(&values[2], b"audit").unwrap());
        assert_ne!(audit, RandomnessCr::random(b"sample").unwrap().1);
        let (frame_value, known_at) =
            <RandomnessCr as frame_support::traits::Randomness<_, _>>::random(b"audit");
        assert_eq!(frame_value, audit);
        assert_eq!(known_at, 4 * EPOCH + 1);
    });
}

// Scenario "离线导致未揭示": dave commits in epoch 0, is offline in epoch 1; R(0) uses the other
// reveals and dave's missed reveals count 1.
#[test]
fn offline_validator_misses_its_reveal() {
    ext().execute_with(|| {
        run(1, EPOCH, &[]);
        run(EPOCH + 1, 2 * EPOCH, &["dave"]);
        let reveals = RandomnessCr::reveals(0);
        assert_eq!(reveals.len(), 3);
        block(2 * EPOCH + 1, &[]);
        assert_eq!(
            RandomnessCr::epoch_randomness(0),
            epoch_randomness(0, &reveals).unwrap()
        );
        assert_eq!(MissedReveals::<Test>::get(account("dave")), 1);
        assert_eq!(MissedReveals::<Test>::get(account("alice")), 0);
    });
}

// An epoch without any reveal has no randomness.
#[test]
fn no_reveals_no_randomness() {
    ext().execute_with(|| {
        run(1, EPOCH, &[]);
        run(EPOCH + 1, 2 * EPOCH + 1, &NAMES);
        assert_eq!(RandomnessCr::epoch_randomness(0), None);
        assert_eq!(RandomnessCr::latest(), None);
        for name in NAMES {
            assert_eq!(MissedReveals::<Test>::get(account(name)), 1);
        }
    });
}

// Only one note per block; data for another epoch is not used.
#[test]
fn one_note_per_block_and_matching_epoch() {
    ext().execute_with(|| {
        block(1, &["bob"]);
        RandomnessCr::note_randomness(RuntimeOrigin::none(), Some([1; 32]), None).unwrap();
        RandomnessCr::note_randomness(RuntimeOrigin::none(), Some([2; 32]), None).unwrap();
        assert_eq!(Commits::<Test>::get(0, account("bob")), Some([1; 32]));
        let mut data = InherentData::new();
        data.put_data(
            INHERENT_IDENTIFIER,
            &InherentSecrets {
                epoch: 5,
                current: [3; 32],
                previous: None,
            },
        )
        .unwrap();
        assert!(RandomnessCr::create_inherent(&data).is_none());
    });
}

// Scenario "文档说明偏置": both READMEs explain the last-revealer bias and the allowed uses.
#[test]
fn readmes_state_the_bias() {
    let en = include_str!("../README.md");
    let zh = include_str!("../README.zh-CN.md");
    assert!(en.contains("## Known bias") && en.contains("last validator to reveal"));
    assert!(en.contains("only for low-value"));
    assert!(zh.contains("## 已知偏置") && zh.contains("最后一个揭示的验证人"));
    assert!(zh.contains("只适用于审计抽样一类低价值用途"));
}
