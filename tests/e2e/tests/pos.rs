//! Acceptance of m3-pos on the four-authority local testnet (tasks 10.1–10.3): three candidates
//! stake more than 10% of the issuance, the chain switches to PoS within minutes, elects them,
//! pays the security budget to validators and nominators, and slashes a validator that double
//! signs. Enabled with `AC_E2E=1`.

// Test code: failures should abort the test.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    missing_docs
)]

use std::time::{Duration, Instant};

use ac_crypto::sig::SigningKey;
use ac_crypto::{PqPublicKey, SigAlg};
use ac_e2e::{TestNode, Testnet, enabled};
use ac_primitives::ac_bft::{Authority, SetId};
use ac_primitives::staking::{
    AccountStake, ChainPhase, TransitionProgress, VALIDATOR_POP_CONTEXT, pop_statement,
};
use ac_runtime::genesis_config_presets::{dev_account, dev_public_key};
use ac_runtime::transaction::{TxParams, assemble, authorized_extensions, implicit_from, payload};
use ac_runtime::{ATC, RuntimeCall, RuntimeEvent};
use ac_wallet::NodeClient;
use jsonrpsee::core::client::ClientT;
use jsonrpsee::rpc_params;
use parity_scale_codec::{Decode, Encode};
use sp_runtime::generic::Era;

const START: Duration = Duration::from_secs(120);
/// `local` epoch length (validator and emission epochs).
const EPOCH: u64 = 20;
/// Alice's self-stake.
const SELF_STAKE: u128 = 150_000 * ATC;
/// Bob's and charlie's self-stake: large enough that the two of them hold more than two thirds
/// of the PoS voting weight once alice is removed for double signing.
const OTHER_STAKE: u128 = 250_000 * ATC;
/// Dave's nomination of alice. All stake: 700,000 ATC, 14% of the local issuance (5,000,000).
const NOMINATION: u128 = 50_000 * ATC;

macro_rules! require_e2e {
    () => {
        if !enabled() {
            eprintln!("end-to-end test disabled; run with AC_E2E=1 after `cargo build -p ac-node`");
            return;
        }
    };
}

async fn api<T: Decode>(node: &TestNode, method: &str, args: &[u8], at: Option<&str>) -> T {
    T::decode(&mut &node.state_call(method, args, at).await.unwrap()[..]).unwrap()
}

/// Events of block `hash`, read from `System::Events`.
async fn events(node: &TestNode, hash: &str) -> Vec<RuntimeEvent> {
    let key = frame_support::storage::storage_prefix(b"System", b"Events");
    let key = format!(
        "0x{}",
        key.iter().map(|b| format!("{b:02x}")).collect::<String>()
    );
    let raw: Option<String> = node
        .rpc()
        .unwrap()
        .request("state_getStorage", rpc_params![key, hash])
        .await
        .unwrap();
    let Some(raw) = raw else { return Vec::new() };
    let bytes = ac_e2e::unhex(&raw).unwrap();
    Vec::<frame_system::EventRecord<RuntimeEvent, sp_core::H256>>::decode(&mut &bytes[..])
        .unwrap()
        .into_iter()
        .map(|r| r.event)
        .collect()
}

/// Signs `call` with the development account `name` and submits it through `client`.
async fn submit_as(client: &NodeClient, name: &str, call: RuntimeCall) -> bool {
    let key = SigningKey::from_seed(SigAlg::MlDsa44, &ac_crypto::dev_seed(name).unwrap()).unwrap();
    let who = pallet_pq_accounts::derived_account(&key.public_key().unwrap());
    let context = client.chain_context().await.unwrap();
    let params = TxParams {
        nonce: client.nonce(&who).await.unwrap(),
        tip: 0,
        era: Era::Immortal,
        era_birth_hash: context.genesis_hash,
    };
    let extensions = authorized_extensions(&params);
    let digest = payload(&call, &extensions, &implicit_from(&context, &params)).unwrap();
    let mut rng = ac_crypto::OsRng::new().unwrap();
    let signature = key
        .sign(&digest, pallet_pq_accounts::TX_SIGNING_CONTEXT, &mut rng)
        .unwrap();
    let first = client.current_key(&who).await.unwrap().is_none();
    let xt = assemble(
        call,
        who,
        signature,
        first.then(|| key.public_key().unwrap()),
        extensions,
    );
    client
        .submit_and_watch(&xt, Duration::from_secs(60))
        .await
        .unwrap()
        .success
}

