//! Adapter tests against an in-memory node (spec evm/eth-rpc). Each test names the scenario it
//! covers; the end-to-end scenarios with Foundry live in `tests/e2e`.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, OnceLock};

use ac_crypto::SigAlg;
use ac_crypto::sig::SigningKey;
use ac_runtime::transaction::{TxParams, assemble, authorized_extensions};
use ac_runtime::{AccountId, Hash, RuntimeCall, RuntimeEvent};
use anyhow::{Result, anyhow};
use async_trait::async_trait;
use frame_system::{EventRecord, Phase};
use pallet_revive::evm::Block;
use pallet_revive::{
    ContractResult, ExecReturnValue, H160, InstantiateReturnValue, StorageDeposit, U256,
};
use parity_scale_codec::{Decode, Encode};
use serde_json::{Value, json};
use sp_core::H256;
use sp_runtime::generic::Era;
use sp_runtime::{DispatchError, Weight};

use crate::chain::{Chain, Events, REF_TIME_PER_GAS, address_of, extrinsic_hash};
use crate::index::{Index, catch_up};
use crate::node::Node;
use crate::rpc::{Ctx, Limits, module};

const GAS_PRICE: u128 = 7;
const CALL_REF_TIME: u64 = 1_234_567;

/// A block of the in-memory chain.
#[derive(Clone, Default)]
struct MockBlock {
    hash: H256,
    extrinsics: Vec<Vec<u8>>,
    events: Events,
}

/// A dry run's answer.
#[derive(Clone)]
enum DryRun {
    Returns(Vec<u8>),
    Reverts(Vec<u8>),
}

#[derive(Default)]
struct State {
    chain: Vec<MockBlock>,
    finalized: u64,
    balances: BTreeMap<H160, U256>,
    dry_run: Option<DryRun>,
    submitted: Vec<Vec<u8>>,
    storage_reads: usize,
}

/// The in-memory node.
#[derive(Clone, Default)]
struct MockNode(Arc<Mutex<State>>);

/// The Ethereum hash the mock reports for a native block hash (distinct from it).
fn eth_hash_of(native: H256) -> H256 {
    let mut bytes = native.0;
    bytes.reverse();
    H256(bytes)
}

fn native_hash(number: u64, fork: u8) -> H256 {
    let mut bytes = [0u8; 32];
    bytes[..8].copy_from_slice(&number.to_be_bytes());
    bytes[31] = fork.wrapping_add(1);
    H256(bytes)
}

impl MockNode {
    fn with_blocks(count: u64) -> Self {
        let node = Self::default();
        {
            let mut state = node.0.lock().unwrap();
            for number in 0..count {
                state.chain.push(MockBlock {
                    hash: native_hash(number, 0),
                    ..Default::default()
                });
            }
        }
        node
    }

    fn push(&self, extrinsics: Vec<Vec<u8>>, events: Events) -> u64 {
        let mut state = self.0.lock().unwrap();
        let number = state.chain.len() as u64;
        state.chain.push(MockBlock {
            hash: native_hash(number, 0),
            extrinsics,
            events,
        });
        number
    }

    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.0.lock().unwrap()
    }

    fn number_of(&self, hash: H256) -> Option<u64> {
        let state = self.state();
        state
            .chain
            .iter()
            .position(|b| b.hash == hash)
            .map(|n| n as u64)
    }
}

#[async_trait]
impl Node for MockNode {
    async fn best_hash(&self) -> Result<H256> {
        let state = self.state();
        state
            .chain
            .last()
            .map(|b| b.hash)
            .ok_or_else(|| anyhow!("empty"))
    }

    async fn finalized_hash(&self) -> Result<H256> {
        let state = self.state();
        let n = usize::try_from(state.finalized)?;
        Ok(state.chain[n].hash)
    }

    async fn block_hash(&self, number: u64) -> Result<Option<H256>> {
        let state = self.state();
        Ok(state.chain.get(usize::try_from(number)?).map(|b| b.hash))
    }

    async fn block_number(&self, hash: H256) -> Result<Option<u64>> {
        Ok(self.number_of(hash))
    }

