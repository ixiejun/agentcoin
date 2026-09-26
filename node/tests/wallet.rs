//! Wallet against a development node (clients/wallet-cli), tasks 8.1–8.4. The wallet library is
//! exercised here because this crate can start the node binary.

// Test helpers unwrap freely; failures should abort the test.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, missing_docs)]

mod common;

use std::time::Duration;

use ac_crypto::SigAlg;
use ac_crypto::sig::SigningKey;
use ac_runtime::transaction::{TxParams, assemble, authorized_extensions, implicit_from, payload};
use ac_runtime::{ATC, RuntimeCall};
use ac_wallet::{NodeClient, Wallet, ops, parse_address};
use common::{start_dev_node, wait_for_height};
use sp_runtime::generic::Era;

/// Sends `amount` from the development account `alice` to `to`.
async fn fund(client: &NodeClient, to: &sp_runtime::AccountId32, amount: u128) {
    let key =
        SigningKey::from_seed(SigAlg::MlDsa44, &ac_crypto::dev_seed("alice").unwrap()).unwrap();
    let alice = pallet_pq_accounts::derived_account(&key.public_key().unwrap());
    let context = client.chain_context().await.unwrap();
    let params = TxParams {
        nonce: client.nonce(&alice).await.unwrap(),
        tip: 0,
        era: Era::Immortal,
        era_birth_hash: context.genesis_hash,
    };
    let call = RuntimeCall::Balances(pallet_balances::Call::transfer_allow_death {
        dest: to.clone(),
        value: amount,
    });
    let extensions = authorized_extensions(&params);
    let digest = payload(&call, &extensions, &implicit_from(&context, &params)).unwrap();
    let mut rng = ac_crypto::OsRng::new().unwrap();
    let signature = key
        .sign(&digest, pallet_pq_accounts::TX_SIGNING_CONTEXT, &mut rng)
        .unwrap();
    let first = client.current_key(&alice).await.unwrap().is_none();
    let xt = assemble(
        call,
        alice,
        signature,
        first.then(|| key.public_key().unwrap()),
        extensions,
    );
    assert!(
        client
            .submit_and_watch(&xt, Duration::from_secs(60))
            .await
            .unwrap()
            .success
    );
}

async fn dev_node() -> (common::Node, NodeClient) {
    let node = start_dev_node(&[]);
    wait_for_height(&node, 1, Duration::from_secs(120)).await;
    let url = node.rpc_url.clone();
    (node, NodeClient::new(&url).unwrap())
}

// Task 8.1 (fallback of design D11) and Scenario "首次转账": a fresh, funded but unregistered
// wallet account sends its first transfer; the key is attached automatically and registered.
#[tokio::test(flavor = "multi_thread")]
async fn first_transfer_registers_the_key() {
    let (_node, client) = dev_node().await;
    let wallet = Wallet::create(SigAlg::MlDsa44, b"pw").unwrap().wallet;
    let me = wallet.account().unwrap();
    fund(&client, &me, 10 * ATC).await;
    assert!(client.current_key(&me).await.unwrap().is_none());

    let bob = parse_address(&ac_primitives::encode_address(&[0xB0; 32])).unwrap();
    let inclusion = ops::transfer(&client, &wallet, b"pw", &bob, 2 * ATC)
        .await
        .unwrap();
    assert!(inclusion.success);
    assert_eq!(client.free_balance(&bob).await.unwrap(), 2 * ATC);
    assert!(client.current_key(&me).await.unwrap().is_some());
}

// Scenario "轮换后继续使用": rotate to ML-DSA-65, then transfer from the same address.
#[tokio::test(flavor = "multi_thread")]
async fn rotation_keeps_the_address_and_later_transfers_work() {
    let (_node, client) = dev_node().await;
    let mut wallet = Wallet::create(SigAlg::MlDsa44, b"pw").unwrap().wallet;
    let me = wallet.account().unwrap();
    let address = wallet.address().to_string();
    fund(&client, &me, 10 * ATC).await;

    ops::rotate(&client, &mut wallet, b"pw", SigAlg::MlDsa65)
        .await
        .unwrap();
    assert_eq!(wallet.address(), address);
    assert_eq!(wallet.current(), (SigAlg::MlDsa65, 1));
    let (key, rotations) = client.current_key(&me).await.unwrap().unwrap();
    assert_eq!((key.alg(), rotations), (SigAlg::MlDsa65, 1));

    let carol = parse_address(&ac_primitives::encode_address(&[0xCA; 32])).unwrap();
    let inclusion = ops::transfer(&client, &wallet, b"pw", &carol, ATC)
        .await
        .unwrap();
    assert!(inclusion.success);
    assert_eq!(client.free_balance(&carol).await.unwrap(), ATC);
}
