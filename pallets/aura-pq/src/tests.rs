//! Unit tests (consensus/aura-pq).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable,
    clippy::indexing_slicing
)]

use ac_crypto::sig::{SecretSeed, SigningKey};
use ac_crypto::{PqPublicKey, SigAlg};
use ac_primitives::aura_pq::pre_digest;
use frame_support::traits::{Hooks, OnTimestampSet};
use sp_runtime::BuildStorage;

use crate::mock::{AuraPq, RuntimeGenesisConfig, System, Test};
use crate::{AuthorityError, AuthoritySetWriter, CurrentSlot, Slot};

fn key(alg: SigAlg, seed: u8) -> PqPublicKey {
    SigningKey::from_seed(alg, &SecretSeed::new([seed; 32]))
        .unwrap()
        .public_key()
        .unwrap()
}

fn ext_with(authorities: Vec<PqPublicKey>) -> sp_io::TestExternalities {
    let config = RuntimeGenesisConfig {
        aura_pq: crate::GenesisConfig {
            authorities,
            ..Default::default()
        },
        ..Default::default()
    };
    config.build_storage().unwrap().into()
}

// Requirement "授权节点集合来自创世" / Scenario "查询授权节点".
#[test]
fn genesis_authorities_are_queryable_in_order() {
    let set = vec![
        key(SigAlg::MlDsa65, 1),
        key(SigAlg::MlDsa65, 2),
        key(SigAlg::MlDsa65, 3),
    ];
    ext_with(set.clone()).execute_with(|| {
        assert_eq!(AuraPq::authorities(), set);
        assert_eq!(AuraPq::slot_duration(), 1000);
    });
}

// Requirement "ML-DSA-65 出块封印" / Scenario "非 ML-DSA-65 授权密钥".
#[test]
#[should_panic(expected = "authority keys must be ML-DSA-65")]
fn genesis_rejects_non_ml_dsa_65() {
    let _ = ext_with(vec![key(SigAlg::MlDsa44, 1)]);
}

// Requirement "授权节点集合来自创世" / Scenario "重复授权节点".
#[test]
#[should_panic(expected = "duplicate")]
fn genesis_rejects_duplicates() {
    let _ = ext_with(vec![key(SigAlg::MlDsa65, 1), key(SigAlg::MlDsa65, 1)]);
}

// The empty default genesis (SDK tooling) installs no authorities; the node refuses to run
// such a chain (node/chain-spec).
#[test]
fn empty_default_genesis_installs_nothing() {
    ext_with(vec![]).execute_with(|| assert!(AuraPq::authorities().is_empty()));
}

#[test]
fn writer_validates_before_installing() {
    let set = vec![key(SigAlg::MlDsa65, 1)];
    ext_with(set.clone()).execute_with(|| {
        let too_many: Vec<_> = (1..=4).map(|i| key(SigAlg::MlDsa65, i)).collect();
        assert_eq!(
            AuraPq::write_authorities(too_many),
            Err(AuthorityError::TooMany)
        );
        assert_eq!(
            AuraPq::write_authorities(vec![key(SigAlg::MlDsa87, 9)]),
            Err(AuthorityError::WrongAlgorithm)
        );
        assert_eq!(AuraPq::authorities(), set);
        let next = vec![key(SigAlg::MlDsa65, 7), key(SigAlg::MlDsa65, 8)];
        assert_eq!(AuraPq::write_authorities(next.clone()), Ok(()));
        assert_eq!(AuraPq::authorities(), next);
    });
}

// The slot of the pre-runtime digest is recorded; a non-increasing slot is logged, not a panic.
#[test]
fn records_slot_from_digest_without_panicking() {
    ext_with(vec![key(SigAlg::MlDsa65, 1)]).execute_with(|| {
        System::set_block_number(1);
        System::deposit_log(pre_digest(Slot::from(10)));
        AuraPq::on_initialize(1);
        assert_eq!(CurrentSlot::<Test>::get(), Slot::from(10));
        AuraPq::on_timestamp_set(10_000);

        System::reset_events();
        let mut digest = sp_runtime::Digest::default();
        digest.push(pre_digest(Slot::from(9)));
        System::initialize(&2, &Default::default(), &digest);
        AuraPq::on_initialize(2);
        assert_eq!(CurrentSlot::<Test>::get(), Slot::from(9));
        // A mismatching timestamp is only logged.
        AuraPq::on_timestamp_set(123_456);
    });
}

// A block without an Aura-PQ digest leaves the slot unchanged.
#[test]
fn missing_digest_is_ignored() {
    ext_with(vec![key(SigAlg::MlDsa65, 1)]).execute_with(|| {
        AuraPq::on_initialize(1);
        assert_eq!(CurrentSlot::<Test>::get(), Slot::from(0));
    });
}