    async fn block_extrinsics(&self, hash: H256) -> Result<Option<Vec<Vec<u8>>>> {
        let state = self.state();
        Ok(state
            .chain
            .iter()
            .find(|b| b.hash == hash)
            .map(|b| b.extrinsics.clone()))
    }

    async fn state_call(&self, method: &str, args: Vec<u8>, at: H256) -> Result<Vec<u8>> {
        let number = self.number_of(at).ok_or_else(|| anyhow!("unknown block"))?;
        let state = self.state();
        // `ReturnFlags` is a `u32` bit set on the wire; bit 0 is REVERT.
        let dry = |data: Vec<u8>, revert: bool| {
            ExecReturnValue::decode(&mut &(u32::from(revert), data).encode()[..]).unwrap()
        };
        let (data, revert) = match state.dry_run.clone() {
            Some(DryRun::Returns(d)) => (d, false),
            Some(DryRun::Reverts(d)) => (d, true),
            None => (Vec::new(), false),
        };
        Ok(match method {
            "ReviveApi_balance" => {
                let address = H160::decode(&mut &args[..])?;
                state
                    .balances
                    .get(&address)
                    .copied()
                    .unwrap_or_default()
                    .encode()
            }
            "ReviveApi_nonce" => 5u32.encode(),
            "ReviveApi_code" => vec![0x60u8, 0x00].encode(),
            "ReviveApi_get_storage" => {
                let r: pallet_revive::GetStorageResult = Ok(Some(vec![0x2a]));
                r.encode()
            }
            "ReviveApi_account_id" => AccountId::new([7; 32]).encode(),
            "ReviveApi_call" => dry_result(Ok(dry(data, revert))).encode(),
            "ReviveApi_instantiate" => dry_result(Ok(InstantiateReturnValue {
                result: dry(data, revert),
                addr: H160::repeat_byte(0xcc),
            }))
            .encode(),
            "ReviveApi_eth_block" => Block {
                hash: eth_hash_of(at),
                number: U256::from(number),
                ..Default::default()
            }
            .encode(),
            "TransactionPaymentApi_query_weight_to_fee" => {
                let weight = Weight::decode(&mut &args[..])?;
                assert_eq!(weight.ref_time(), REF_TIME_PER_GAS);
                GAS_PRICE.encode()
            }
            other => return Err(anyhow!("unexpected runtime API {other}")),
        })
    }

    async fn storage(&self, _key: Vec<u8>, at: H256) -> Result<Option<Vec<u8>>> {
        let mut state = self.state();
        state.storage_reads += 1;
        // Like a node: storage without events (genesis) is absent, not an empty list.
        Ok(state
            .chain
            .iter()
            .find(|b| b.hash == at)
            .filter(|b| !b.events.is_empty())
            .map(|b| b.events.encode()))
    }

    async fn submit(&self, extrinsic: Vec<u8>) -> Result<H256> {
        let hash = extrinsic_hash(&extrinsic);
        self.state().submitted.push(extrinsic);
        Ok(hash)
    }
}

/// A dry-run answer needing [`CALL_REF_TIME`].
fn dry_result<R>(result: Result<R, DispatchError>) -> ContractResult<R, u128> {
    ContractResult {
        weight_consumed: Weight::from_parts(CALL_REF_TIME, 0),
        weight_required: Weight::from_parts(CALL_REF_TIME, 0),
        storage_deposit: StorageDeposit::Charge(0),
        max_storage_deposit: StorageDeposit::Charge(0),
        gas_consumed: 0,
        result,
    }
}

fn ctx(node: &MockNode) -> Arc<Ctx<MockNode>> {
    Arc::new(Ctx {
        chain: Chain::new(node.clone()),
        index: Default::default(),
        limits: Limits {
            max_log_range: 1_000,
        },
    })
}

/// Sends one JSON-RPC request and returns the whole answer object.
async fn request(ctx: &Arc<Ctx<MockNode>>, method: &str, params: Value) -> Value {
    let module = module(Arc::clone(ctx)).unwrap();
    let body = json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params});
    let (answer, _) = module.raw_json_request(&body.to_string(), 1).await.unwrap();
    serde_json::from_str(&answer).unwrap()
}

async fn ok(ctx: &Arc<Ctx<MockNode>>, method: &str, params: Value) -> Value {
    let answer = request(ctx, method, params).await;
    assert!(answer.get("error").is_none(), "{method}: {answer}");
    answer["result"].clone()
}

