//! The well-known storage keys of constitution layer 1 (`ac-invariants`) are the keys the
//! runtime actually uses, with the encodings the node expects (m3-economics 7.3; red line 3).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use ac_invariants::{GenesisError, check_genesis, keys, read_ledger};
use ac_runtime::Runtime;
use common::{issuance, preset_ext};
use frame_support::storage::storage_prefix;

#[test]
fn keys_match_the_runtime() {
    assert_eq!(
        keys::TOTAL_ISSUANCE.to_vec(),
        pallet_balances::TotalIssuance::<Runtime>::hashed_key().to_vec()
    );
    assert_eq!(
        keys::TOTAL_BURNED.to_vec(),
        pallet_emission::TotalBurned::<Runtime>::hashed_key().to_vec()
    );
    assert_eq!(
        keys::EMISSION_EPOCH_LENGTH.to_vec(),
        pallet_emission::EpochLength::<Runtime>::hashed_key().to_vec()
    );
    assert_eq!(
        keys::SYSTEM_ACCOUNT_PREFIX.to_vec(),
        storage_prefix(b"System", b"Account").to_vec()
    );
    assert_eq!(
        keys::POA_COUNCIL_MEMBERS.to_vec(),
        pallet_collective::Members::<Runtime, pallet_collective::Instance1>::hashed_key().to_vec()
    );
}

// The node reads the development genesis through these keys: a valid emission epoch and the
// endowed issuance; as a live chain it would be refused for its premine.
#[test]
fn genesis_reads_through_the_keys() {
    let mut ext = preset_ext(sp_genesis_builder::DEV_RUNTIME_PRESET);
    let entries: Vec<(Vec<u8>, Vec<u8>)> = ext.execute_with(|| {
        let mut out = Vec::new();
        let mut key = Vec::new();
        while let Some(next) = sp_io::storage::next_key(&key) {
            out.push((next.clone(), sp_io::storage::get(&next).unwrap().to_vec()));
            key = next;
        }
        out
    });
    let pairs = || entries.iter().map(|(k, v)| (k.as_slice(), v.as_slice()));
    let params = check_genesis(pairs(), false).unwrap();
    assert_eq!(params.schedule.epoch_length(), 10);
    let genesis_issuance = ext.execute_with(issuance);
    assert_eq!(params.genesis_issuance, genesis_issuance);
    assert_eq!(
        check_genesis(pairs(), true).unwrap_err(),
        GenesisError::NonZeroIssuance(genesis_issuance)
    );
    ext.execute_with(|| {
        let ledger = read_ledger(|k| sp_io::storage::get(k).map(|v| v.to_vec())).unwrap();
        assert_eq!((ledger.issuance, ledger.burned), (genesis_issuance, 0));
    });
}
