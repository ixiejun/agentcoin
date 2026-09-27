//! The well-known storage keys of constitution layer 1 (`ac-invariants`) are the keys the
//! runtime actually uses, with the encodings the node expects (m3-economics 7.3; red line 3).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use ac_invariants::{
    GenesisError, StakeSnapshot, TransitionGenesis, check_genesis, check_transition, keys,
    read_ledger, read_switch_state, stake_snapshot,
};
use ac_primitives::staking::ChainPhase;
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
    // m3-pos (design D2).
    assert_eq!(
        keys::PHASE.to_vec(),
        pallet_validator_set::Phase::<Runtime>::hashed_key().to_vec()
    );
    assert_eq!(
        keys::QUALIFIED_SINCE.to_vec(),
        pallet_validator_set::QualifiedSince::<Runtime>::hashed_key().to_vec()
    );
    assert_eq!(
        keys::POA_AUTHORITIES.to_vec(),
        pallet_validator_set::PoaAuthorities::<Runtime>::hashed_key().to_vec()
    );
    assert_eq!(
        keys::TRANSITION_PARAMS.to_vec(),
        pallet_validator_set::TransitionParams::<Runtime>::hashed_key().to_vec()
    );
    assert_eq!(
        keys::VALIDATOR_EPOCH_LENGTH.to_vec(),
        pallet_validator_set::EpochLength::<Runtime>::hashed_key().to_vec()
    );
    assert_eq!(
        keys::STAKING_LEDGER_PREFIX.to_vec(),
        storage_prefix(b"StakingPos", b"Ledger").to_vec()
    );
    assert_eq!(
        keys::STAKING_CANDIDATES_PREFIX.to_vec(),
        storage_prefix(b"StakingPos", b"Candidates").to_vec()
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
    assert_eq!(
        params.transition.params,
        ac_runtime::genesis_config_presets::DEV_TRANSITION
    );
    assert_eq!(params.transition.epoch_length, 10);
    let genesis_issuance = ext.execute_with(issuance);
    assert_eq!(params.genesis_issuance, genesis_issuance);
    assert_eq!(
        check_genesis(pairs(), true).unwrap_err(),
        GenesisError::NonZeroIssuance(genesis_issuance)
    );
    ext.execute_with(|| {
        let ledger = read_ledger(|k| sp_io::storage::get(k).map(|v| v.to_vec())).unwrap();
        assert_eq!((ledger.issuance, ledger.burned), (genesis_issuance, 0));
        let state = read_switch_state(|k| sp_io::storage::get(k).map(|v| v.to_vec())).unwrap();
        assert_eq!(state.phase, ChainPhase::Poa);
        assert_eq!(state.qualified_since, None);
        assert_eq!(state.poa_authorities, 1);
    });
}

/// The `(account, value)` entries of an `Identity`-hashed map, read like the node reads them.
fn map_entries(prefix: &[u8]) -> Vec<(Vec<u8>, Vec<u8>)> {
    let mut out = Vec::new();
    let mut key = prefix.to_vec();
    while let Some(next) = sp_io::storage::next_key(&key) {
        let Some(suffix) = next.strip_prefix(prefix) else {
            break;
        };
        out.push((
            suffix.to_vec(),
            sp_io::storage::get(&next).unwrap().to_vec(),
        ));
        key = next;
    }
    out
}

fn get(key: &[u8]) -> Option<Vec<u8>> {
    sp_io::storage::get(key).map(|v| v.to_vec())
}

/// The stake snapshot the node computes from the current state.
fn node_snapshot() -> StakeSnapshot {
    let ledgers = map_entries(&keys::STAKING_LEDGER_PREFIX);
    let candidates = map_entries(&keys::STAKING_CANDIDATES_PREFIX);
    stake_snapshot(
        issuance(),
        ledgers.iter().map(|(k, v)| (k.as_slice(), v.as_slice())),
        candidates.iter().map(|(k, v)| (k.as_slice(), v.as_slice())),
    )
    .unwrap()
}

// m3-pos 7.1 / 8.1: the node's reading of the stake ledger matches the runtime's, and every
// block of an honest switch on the development chain passes the node's transition check.
#[test]
fn honest_switch_passes_the_node_check() {
    use ac_primitives::validator_set::StakingInterface;
    use ac_runtime::StakingPos;

    let mut ext = preset_ext(sp_genesis_builder::DEV_RUNTIME_PRESET);
    ext.execute_with(|| common::start_authored_block(1, common::GENESIS));
    ext.execute_with(|| {
        let alice = common::Signer::dev("alice");
        let authority = ac_crypto::sig::SigningKey::from_seed(
            ac_crypto::SigAlg::MlDsa65,
            &ac_crypto::dev_seed("alice").unwrap(),
        )
        .unwrap();
        let key = authority.public_key().unwrap();
        let genesis = frame_system::Pallet::<Runtime>::block_hash(0);
        let proof = authority
            .sign_deterministic(
                &ac_primitives::staking::pop_statement(&genesis, &alice.account, &key),
                ac_primitives::staking::VALIDATOR_POP_CONTEXT,
            )
            .unwrap();
        let call =
            ac_runtime::RuntimeCall::StakingPos(pallet_staking_pos::Call::register_candidate {
                key,
                proof,
                value: 600_000 * ac_runtime::ATC,
                commission_bps: 1_000,
            });
        assert_eq!(common::apply(common::signed(&alice, call)), Ok(Ok(())));
        let snapshot = node_snapshot();
        assert_eq!(
            snapshot.total_active,
            <StakingPos as StakingInterface>::total_active()
        );
        assert_eq!(
            snapshot.qualified_candidates,
            <StakingPos as StakingInterface>::qualified_candidates()
        );

        let transition = TransitionGenesis {
            params: ac_runtime::genesis_config_presets::DEV_TRANSITION,
            epoch_length: 10,
        };
        let mut switched = None;
        for _ in 0..60 {
            let pre = read_switch_state(get).unwrap();
            let pre_stake = node_snapshot();
            common::next_authored_block();
            let number = u64::from(frame_system::Pallet::<Runtime>::block_number());
            let post = read_switch_state(get).unwrap();
            check_transition(&transition, number, &pre, &post, || Ok(pre_stake)).unwrap();
            if switched.is_none() && post.phase == ChainPhase::Pos {
                switched = Some(number);
            }
        }
        // Qualified from the first checkpoint at height ≥ 20 (block 21), held for 20 blocks.
        assert_eq!(switched, Some(41));
    });
}