async fn err(ctx: &Arc<Ctx<MockNode>>, method: &str, params: Value) -> Value {
    let answer = request(ctx, method, params).await;
    answer
        .get("error")
        .cloned()
        .unwrap_or_else(|| panic!("{method} should fail: {answer}"))
}

fn signer() -> (SigningKey, AccountId) {
    let key =
        SigningKey::from_seed(SigAlg::MlDsa44, &ac_crypto::dev_seed("dave").unwrap()).unwrap();
    let who = pallet_pq_accounts::derived_account(&key.public_key().unwrap());
    (key, who)
}

/// A signed transaction carrying `call` (the signature covers an arbitrary digest: the adapter
/// decodes, it does not verify).
fn signed(call: RuntimeCall, nonce: u32) -> Vec<u8> {
    let (key, who) = signer();
    let params = TxParams {
        nonce,
        tip: 0,
        era: Era::Immortal,
        era_birth_hash: Hash::zero(),
    };
    let mut rng = ac_crypto::OsRng::new().unwrap();
    let signature = key
        .sign(&[0u8; 32], pallet_pq_accounts::TX_SIGNING_CONTEXT, &mut rng)
        .unwrap();
    assemble(call, who, signature, None, authorized_extensions(&params)).encode()
}

fn contract_call(data: Vec<u8>) -> RuntimeCall {
    RuntimeCall::Revive(pallet_revive::Call::call {
        dest: H160::repeat_byte(0xaa),
        value: 3,
        weight_limit: Weight::from_parts(50 * REF_TIME_PER_GAS, 0),
        storage_deposit_limit: 1_000,
        data,
    })
}

fn deployment() -> RuntimeCall {
    RuntimeCall::Revive(pallet_revive::Call::instantiate_with_code {
        value: 0,
        weight_limit: Weight::from_parts(10 * REF_TIME_PER_GAS, 0),
        storage_deposit_limit: 1_000,
        code: vec![0x60, 0x80],
        data: Vec::new(),
        salt: None,
    })
}

fn record(index: u32, event: RuntimeEvent) -> EventRecord<RuntimeEvent, Hash> {
    EventRecord {
        phase: Phase::ApplyExtrinsic(index),
        event,
        topics: Vec::new(),
    }
}

fn fee_paid(who: &AccountId, fee: u128) -> RuntimeEvent {
    RuntimeEvent::TransactionPayment(pallet_transaction_payment::Event::TransactionFeePaid {
        who: who.clone(),
        actual_fee: fee,
        tip: 0,
    })
}

fn success() -> RuntimeEvent {
    RuntimeEvent::System(frame_system::Event::ExtrinsicSuccess {
        dispatch_info: Default::default(),
    })
}

fn failed() -> RuntimeEvent {
    RuntimeEvent::System(frame_system::Event::ExtrinsicFailed {
        dispatch_error: DispatchError::Other("reverted"),
        dispatch_info: Default::default(),
    })
}

fn emitted(contract: H160, topics: Vec<H256>, data: Vec<u8>) -> RuntimeEvent {
    RuntimeEvent::Revive(pallet_revive::Event::ContractEmitted {
        contract,
        data,
        topics,
    })
}

async fn indexed(node: &MockNode) -> Arc<Ctx<MockNode>> {
    let ctx = ctx(node);
    catch_up(&ctx.chain, &ctx.index, 10_000).await.unwrap();
    ctx
}

fn hex(bytes: &[u8]) -> String {
    format!("0x{}", ::hex::encode(bytes))
}

// Scenario "未支持的方法".
#[tokio::test]
async fn unsupported_methods_do_not_exist() {
    let ctx = ctx(&MockNode::with_blocks(1));
    for method in ["eth_newFilter", "debug_traceTransaction", "eth_mining"] {
        assert_eq!(err(&ctx, method, json!([])).await["code"], -32601);
    }
}