/// `name` registers its authority key as a candidate with `value`.
async fn register(client: &NodeClient, name: &str, value: u128) {
    let context = client.chain_context().await.unwrap();
    let account = dev_account(name).unwrap();
    let authority =
        SigningKey::from_seed(SigAlg::MlDsa65, &ac_crypto::dev_seed(name).unwrap()).unwrap();
    let key = authority.public_key().unwrap();
    let proof = authority
        .sign_deterministic(
            &pop_statement(&context.genesis_hash, &account, &key),
            VALIDATOR_POP_CONTEXT,
        )
        .unwrap();
    let call = RuntimeCall::StakingPos(pallet_staking_pos::Call::register_candidate {
        key,
        proof,
        value,
        commission_bps: 1_000,
    });
    assert!(
        submit_as(client, name, call).await,
        "{name} could not register"
    );
}

/// Stakes alice, bob and charlie, has dave nominate alice, and waits for the switch to PoS.
/// Returns the block of the switch.
async fn stake_and_switch(net: &Testnet) -> u64 {
    let client = NodeClient::new(&net.nodes[0].url).unwrap();
    register(&client, "alice", SELF_STAKE).await;
    register(&client, "bob", OTHER_STAKE).await;
    register(&client, "charlie", OTHER_STAKE).await;
    let nominate = RuntimeCall::StakingPos(pallet_staking_pos::Call::nominate {
        value: NOMINATION,
        targets: vec![dev_account("alice").unwrap()],
    });
    assert!(
        submit_as(&client, "dave", nominate).await,
        "dave could not nominate"
    );
    let node = &net.nodes[1];
    let started = Instant::now();
    loop {
        let progress: TransitionProgress = api(node, "StakingApi_transition", &[], None).await;
        if let Some(block) = progress.switched_at {
            assert_eq!(progress.phase, ChainPhase::Pos);
            return block;
        }
        assert!(
            started.elapsed() < Duration::from_secs(240),
            "no switch within four minutes ({progress:?}); logs in {}",
            net.base.display()
        );
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

async fn set_at(node: &TestNode, at: &str) -> (SetId, Vec<Authority>) {
    api(node, "ValidatorSetApi_authority_set", &[], Some(at)).await
}

// node/chain-spec Scenario "本地链几分钟内完成切换", consensus/pos-transition "连续保持 7 天后切换"
// (with local values), consensus/npos-election "选出 K 个验证人" and economics/validator-rewards
// "自动发放": the three candidates form the new set, the chain keeps finalizing, and the first
// PoS settlements pay the security budget to validators and to dave, who nominated alice.
#[tokio::test(flavor = "multi_thread")]
async fn switch_elect_and_reward() {
    require_e2e!();
    let net = Testnet::start("pos-switch", START).await.unwrap();
    net.wait_finalized(&[0, 1, 2, 3], 2, START).await.unwrap();
    let dave = dev_account("dave").unwrap();
    let client = NodeClient::new(&net.nodes[3].url).unwrap();

    let switched = stake_and_switch(&net).await;
    // Local: conditions from height 40 at a boundary, held for 40 blocks.
    assert_eq!(switched % EPOCH, 1, "switch at #{switched}");
    assert!(switched >= 81, "switch at #{switched}");
    let at = net.nodes[1].hash_at(switched).await.unwrap();
    let (_, set) = set_at(&net.nodes[1], &at).await;
    let expected: Vec<PqPublicKey> = ["alice", "bob", "charlie"]
        .iter()
        .map(|n| dev_public_key(n, SigAlg::MlDsa65).unwrap())
        .collect();
    let mut keys: Vec<PqPublicKey> = set.iter().map(|a| a.key.clone()).collect();
    keys.sort_by_key(|k| k.to_canonical());
    let mut sorted = expected.clone();
    sorted.sort_by_key(|k| k.to_canonical());
    assert_eq!(keys, sorted, "PoS set");
    // Weights follow backing: alice 150,000 + 50,000 from dave, bob and charlie 250,000 each.
    let weight = |key: &PqPublicKey| set.iter().find(|a| &a.key == key).unwrap().weight;
    assert_eq!(weight(&expected[1]), weight(&expected[2]));
    assert_eq!(weight(&expected[0]) * 5, weight(&expected[1]) * 4);

    // The chain keeps finalizing with the elected set.
    net.wait_finalized(&[0, 1, 2], switched + 10, START)
        .await
        .unwrap();

    // The first settlement with work points of a full PoS epoch pays rewards.
    let dave_before = client.free_balance(&dave).await.unwrap();
    let settlement = (switched / EPOCH + 2) * EPOCH + 1;
    net.wait_finalized(&[0, 1, 2], settlement + 3, Duration::from_secs(3 * EPOCH))
        .await
        .unwrap();
    let hash = net.nodes[1].hash_at(settlement).await.unwrap();
    let settled = events(&net.nodes[1], &hash).await;
    let security = settled
        .iter()
        .find_map(|e| match e {
            RuntimeEvent::Emission(pallet_emission::Event::EpochSettled { security, .. }) => {
                Some(*security)
            }
            _ => None,
        })
        .expect("settlement event");
    let allotted = settled
        .iter()
        .find_map(|e| match e {
            RuntimeEvent::StakingPos(pallet_staking_pos::Event::RewardsAllotted {
                amount, ..
            }) => Some(*amount),
            _ => None,
        })
        .expect("rewards allotted");
    assert!(security > 0);
    assert_eq!(security, allotted, "EmissionApi and staking agree");
    // Payouts need no transaction and are done within a few blocks.
    let mut paid = 0u128;
    for block in settlement..=settlement + 3 {
        let hash = net.nodes[1].hash_at(block).await.unwrap();
        for e in events(&net.nodes[1], &hash).await {
            if let RuntimeEvent::StakingPos(pallet_staking_pos::Event::Rewarded {
                amount, ..
            }) = e
            {
                paid += amount;
            }
        }
    }
    assert!(paid > 0 && paid <= allotted, "paid {paid} of {allotted}");
    assert!(
        client.free_balance(&dave).await.unwrap() > dave_before,
        "dave was not rewarded"
    );
}

// consensus/offences Scenario "投票双签罚没全部自质押" / "出块双签罚没 10%" and "PoS 阶段违规后暂停
// 参选" on the local chain: after the switch, alice's key runs on a second node; her double
// signing is reported, her self-stake is slashed and burned, dave's nomination is untouched,
// and from the next epoch the set is bob and charlie.
#[tokio::test(flavor = "multi_thread")]
async fn double_signing_in_pos_is_slashed() {
    require_e2e!();
    let mut net = Testnet::start("pos-slash", START).await.unwrap();
    net.wait_finalized(&[0, 1, 2, 3], 2, START).await.unwrap();
    let switched = stake_and_switch(&net).await;
    net.wait_finalized(&[0, 1, 2], switched + 2, START)
        .await
        .unwrap();
    let alice = dev_account("alice").unwrap();
    let dave = dev_account("dave").unwrap();
    let before: AccountStake = api(&net.nodes[1], "StakingApi_stake", &alice.encode(), None).await;
    let dave_before: AccountStake =
        api(&net.nodes[1], "StakingApi_stake", &dave.encode(), None).await;
    assert_eq!(before.active, SELF_STAKE);
    let burned_before: u128 = api(&net.nodes[1], "EmissionApi_total_burned", &[], None).await;

    net.add_node("alice-twin", Some("alice"), &[]).unwrap();
    let started = Instant::now();
    let slashed_at = loop {
        let now: AccountStake = api(&net.nodes[1], "StakingApi_stake", &alice.encode(), None).await;
        if now.active + now.unlocking + now.withdrawable < SELF_STAKE {
            break net.nodes[1].height().await.unwrap();
        }
        assert!(
            started.elapsed() < Duration::from_secs(90),
            "alice was not slashed; logs in {}",
            net.base.display()
        );
        tokio::time::sleep(Duration::from_millis(500)).await;
    };
    let after: AccountStake = api(&net.nodes[1], "StakingApi_stake", &alice.encode(), None).await;
    let slashed = SELF_STAKE - (after.active + after.unlocking + after.withdrawable);
    // Seal double signing alone takes 10%, vote double signing everything.
    assert!(
        slashed == SELF_STAKE / 10 || slashed == SELF_STAKE,
        "slashed {slashed}"
    );
    let burned: u128 = api(&net.nodes[1], "EmissionApi_total_burned", &[], None).await;
    assert!(burned - burned_before >= slashed, "slash not burned");
    let dave_after: AccountStake =
        api(&net.nodes[1], "StakingApi_stake", &dave.encode(), None).await;
    assert_eq!(dave_after, dave_before, "the nominator was slashed");

    // From the next boundary alice is out; bob and charlie keep finalizing.
    let boundary = (slashed_at.saturating_sub(1) / EPOCH + 1) * EPOCH + 1;
    let others = [1usize, 2];
    net.wait_finalized(&others, boundary + 5, Duration::from_secs(3 * EPOCH))
        .await
        .unwrap();
    let at = net.nodes[1].hash_at(boundary + 1).await.unwrap();
    let (_, set) = set_at(&net.nodes[1], &at).await;
    let alice_key = dev_public_key("alice", SigAlg::MlDsa65).unwrap();
    assert!(
        set.iter().all(|a| a.key != alice_key),
        "alice still in the set"
    );
    assert_eq!(set.len(), 2);
}
