//! Runtime integration of the M2 consensus pallets on the `local` preset (four authorities,
//! 20-block epochs): epoch-boundary set changes, reports leading to disabling, and a full
//! randomness cycle, with total issuance unchanged throughout (m2-finality 6.3).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

mod common;

use ac_crypto::SigAlg;
use ac_primitives::ac_bft::{Authority, ConsensusLog};
use ac_primitives::aura_pq::{Slot, pre_digest};
use ac_primitives::randomness::{INHERENT_IDENTIFIER, InherentSecrets, epoch_randomness};
use ac_runtime::genesis_config_presets::dev_public_key;
use ac_runtime::{
    AuraPq, Executive, Header, RandomnessCr, Runtime, RuntimeCall, System, UncheckedExtrinsic,
    ValidatorSet,
};
use common::{apply, issuance, preset_ext, report, seal_evidence, sum_of_balances};
use frame_support::pallet_prelude::ProvideInherent;
use sp_core::H256;
use sp_inherents::InherentData;
use sp_runtime::Digest;
use sp_runtime::traits::Header as _;

const NAMES: [&str; 4] = ["alice", "bob", "charlie", "dave"];
const EPOCH: u32 = 20;

/// Produces block `number` in slot `number` on top of `parent`: its author (unless offline)
/// adds the randomness inherent as its node would. Returns the finalized header.
fn produce(number: u32, parent: H256, offline: &[&str]) -> Header {
    let mut digest = Digest::default();
    digest.push(pre_digest(Slot::from(u64::from(number))));
    Executive::initialize_block(&Header::new(
        number,
        H256::zero(),
        H256::zero(),
        parent,
        digest,
    ));
    pallet_timestamp::Pallet::<Runtime>::set_timestamp(
        u64::from(number) * ac_runtime::MILLISECS_PER_BLOCK,
    );
    let author = AuraPq::current_author_name();
    if let Some(name) = author
        && !offline.contains(&name)
    {
        let epoch = ValidatorSet::current_epoch();
        let genesis = System::block_hash(0);
        let seed = ac_crypto::dev_seed(name).unwrap();
        let secret = |e: u64| {
            *ac_crypto::randomness_secret(&seed, genesis.as_fixed_bytes(), e)
                .unwrap()
                .expose()
        };
        let mut data = InherentData::new();
        data.put_data(
            INHERENT_IDENTIFIER,
            &InherentSecrets {
                epoch,
                current: secret(epoch),
                previous: epoch.checked_sub(1).map(secret),
            },
        )
        .unwrap();
        if let Some(call) = RandomnessCr::create_inherent(&data) {
            let xt = UncheckedExtrinsic::new_bare(RuntimeCall::RandomnessCr(call));
            assert_eq!(apply(xt), Ok(Ok(())));
        }
    }
    Executive::finalize_block()
}

/// Maps the recorded block author back to a development name.
trait AuthorName {
    fn current_author_name() -> Option<&'static str>;
}

impl AuthorName for AuraPq {
    fn current_author_name() -> Option<&'static str> {
        let author = pallet_aura_pq::CurrentAuthor::<Runtime>::get()?;
        NAMES
            .into_iter()
            .find(|n| dev_public_key(n, SigAlg::MlDsa65).unwrap() == author)
    }
}

struct Chain {
    parent: H256,
    number: u32,
}

impl Chain {
    fn new() -> Self {
        Self {
            parent: System::block_hash(0),
            number: 0,
        }
    }

    /// Produces blocks up to `to`, returning the last header.
    fn run_to(&mut self, to: u32, offline: &[&str]) -> Header {
        let mut last = None;
        while self.number < to {
            self.number += 1;
            let header = produce(self.number, self.parent, offline);
            self.parent = header.hash();
            last = Some(header);
        }
        last.unwrap()
    }
}

// consensus/validator-set Scenario "变更摘要", consensus/offences Scenario "下一纪元生效" and
// consensus/aura-pq Scenario "纪元边界后按新列表出块" (runtime parts), with conservation.
#[test]
fn report_disables_at_next_boundary() {
    preset_ext(sp_genesis_builder::LOCAL_TESTNET_RUNTIME_PRESET).execute_with(|| {
        let supply = issuance();
        let mut chain = Chain::new();
        chain.run_to(5, &[]);
        // Evidence is applied inside block 6.
        chain.number += 1;
        let mut digest = Digest::default();
        digest.push(pre_digest(Slot::from(6)));
        Executive::initialize_block(&Header::new(
            6,
            H256::zero(),
            H256::zero(),
            chain.parent,
            digest,
        ));
        pallet_timestamp::Pallet::<Runtime>::set_timestamp(6_000);
        assert_eq!(apply(report(seal_evidence("bob", 3, false))), Ok(Ok(())));
        chain.parent = Executive::finalize_block().hash();

        let last_of_epoch = chain.run_to(EPOCH, &[]);
        assert_eq!(
            ConsensusLog::find_change(last_of_epoch.digest().logs()),
            None
        );
        assert_eq!(ValidatorSet::authority_set().1.len(), 4);

        let boundary = chain.run_to(EPOCH + 1, &[]);
        let change = ConsensusLog::find_change(boundary.digest().logs()).expect("change digest");
        let remaining: Vec<Authority> = ["alice", "charlie", "dave"]
            .iter()
            .map(|n| Authority::poa(dev_public_key(n, SigAlg::MlDsa65).unwrap()))
            .collect();
        assert_eq!(change.set_id, 1);
        assert_eq!(change.authorities.to_vec(), remaining);
        assert_eq!(ValidatorSet::authority_set(), (1, remaining));
        assert_eq!(AuraPq::authorities().len(), 3);

        // Block 22, slot 22: new list [alice, charlie, dave] gives charlie (22 mod 3 = 1).
        chain.run_to(EPOCH + 2, &[]);
        assert_eq!(AuraPq::current_author_name(), Some("charlie"));
        assert_eq!(issuance(), supply);
        assert_eq!(sum_of_balances(), supply);
    });
}

// chain/randomness Scenarios "独立复算" and "各纪元不同" through the executive, with
// conservation.
#[test]
fn randomness_full_cycle() {
    preset_ext(sp_genesis_builder::LOCAL_TESTNET_RUNTIME_PRESET).execute_with(|| {
        let supply = issuance();
        let mut chain = Chain::new();
        let mut values = Vec::new();
        for epoch in 0..2u64 {
            let end = (u32::try_from(epoch).unwrap() + 2) * EPOCH;
            chain.run_to(end, &[]);
            let reveals = RandomnessCr::reveals(epoch);
            assert_eq!(reveals.len(), 4, "every authority revealed");
            chain.run_to(end + 1, &[]);
            let expected = epoch_randomness(epoch, &reveals).unwrap().unwrap();
            assert_eq!(RandomnessCr::epoch_randomness(epoch), Some(expected));
            values.push(expected);
        }
        assert_ne!(values[0], values[1]);
        assert_eq!(issuance(), supply);
        assert_eq!(sum_of_balances(), supply);
    });
}