// Requirement "支持的只读方法": chain facts, hex encoding.
#[tokio::test]
async fn chain_information_methods() {
    let node = MockNode::with_blocks(12);
    let ctx = ctx(&node);
    assert_eq!(ok(&ctx, "eth_chainId", json!([])).await, "0x1133");
    assert_eq!(ok(&ctx, "net_version", json!([])).await, "4403");
    assert_eq!(ok(&ctx, "eth_syncing", json!([])).await, false);
    assert_eq!(ok(&ctx, "eth_blockNumber", json!([])).await, "0xb");
    assert_eq!(ok(&ctx, "eth_gasPrice", json!([])).await, "0x7");
    assert_eq!(ok(&ctx, "eth_maxPriorityFeePerGas", json!([])).await, "0x0");
    assert!(
        ok(&ctx, "web3_clientVersion", json!([]))
            .await
            .as_str()
            .unwrap()
            .starts_with("ac-eth-rpc/")
    );
    let history = ok(&ctx, "eth_feeHistory", json!(["0x3", "latest", [50.0]])).await;
    assert_eq!(history["oldestBlock"], "0x9");
    assert_eq!(
        history["baseFeePerGas"],
        json!(["0x7", "0x7", "0x7", "0x7"])
    );
    assert_eq!(history["reward"], json!([["0x0"], ["0x0"], ["0x0"]]));
}

// Requirement "支持的只读方法": account state.
#[tokio::test]
async fn account_state_methods() {
    let node = MockNode::with_blocks(3);
    let address = H160::repeat_byte(0x11);
    node.state()
        .balances
        .insert(address, U256::from(1_000_000_000_000_000_000u128));
    let ctx = ctx(&node);
    assert_eq!(
        ok(&ctx, "eth_getBalance", json!([address, "latest"])).await,
        "0xde0b6b3a7640000"
    );
    assert_eq!(
        ok(&ctx, "eth_getTransactionCount", json!([address, "0x1"])).await,
        "0x5"
    );
    assert_eq!(ok(&ctx, "eth_getCode", json!([address])).await, "0x6000");
    assert_eq!(
        ok(&ctx, "eth_getStorageAt", json!([address, "0x0", "latest"])).await,
        format!("0x{}2a", "00".repeat(31))
    );
    // An unknown block number is an invalid parameter, not an internal error.
    assert_eq!(
        err(&ctx, "eth_getBalance", json!([address, "0x99"])).await["code"],
        -32602
    );
}

// Scenario "finalized 标签" and the other block tags.
#[tokio::test]
async fn block_tags_follow_best_and_finality() {
    let node = MockNode::with_blocks(10);
    node.state().finalized = 6;
    let ctx = ctx(&node);
    let number = |v: Value| v["number"].as_str().unwrap().to_owned();
    for (tag, expected) in [
        ("latest", "0x9"),
        ("pending", "0x9"),
        ("finalized", "0x6"),
        ("safe", "0x6"),
        ("earliest", "0x0"),
        ("0x4", "0x4"),
    ] {
        let block = ok(&ctx, "eth_getBlockByNumber", json!([tag, false])).await;
        assert_eq!(number(block), expected, "tag {tag}");
    }
    assert_eq!(
        ok(&ctx, "eth_getBlockByNumber", json!(["0x64", false])).await,
        Value::Null
    );
}

// Scenario "区块哈希一致": the block hash is revive's (what BLOCKHASH returns), and both it and
// the native hash find the block.
#[tokio::test]
async fn block_hash_is_the_evm_block_hash() {
    let node = MockNode::with_blocks(5);
    let ctx = indexed(&node).await;
    let native = native_hash(3, 0);
    let block = ok(&ctx, "eth_getBlockByNumber", json!(["0x3", false])).await;
    assert_eq!(block["hash"], json!(eth_hash_of(native)));
    for hash in [eth_hash_of(native), native] {
        let by_hash = ok(&ctx, "eth_getBlockByHash", json!([hash, false])).await;
        assert_eq!(by_hash["number"], "0x3");
    }
    assert_eq!(
        ok(
            &ctx,
            "eth_getBlockByHash",
            json!([H256::repeat_byte(9), false])
        )
        .await,
        Value::Null
    );
}

