//! Acceptance of m3-economics on the four-authority local testnet (task 8.3): emission over
//! several epochs matches the settlement formula (spec economics/emission "链上结果与模拟一致"),
//! and the 2-of-3 PoA multisig spends from community grants (governance/poa-admin
//! "达到门限后执行"). Enabled with `AC_E2E=1`.

// Test code: failures should abort the test.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    missing_docs
)]

use std::time::Duration;

use ac_crypto::SigAlg;
use ac_crypto::sig::SigningKey;
use ac_e2e::{TestNode, Testnet, enabled};
use ac_primitives::emission::{EmissionSchedule, EpochInput, Phase, settle};
use ac_runtime::transaction::{TxParams, assemble, authorized_extensions, implicit_from, payload};
use ac_runtime::{ATC, AccountId, RuntimeCall, RuntimeEvent};
use ac_wallet::NodeClient;
use frame_support::dispatch::GetDispatchInfo;
use jsonrpsee::core::client::ClientT;
use jsonrpsee::rpc_params;
use parity_scale_codec::{Decode, Encode};
use sp_runtime::generic::Era;

const START: Duration = Duration::from_secs(120);
/// Emission epoch length of the `local` preset.
const EPOCH: u64 = 20;

macro_rules! require_e2e {
    () => {
        if !enabled() {
            eprintln!("end-to-end test disabled; run with AC_E2E=1 after `cargo build -p ac-node`");
            return;
        }
    };
}

async fn api<T: Decode>(node: &TestNode, method: &str, args: &[u8], at: &str) -> T {
    T::decode(&mut &node.state_call(method, args, Some(at)).await.unwrap()[..]).unwrap()
}

/// Events of block `hash`, read from `System::Events`.
async fn events(node: &TestNode, hash: &str) -> Vec<RuntimeEvent> {
    let key = frame_support::storage::storage_prefix(b"System", b"Events");
    let key = format!("0x{}", hex_string(&key));
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

fn hex_string(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

// Scenario "链上结果与模拟一致": after three settled epochs every node reports the same
// emission, equal to the settlement formula replayed off chain, and each settlement block
// carries its `EpochSettled` event.
#[tokio::test(flavor = "multi_thread")]
async fn emission_matches_the_formula() {
    require_e2e!();
    let net = Testnet::start("emission", START).await.unwrap();
    let target = 3 * EPOCH + 1;
    net.wait_finalized(&[0, 1, 2, 3], target, START)
        .await
        .unwrap();
    let at = net.nodes[0].hash_at(target).await.unwrap();

    let schedule = EmissionSchedule::new(EPOCH).unwrap();
    let mut reserve = 0u128;
    let mut minted = 0u128;
    for epoch in 0..3u64 {
        let out = settle(&EpochInput::new(
            schedule.scheduled(epoch),
            reserve,
            (0, 0),
            Phase::Poa,
        ));
        reserve = out.reserve + out.security;
        minted += out.total - out.security;
        let block = (epoch + 1) * EPOCH + 1;
        let hash = net.nodes[1].hash_at(block).await.unwrap();
        let settled = events(&net.nodes[1], &hash)
            .await
            .into_iter()
            .find_map(|e| match e {
                RuntimeEvent::Emission(pallet_emission::Event::EpochSettled {
                    epoch: e,
                    minted,
                    floor_topup,
                    reserve,
                    ..
                }) => Some((e, minted, floor_topup, reserve)),
                _ => None,
            });
        assert_eq!(
            settled,
            Some((
                epoch,
                out.treasury_floor_topup,
                out.treasury_floor_topup,
                reserve
            )),
            "epoch {epoch}"
        );
    }
    for node in &net.nodes {
        assert_eq!(
            api::<u64>(node, "EmissionApi_epoch_length", &[], &at).await,
            EPOCH
        );
        assert_eq!(
            api::<u128>(node, "EmissionApi_total_minted", &[], &at).await,
            minted
        );
        assert_eq!(
            api::<u128>(node, "EmissionApi_reserve", &[], &at).await,
            reserve
        );
        let (_, floor) = api::<(AccountId, u128)>(node, "TreasuryApi_floor", &[], &at).await;
        assert_eq!(floor, minted);
    }
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

// Scenario "达到门限后执行" on the local chain: alice proposes a community-grants spend through
// her node, bob approves through his, and once closed the spend has run exactly once, as seen
// from a third node.
#[tokio::test(flavor = "multi_thread")]
async fn multisig_spends_community_grants() {
    require_e2e!();
    let net = Testnet::start("multisig", START).await.unwrap();
    net.wait_all(&[0, 1, 2, 3], 2, START).await.unwrap();
    let alice = NodeClient::new(&net.nodes[0].url).unwrap();
    let bob = NodeClient::new(&net.nodes[1].url).unwrap();
    let charlie = NodeClient::new(&net.nodes[2].url).unwrap();

    let at = net.nodes[0].hash_at(1).await.unwrap();
    let (community, _) =
        api::<(AccountId, u128)>(&net.nodes[0], "TreasuryApi_community", &[], &at).await;
    let fund = RuntimeCall::Balances(pallet_balances::Call::transfer_keep_alive {
        dest: community.clone(),
        value: 10 * ATC,
    });
    assert!(submit_as(&alice, "dave", fund).await);
    // Charlie's node must have imported what alice's node included before we read from it.
    let synced = || async {
        let height = net.nodes[0].height().await.unwrap();
        net.wait_all(&[2], height, START).await.unwrap();
    };

    let recipient = ac_runtime::genesis_config_presets::dev_account("dave").unwrap();
    synced().await;
    let before = charlie.free_balance(&recipient).await.unwrap();
    let spend = RuntimeCall::TreasuryDual(pallet_treasury_dual::Call::spend {
        to: recipient.clone(),
        amount: 3 * ATC,
    });
    let hash = <ac_primitives::Blake3Hasher as sp_runtime::traits::Hash>::hash_of(&spend);
    let length_bound = u32::try_from(spend.encoded_size()).unwrap();
    let propose = RuntimeCall::PoaCouncil(pallet_collective::Call::propose {
        threshold: 2,
        proposal: Box::new(spend.clone()),
        length_bound,
    });
    assert!(submit_as(&alice, "alice", propose).await);
    let vote = |approve| {
        RuntimeCall::PoaCouncil(pallet_collective::Call::vote {
            proposal: hash,
            index: 0,
            approve,
        })
    };
    assert!(submit_as(&alice, "alice", vote(true)).await);
    assert!(submit_as(&bob, "bob", vote(true)).await);
    let close = RuntimeCall::PoaCouncil(pallet_collective::Call::close {
        proposal_hash: hash,
        index: 0,
        proposal_weight_bound: spend.get_dispatch_info().call_weight,
        length_bound,
    });
    assert!(submit_as(&alice, "alice", close.clone()).await);
    synced().await;
    assert_eq!(
        charlie.free_balance(&recipient).await.unwrap(),
        before + 3 * ATC
    );
    // Closing again fails: the motion ran once.
    assert!(!submit_as(&alice, "alice", close).await);
    synced().await;
    assert_eq!(
        charlie.free_balance(&recipient).await.unwrap(),
        before + 3 * ATC
    );
}