// Task 5.4: backfill depth, following new blocks and rewinding after a fork.
#[tokio::test]
async fn index_backfills_follows_and_rewinds() {
    let node = MockNode::with_blocks(20);
    let ctx = ctx(&node);
    catch_up(&ctx.chain, &ctx.index, 5).await.unwrap();
    {
        let index = ctx.index.read().await;
        assert_eq!((index.first(), index.tip()), (Some(14), Some(19)));
    }
    let (_, who) = signer();
    let xt = signed(contract_call(vec![1]), 0);
    let hash = extrinsic_hash(&xt);
    let n = node.push(
        vec![xt],
        vec![record(0, success()), record(0, fee_paid(&who, 1))],
    );
    catch_up(&ctx.chain, &ctx.index, 5).await.unwrap();
    assert_eq!(ctx.index.read().await.tx(&hash).map(|l| l.number), Some(n));

    // A fork replaces blocks 18.. with blocks that no longer carry the transaction.
    {
        let mut state = node.state();
        state.chain.truncate(18);
        for number in 18..23 {
            state.chain.push(MockBlock {
                hash: native_hash(number, 1),
                ..Default::default()
            });
        }
    }
    catch_up(&ctx.chain, &ctx.index, 5).await.unwrap();
    let index = ctx.index.read().await;
    assert_eq!(index.tip(), Some(22));
    assert_eq!(index.tx(&hash), None);
    assert_eq!(index.number_of(&native_hash(20, 0)), None);
    assert_eq!(index.number_of(&native_hash(20, 1)), Some(20));
    assert_eq!(index.number_of(&eth_hash_of(native_hash(20, 1))), Some(20));
}

#[test]
fn index_insert_replaces_a_block() {
    let mut index = Index::default();
    let tx = H256::repeat_byte(1);
    index.insert(
        4,
        crate::index::IndexedBlock {
            hash: native_hash(4, 0),
            eth_hash: eth_hash_of(native_hash(4, 0)),
            contract_txs: vec![(0, tx)],
        },
    );
    index.insert(
        4,
        crate::index::IndexedBlock {
            hash: native_hash(4, 1),
            eth_hash: eth_hash_of(native_hash(4, 1)),
            contract_txs: Vec::new(),
        },
    );
    assert_eq!(index.tx(&tx), None);
    assert_eq!(index.number_of(&native_hash(4, 0)), None);
}

// Scenario "eth_call 回滚".
#[tokio::test]
async fn eth_call_revert_returns_code_3_with_data() {
    let node = MockNode::with_blocks(2);
    // Error(string) "no".
    let revert = ::hex::decode(
        "08c379a0\
         0000000000000000000000000000000000000000000000000000000000000020\
         0000000000000000000000000000000000000000000000000000000000000002\
         6e6f000000000000000000000000000000000000000000000000000000000000",
    )
    .unwrap();
    node.state().dry_run = Some(DryRun::Reverts(revert.clone()));
    let ctx = ctx(&node);
    let call = json!({"to": H160::repeat_byte(0xaa), "data": "0x12345678"});
    let error = err(&ctx, "eth_call", json!([call, "latest"])).await;
    assert_eq!(error["code"], 3);
    assert_eq!(error["data"], hex(&revert));
    assert_eq!(err(&ctx, "eth_estimateGas", json!([call])).await["code"], 3);
}

// Requirement "模拟调用与估算": results, deployments and the gas figure.
#[tokio::test]
async fn eth_call_and_estimate() {
    let node = MockNode::with_blocks(2);
    node.state().dry_run = Some(DryRun::Returns(vec![0xab; 32]));
    let ctx = ctx(&node);
    let call = json!({"from": H160::repeat_byte(1), "to": H160::repeat_byte(0xaa), "data": "0x"});
    assert_eq!(
        ok(&ctx, "eth_call", json!([call, "latest"])).await,
        hex(&[0xab; 32])
    );
    // Rounded up, so a transaction with this limit has at least the weight it needs.
    let gas = CALL_REF_TIME.div_ceil(REF_TIME_PER_GAS);
    assert_eq!(
        ok(&ctx, "eth_estimateGas", json!([call])).await,
        format!("{gas:#x}")
    );
    let deploy = json!({"data": "0x6080"});
    assert_eq!(
        ok(&ctx, "eth_estimateGas", json!([deploy, "latest"])).await,
        format!("{gas:#x}")
    );
}

// Scenario "部署回执" and the transaction object.
#[tokio::test]
async fn deployment_receipt() {
    let node = MockNode::with_blocks(3);
    let (_, who) = signer();
    let from = address_of(&who);
    let contract = H160::repeat_byte(0xcc);
    let xt = signed(deployment(), 4);
    let hash = extrinsic_hash(&xt);
    let fee = 1_000_003u128;
    let n = node.push(
        vec![vec![0u8], xt],
        vec![
            record(
                1,
                RuntimeEvent::Revive(pallet_revive::Event::Instantiated {
                    deployer: from,
                    contract,
                }),
            ),
            record(1, fee_paid(&who, fee)),
            record(1, success()),
        ],
    );
    let ctx = indexed(&node).await;
    let receipt = ok(&ctx, "eth_getTransactionReceipt", json!([hash])).await;
    assert_eq!(receipt["status"], "0x1");
    assert_eq!(receipt["contractAddress"], json!(contract));
    assert_eq!(receipt["to"], Value::Null);
    assert_eq!(receipt["from"], json!(from));
    assert_eq!(receipt["transactionIndex"], "0x1");
    assert_eq!(receipt["blockNumber"], format!("{n:#x}"));
    assert_eq!(receipt["blockHash"], json!(eth_hash_of(native_hash(n, 0))));
    let tx = ok(&ctx, "eth_getTransactionByHash", json!([hash])).await;
    assert_eq!(tx["nonce"], "0x4");
    assert_eq!(tx["to"], Value::Null);
    assert_eq!(tx["input"], "0x6080");
    assert_eq!(tx["gas"], "0xa");
    assert_eq!(tx["chainId"], "0x1133");
    let block = ok(
        &ctx,
        "eth_getBlockByNumber",
        json!([format!("{n:#x}"), false]),
    )
    .await;
    assert_eq!(block["transactions"], json!([hash]));
    let full = ok(
        &ctx,
        "eth_getBlockByNumber",
        json!([format!("{n:#x}"), true]),
    )
    .await;
    assert_eq!(full["transactions"][0]["hash"], json!(hash));
}

// Scenario "回滚回执": status 0, no logs, and gasUsed × effectiveGasPrice within one gas price
// of the fee actually paid.
#[tokio::test]
async fn reverted_call_receipt() {
    let node = MockNode::with_blocks(3);
    let (_, who) = signer();
    let xt = signed(contract_call(vec![9]), 0);
    let hash = extrinsic_hash(&xt);
    let fee = 1_000_003u128;
    node.push(
        vec![xt],
        vec![record(0, fee_paid(&who, fee)), record(0, failed())],
    );
    let ctx = indexed(&node).await;
    let receipt = ok(&ctx, "eth_getTransactionReceipt", json!([hash])).await;
    assert_eq!(receipt["status"], "0x0");
    assert_eq!(receipt["logs"], json!([]));
    assert_eq!(receipt["to"], json!(H160::repeat_byte(0xaa)));
    let gas_used = u128::from_str_radix(
        receipt["gasUsed"]
            .as_str()
            .unwrap()
            .trim_start_matches("0x"),
        16,
    )
    .unwrap();
    let price = u128::from_str_radix(
        receipt["effectiveGasPrice"]
            .as_str()
            .unwrap()
            .trim_start_matches("0x"),
        16,
    )
    .unwrap();
    assert_eq!(price, GAS_PRICE);
    let charged = gas_used * price;
    assert!(
        charged >= fee && charged - fee < price,
        "{charged} vs {fee}"
    );
}

// Scenario "未知交易".
#[tokio::test]
async fn unknown_transaction_is_null() {
    let ctx = indexed(&MockNode::with_blocks(3)).await;
    let unknown = H256::repeat_byte(0x42);
    assert_eq!(
        ok(&ctx, "eth_getTransactionReceipt", json!([unknown])).await,
        Value::Null
    );
    assert_eq!(
        ok(&ctx, "eth_getTransactionByHash", json!([unknown])).await,
        Value::Null
    );
}

// Scenario "按主题过滤", with log order and numbering.
#[tokio::test]
async fn logs_filter_by_address_and_topics() {
    let node = MockNode::with_blocks(2);
    let (_, who) = signer();
    let token = H160::repeat_byte(0x70);
    let other = H160::repeat_byte(0x71);
    let transfer = H256::repeat_byte(0xdd);
    let alice = H256::from(H160::repeat_byte(0xa1));
    let bob = H256::from(H160::repeat_byte(0xb0));
    let first = signed(contract_call(vec![1]), 0);
    let second = signed(contract_call(vec![2]), 1);
    let (h1, h2) = (extrinsic_hash(&first), extrinsic_hash(&second));
    let n = node.push(
        vec![first, second],
        vec![
            record(0, emitted(token, vec![transfer, alice, bob], vec![1])),
            record(0, emitted(other, vec![transfer, alice, bob], vec![2])),
            record(0, success()),
            record(0, fee_paid(&who, 1)),
            record(1, emitted(token, vec![transfer, bob, alice], vec![3])),
            record(1, emitted(token, vec![transfer, alice, bob], vec![4])),
            record(1, success()),
            record(1, fee_paid(&who, 1)),
        ],
    );
    let ctx = indexed(&node).await;
    let range = format!("{n:#x}");
    let to_bob = ok(
        &ctx,
        "eth_getLogs",
        json!([{"fromBlock": range, "toBlock": range, "address": token,
                "topics": [transfer, null, bob]}]),
    )
    .await;
    let logs = to_bob.as_array().unwrap();
    assert_eq!(logs.len(), 2);
    assert_eq!(logs[0]["data"], "0x01");
    assert_eq!(logs[0]["logIndex"], "0x0");
    assert_eq!(logs[0]["transactionHash"], json!(h1));
    assert_eq!(logs[0]["transactionIndex"], "0x0");
    assert_eq!(logs[1]["data"], "0x04");
    assert_eq!(logs[1]["logIndex"], "0x3");
    assert_eq!(logs[1]["transactionHash"], json!(h2));
    assert_eq!(logs[1]["blockHash"], json!(eth_hash_of(native_hash(n, 0))));

    // Address lists and per-position alternatives.
    let either = ok(
        &ctx,
        "eth_getLogs",
        json!([{"blockHash": native_hash(n, 0), "address": [token, other],
                "topics": [[transfer], [alice, bob]]}]),
    )
    .await;
    assert_eq!(either.as_array().unwrap().len(), 4);

    // The transaction's receipt carries exactly its own logs.
    let receipt = ok(&ctx, "eth_getTransactionReceipt", json!([h2])).await;
    assert_eq!(receipt["logs"].as_array().unwrap().len(), 2);
}

// Scenario "超出范围上限".
#[tokio::test]
async fn log_range_above_the_limit_is_refused() {
    let node = MockNode::with_blocks(1_200);
    let ctx = ctx(&node);
    let reads = node.state().storage_reads;
    let error = err(
        &ctx,
        "eth_getLogs",
        json!([{"fromBlock": "0x0", "toBlock": format!("{:#x}", 1_000)}]),
    )
    .await;
    assert_eq!(error["code"], -32005);
    assert!(error["message"].as_str().unwrap().contains("1000"));
    // Refused up front, not truncated after reading.
    assert_eq!(node.state().storage_reads, reads);
    ok(
        &ctx,
        "eth_getLogs",
        json!([{"fromBlock": "0x0", "toBlock": format!("{:#x}", 999)}]),
    )
    .await;
}

// Scenario "提交原生合约交易": forwarded, with the node's hash.
#[tokio::test]
async fn contract_transactions_are_forwarded() {
    let node = MockNode::with_blocks(1);
    let ctx = ctx(&node);
    for xt in [signed(contract_call(vec![1]), 0), signed(deployment(), 1)] {
        let hash = ok(&ctx, "eth_sendRawTransaction", json!([hex(&xt)])).await;
        assert_eq!(hash, json!(extrinsic_hash(&xt)));
    }
    assert_eq!(node.state().submitted.len(), 2);
}

// Scenarios "拒绝以太坊签名交易" and "拒绝非合约调用": refused, nothing reaches the node.
#[tokio::test]
async fn other_raw_transactions_are_refused() {
    let node = MockNode::with_blocks(1);
    let ctx = ctx(&node);
    // The EIP-155 legacy example and typed transactions (EIP-2930, EIP-1559, EIP-4844,
    // EIP-7702): type byte, then an RLP list, the same shapes as the runtime's decoding test.
    let legacy = concat!(
        "0xf86c098504a817c800825208943535353535353535353535353535353535353535880de0b6b3a764000080",
        "25a028ef61340bd939bc2195fe537567866003e1a15d3c71ff63e1590620aa636276a067cbe9d8997f761a",
        "ecb703304b3800ccf555c9f3dc64214b297fb1966a3b6d83"
    );
    let typed = |ty: u8| {
        let mut tx = vec![ty, 0xf8, 0x50, 0x82, 0x11, 0x33];
        tx.extend(std::iter::repeat_n(0x80, 0x4d));
        hex(&tx)
    };
    let transfer = RuntimeCall::Balances(pallet_balances::Call::transfer_keep_alive {
        dest: AccountId::new([1; 32]),
        value: 1,
    });
    let cases = [
        (legacy.to_owned(), "ML-DSA"),
        (typed(0x01), "ML-DSA"),
        (typed(0x02), "ML-DSA"),
        (typed(0x03), "ML-DSA"),
        (typed(0x04), "ML-DSA"),
        ("0xdeadbeef".to_owned(), "ML-DSA"),
        (hex(&signed(transfer, 0)), "contract calls"),
    ];
    for (bytes, reason) in cases {
        let error = err(&ctx, "eth_sendRawTransaction", json!([bytes])).await;
        assert_eq!(error["code"], -32000);
        assert!(
            error["message"].as_str().unwrap().contains(reason),
            "{error}"
        );
    }
    assert!(node.state().submitted.is_empty());
}

// Scenario "拒绝节点签名".
#[tokio::test]
async fn signing_methods_point_to_the_wallet() {
    let node = MockNode::with_blocks(1);
    let ctx = ctx(&node);
    for method in crate::rpc::SIGNING_METHODS {
        let error = err(&ctx, method, json!([{"from": H160::zero()}])).await;
        assert_eq!(error["code"], -32000);
        assert!(error["message"].as_str().unwrap().contains("ac-wallet evm"));
    }
    assert!(node.state().submitted.is_empty());
}

// Scenario "无私钥": nothing to sign with, no accounts.
#[tokio::test]
async fn no_accounts() {
    let ctx = ctx(&MockNode::with_blocks(1));
    assert_eq!(ok(&ctx, "eth_accounts", json!([])).await, json!([]));
}

/// Captures every log line of the test binary.
struct Capture(Mutex<Vec<String>>);

impl log::Log for Capture {
    fn enabled(&self, _: &log::Metadata<'_>) -> bool {
        true
    }

    fn log(&self, record: &log::Record<'_>) {
        // What the adapter's logger would write.
        if crate::logging::shown(record.metadata()) {
            self.0.lock().unwrap().push(format!("{}", record.args()));
        }
    }

    fn flush(&self) {}
}

fn captured() -> &'static Capture {
    static CAPTURE: OnceLock<&'static Capture> = OnceLock::new();
    CAPTURE.get_or_init(|| {
        let capture: &'static Capture = Box::leak(Box::new(Capture(Mutex::new(Vec::new()))));
        let _ = log::set_logger(capture);
        log::set_max_level(log::LevelFilter::Debug);
        capture
    })
}

// Scenario "日志不含请求内容": debug logs name the method, never the data.
#[tokio::test]
async fn logs_hold_no_request_content() {
    let capture = captured();
    let node = MockNode::with_blocks(2);
    node.state().dry_run = Some(DryRun::Returns(vec![0x5e; 4]));
    let ctx = ctx(&node);
    let call = json!({"to": H160::repeat_byte(0xaa), "data": "0xc0ffee42"});
    ok(&ctx, "eth_call", json!([call, "latest"])).await;
    let xt = signed(contract_call(vec![0xc0, 0xff, 0xee]), 0);
    ok(&ctx, "eth_sendRawTransaction", json!([hex(&xt)])).await;
    let lines = capture.0.lock().unwrap().clone();
    assert!(lines.iter().any(|l| l.starts_with("eth_call ok")));
    for line in &lines {
        for secret in ["c0ffee", "5e5e5e5e", "aaaaaaaa"] {
            assert!(!line.contains(secret), "log line leaks content: {line}");
        }
    }
}
