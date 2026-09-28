//! The Ethereum JSON-RPC methods (spec evm/eth-rpc; design D9 of m4-evm).

use std::sync::Arc;
use std::time::Instant;

use ac_primitives::evm::{ContractTransaction, EVM_CHAIN_ID};
use ac_runtime::{AccountId, RuntimeEvent};
use anyhow::anyhow;
use jsonrpsee::RpcModule;
use jsonrpsee::types::{ErrorObjectOwned, Params};
use pallet_revive::evm::{
    AddressOrAddresses, BlockNumberOrTag, BlockNumberOrTagOrHash, BlockTag, Bytes, FilterTopic,
    GenericTransaction, HashesOrTransactionInfos, Log, ReceiptInfo,
};
use pallet_revive::{H160, U256};
use serde_json::{Value, json};
use sp_core::H256;
use tokio::sync::RwLock;

use crate::chain::{
    Chain, ContractTx, DryRun, Refusal, address_of, decode_contract_tx, gas_for_fee, gas_of,
};
use crate::index::{Index, TxLocation};
use crate::node::Node;

/// Invalid parameters (JSON-RPC).
pub const INVALID_PARAMS: i32 = -32602;
/// Execution reverted (Ethereum convention).
pub const REVERTED: i32 = 3;
/// Refused transaction or signing method.
pub const REFUSED: i32 = -32000;
/// `eth_getLogs` range above the limit.
pub const LIMIT_EXCEEDED: i32 = -32005;
/// Any other failure (node unreachable, undecodable state…).
pub const INTERNAL: i32 = -32603;

/// What to tell callers of signing methods and of Ethereum-signed raw transactions.
pub const USE_WALLET: &str = "AgentCoin accepts only ML-DSA-signed AgentCoin transactions: \
build and sign contract transactions with `ac-wallet evm deploy|send|broadcast|raw`, then submit \
them here with eth_sendRawTransaction";

/// Adapter settings that shape answers.
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    /// Largest block span of one `eth_getLogs` query.
    pub max_log_range: u64,
}

/// Shared state of the RPC methods.
pub struct Ctx<N> {
    /// Chain access.
    pub chain: Chain<N>,
    /// Block and transaction index.
    pub index: RwLock<Index>,
    /// Limits.
    pub limits: Limits,
}

type RpcResult<T> = Result<T, ErrorObjectOwned>;

fn error(code: i32, message: impl Into<String>) -> ErrorObjectOwned {
    ErrorObjectOwned::owned::<()>(code, message.into(), None)
}

fn internal(err: anyhow::Error) -> ErrorObjectOwned {
    // Only the error class is exposed; details stay out of logs and answers alike.
    let _ = err;
    error(INTERNAL, "node request failed")
}

fn hex_u64(n: u64) -> String {
    format!("{n:#x}")
}

fn hex_bytes(bytes: &[u8]) -> String {
    format!("0x{}", hex::encode(bytes))
}

fn to_value<T: serde::Serialize>(value: &T) -> RpcResult<Value> {
    serde_json::to_value(value).map_err(|_| error(INTERNAL, "serialization failed"))
}

impl<N: Node> Ctx<N> {
    /// Resolves an Ethereum block parameter to (number, native hash).
    async fn resolve(&self, block: &BlockNumberOrTagOrHash) -> RpcResult<(u64, H256)> {
        let node = self.chain.node();
        match block {
            BlockNumberOrTagOrHash::BlockTag(tag) => self.resolve_tag(*tag).await,
            BlockNumberOrTagOrHash::BlockNumber(number) => {
                let number = u64::try_from(*number)
                    .map_err(|_| error(INVALID_PARAMS, "block number out of range"))?;
                let hash = node
                    .block_hash(number)
                    .await
                    .map_err(internal)?
                    .ok_or_else(|| error(INVALID_PARAMS, "unknown block"))?;
                Ok((number, hash))
            }
            BlockNumberOrTagOrHash::BlockHash(hash) => {
                // An Ethereum block hash (as seen by BLOCKHASH) or a native one.
                let number = match self.index.read().await.number_of(hash) {
                    Some(number) => number,
                    None => node
                        .block_number(*hash)
                        .await
                        .map_err(internal)?
                        .ok_or_else(|| error(INVALID_PARAMS, "unknown block"))?,
                };
                let native = node
                    .block_hash(number)
                    .await
                    .map_err(internal)?
                    .ok_or_else(|| error(INVALID_PARAMS, "unknown block"))?;
                Ok((number, native))
            }
        }
    }

    async fn resolve_tag(&self, tag: BlockTag) -> RpcResult<(u64, H256)> {
        let node = self.chain.node();
        let hash = match tag {
            BlockTag::Latest | BlockTag::Pending => node.best_hash().await,
            // AC-BFT finality: `safe` and `finalized` are the same block.
            BlockTag::Finalized | BlockTag::Safe => node.finalized_hash().await,
            BlockTag::Earliest => node
                .block_hash(0)
                .await
                .and_then(|h| h.ok_or_else(|| anyhow!("no genesis"))),
        }
        .map_err(internal)?;
        let number = node
            .block_number(hash)
            .await
            .map_err(internal)?
            .ok_or_else(|| error(INTERNAL, "unknown block"))?;
        Ok((number, hash))
    }

    async fn resolve_number_or_tag(&self, block: Option<BlockNumberOrTag>) -> RpcResult<u64> {
        match block.unwrap_or_default() {
            BlockNumberOrTag::BlockTag(tag) => Ok(self.resolve_tag(tag).await?.0),
            BlockNumberOrTag::U256(n) => {
                u64::try_from(n).map_err(|_| error(INVALID_PARAMS, "block number out of range"))
            }
        }
    }

    async fn best(&self) -> RpcResult<(u64, H256)> {
        self.resolve_tag(BlockTag::Latest).await
    }

    /// The account a call request runs as (zero address when `from` is absent).
    async fn origin(&self, from: Option<H160>, at: H256) -> RpcResult<AccountId> {
        self.chain
            .account_id(from.unwrap_or_default(), at)
            .await
            .map_err(internal)
    }

    /// `eth_call`: a dry run; reverts become error 3 with the revert data.
    pub async fn call(
        &self,
        tx: GenericTransaction,
        block: BlockNumberOrTagOrHash,
    ) -> RpcResult<Bytes> {
        let (_, at) = self.resolve(&block).await?;
        let (data, reverted, _) = self.dry_run(&tx, at).await?;
        if reverted {
            return Err(ErrorObjectOwned::owned(
                REVERTED,
                "execution reverted",
                Some(hex_bytes(&data)),
            ));
        }
        Ok(Bytes(data))
    }

    /// `eth_estimateGas`: the weight a dry run needs, in gas units.
    pub async fn estimate_gas(
        &self,
        tx: GenericTransaction,
        block: Option<BlockNumberOrTagOrHash>,
    ) -> RpcResult<U256> {
        let (_, at) = self.resolve(&block.unwrap_or_default()).await?;
        let (data, reverted, ref_time) = self.dry_run(&tx, at).await?;
        if reverted {
            return Err(ErrorObjectOwned::owned(
                REVERTED,
                "execution reverted",
                Some(hex_bytes(&data)),
            ));
        }
        Ok(U256::from(gas_of(ref_time)))
    }

    /// Runs `tx` as a dry run: (output, reverted, required ref time).
    async fn dry_run(&self, tx: &GenericTransaction, at: H256) -> RpcResult<(Vec<u8>, bool, u64)> {
        let origin = self.origin(tx.from, at).await?;
        let value = u128::try_from(tx.value.unwrap_or_default())
            .map_err(|_| error(INVALID_PARAMS, "value out of range"))?;
        let request = DryRun {
            origin,
            value,
            data: tx.input.clone().to_vec(),
        };
        match tx.to {
            Some(to) => {
                let result = self
                    .chain
                    .dry_call(request, to, at)
                    .await
                    .map_err(internal)?;
                let required = result.weight_required.ref_time();
                let out = result
                    .result
                    .map_err(|_| error(REVERTED, "execution failed"))?;
                let reverted = out.did_revert();
                Ok((out.data, reverted, required))
            }
            None => {
                let result = self
                    .chain
                    .dry_instantiate(request, at)
                    .await
                    .map_err(internal)?;
                let required = result.weight_required.ref_time();
                let out = result
                    .result
                    .map_err(|_| error(REVERTED, "deployment failed"))?;
                let reverted = out.result.did_revert();
                Ok((out.result.data, reverted, required))
            }
        }
    }

    /// `eth_sendRawTransaction`: only ML-DSA-signed AgentCoin contract transactions.
    pub async fn send_raw(&self, bytes: Bytes) -> RpcResult<H256> {
        match decode_contract_tx(&bytes.0) {
            Ok(_) => self
                .chain
                .node()
                .submit(bytes.0)
                .await
                .map_err(|_| error(REFUSED, "the node rejected the transaction")),
            Err(Refusal::NotAgentCoin) => Err(error(
                REFUSED,
                format!(
                    "not an AgentCoin transaction (Ethereum-signed transactions are refused). {USE_WALLET}"
                ),
            )),
            Err(Refusal::Unsigned) => Err(error(
                REFUSED,
                format!("the transaction carries no ML-DSA account signature. {USE_WALLET}"),
            )),
            Err(Refusal::NotContract) => Err(error(
                REFUSED,
                "only contract calls and EVM deployments are accepted here",
            )),
        }
    }

    /// The Ethereum view of block `number` (hash, gas limit, author…), with our transactions.
    pub async fn block(&self, number: u64, hash: H256, full: bool) -> RpcResult<Value> {
        let mut block = self.chain.eth_block(hash).await.map_err(internal)?;
        let txs = self.contract_txs_of(number, hash).await?;
        block.transactions = HashesOrTransactionInfos::Hashes(txs.iter().map(|t| t.0).collect());
        let mut value = to_value(&block)?;
        if full {
            let gas_price = self.chain.gas_price(hash).await.map_err(internal)?;
            let objects = txs
                .iter()
                .map(|(tx_hash, index, tx)| {
                    let at = Position {
                        number,
                        block_hash: block.hash,
                        index: *index,
                    };
                    tx_object(*tx_hash, tx, at, gas_price)
                })
                .collect::<Vec<_>>();
            value["transactions"] = Value::Array(objects);
        }
        Ok(value)
    }

    /// Contract transactions of block `number`: (hash, extrinsic index, decoded).
    async fn contract_txs_of(
        &self,
        number: u64,
        hash: H256,
    ) -> RpcResult<Vec<(H256, u32, ContractTx)>> {
        let _ = number;
        let extrinsics = self
            .chain
            .node()
            .block_extrinsics(hash)
            .await
            .map_err(internal)?
            .unwrap_or_default();
        Ok(extrinsics
            .iter()
            .enumerate()
            .filter_map(|(i, xt)| {
                let tx = decode_contract_tx(xt).ok()?;
                Some((crate::chain::extrinsic_hash(xt), u32::try_from(i).ok()?, tx))
            })
            .collect())
    }

    async fn located(&self, tx_hash: H256) -> Option<TxLocation> {
        self.index.read().await.tx(&tx_hash)
    }

    /// `eth_getTransactionByHash`.
    pub async fn transaction(&self, tx_hash: H256) -> RpcResult<Option<Value>> {
        let Some(at) = self.located(tx_hash).await else {
            return Ok(None);
        };
        let Some((_, _, tx)) = self
            .contract_txs_of(at.number, at.block)
            .await?
            .into_iter()
            .find(|(h, _, _)| *h == tx_hash)
        else {
            return Ok(None);
        };
        let eth_hash = self.chain.eth_block(at.block).await.map_err(internal)?.hash;
        let gas_price = self.chain.gas_price(at.block).await.map_err(internal)?;
        let position = Position {
            number: at.number,
            block_hash: eth_hash,
            index: at.index,
        };
        Ok(Some(tx_object(tx_hash, &tx, position, gas_price)))
    }

    /// `eth_getTransactionReceipt`.
    pub async fn receipt(&self, tx_hash: H256) -> RpcResult<Option<ReceiptInfo>> {
        let Some(at) = self.located(tx_hash).await else {
            return Ok(None);
        };
        let Some((_, _, tx)) = self
            .contract_txs_of(at.number, at.block)
            .await?
            .into_iter()
            .find(|(h, _, _)| *h == tx_hash)
        else {
            return Ok(None);
        };
        let events = self.chain.events(at.block).await.map_err(internal)?;
        let eth_hash = self.chain.eth_block(at.block).await.map_err(internal)?.hash;
        let gas_price = self.chain.gas_price(at.block).await.map_err(internal)?;
        let hashes = self.tx_hashes(at.block).await?;
        Ok(Some(build_receipt(ReceiptInput {
            tx_hash,
            tx: &tx,
            number: at.number,
            eth_hash,
            index: at.index,
            events: &events,
            gas_price,
            hashes: &hashes,
        })))
    }

    /// Hashes of every extrinsic of block `hash`, by index.
    async fn tx_hashes(&self, hash: H256) -> RpcResult<Vec<H256>> {
        Ok(self
            .chain
            .node()
            .block_extrinsics(hash)
            .await
            .map_err(internal)?
            .unwrap_or_default()
            .iter()
            .map(|xt| crate::chain::extrinsic_hash(xt))
            .collect())
    }

    /// `eth_getLogs`.
    pub async fn logs(&self, filter: LogFilter) -> RpcResult<Vec<Log>> {
        let (from, to) = match filter.block_hash {
            Some(hash) => {
                let (n, _) = self
                    .resolve(&BlockNumberOrTagOrHash::BlockHash(hash))
                    .await?;
                (n, n)
            }
            None => (
                self.resolve_number_or_tag(filter.from_block).await?,
                self.resolve_number_or_tag(filter.to_block).await?,
            ),
        };
        if to < from {
            return Ok(Vec::new());
        }
        if to.saturating_sub(from) >= self.limits.max_log_range {
            return Err(error(
                LIMIT_EXCEEDED,
                format!(
                    "block range too large: at most {} blocks per query",
                    self.limits.max_log_range
                ),
            ));
        }
        let mut logs = Vec::new();
        for number in from..=to {
            let hash = self.chain.hash_at(number).await.map_err(internal)?;
            let events = self.chain.events(hash).await.map_err(internal)?;
            let eth_hash = self.chain.eth_block(hash).await.map_err(internal)?.hash;
            let hashes = self.tx_hashes(hash).await?;
            logs.extend(
                block_logs(&events, number, eth_hash, &hashes)
                    .into_iter()
                    .filter(|log| matches(&filter, log)),
            );
        }
        Ok(logs)
    }
}

/// Where a transaction sits, as Ethereum tools see it.
#[derive(Clone, Copy, Debug)]
struct Position {
    number: u64,
    /// Ethereum block hash.
    block_hash: H256,
    index: u32,
}

/// An Ethereum-style transaction object for a contract transaction. `v`, `r` and `s` are zero:
/// the transaction is authorized by an ML-DSA signature, not an ECDSA one.
fn tx_object(hash: H256, tx: &ContractTx, at: Position, gas_price: U256) -> Value {
    let (to, value, input, weight) = match &tx.call {
        ContractTransaction::Call(c) => (
            Some(H160::from(c.dest)),
            c.value,
            c.data.clone(),
            c.weight_limit,
        ),
        ContractTransaction::Deploy(d) => (None, d.value, d.code.clone(), d.weight_limit),
        _ => (None, 0, Vec::new(), Default::default()),
    };
    json!({
        "hash": hash,
        "nonce": hex_u64(u64::from(tx.nonce)),
        "blockHash": at.block_hash,
        "blockNumber": hex_u64(at.number),
        "transactionIndex": hex_u64(u64::from(at.index)),
        "from": address_of(&tx.from),
        "to": to,
        "value": U256::from(value),
        "gas": U256::from(gas_of(weight.ref_time)),
        "gasPrice": gas_price,
        "input": hex_bytes(&input),
        "type": "0x0",
        "chainId": hex_u64(EVM_CHAIN_ID),
        "v": "0x0",
        "r": "0x0",
        "s": "0x0",
    })
}

/// Everything a receipt is built from.
pub struct ReceiptInput<'a> {
    /// Transaction hash.
    pub tx_hash: H256,
    /// The decoded transaction.
    pub tx: &'a ContractTx,
    /// Block number.
    pub number: u64,
    /// Ethereum block hash.
    pub eth_hash: H256,
    /// Extrinsic index.
    pub index: u32,
    /// Events of the block.
    pub events: &'a crate::chain::Events,
    /// Gas price of the block.
    pub gas_price: U256,
    /// Hashes of the block's extrinsics, by index.
    pub hashes: &'a [H256],
}

/// Builds a receipt from the events of the transaction's block.
pub fn build_receipt(input: ReceiptInput<'_>) -> ReceiptInfo {
    let from = address_of(&input.tx.from);
    let phase = frame_system::Phase::ApplyExtrinsic(input.index);
    let mine = || input.events.iter().filter(|r| r.phase == phase);
    let success = mine().any(|r| {
        matches!(
            r.event,
            RuntimeEvent::System(frame_system::Event::ExtrinsicSuccess { .. })
        )
    });
    let fee = mine()
        .find_map(|r| match &r.event {
            RuntimeEvent::TransactionPayment(
                pallet_transaction_payment::Event::TransactionFeePaid { actual_fee, .. },
            ) => Some(*actual_fee),
            _ => None,
        })
        .unwrap_or_default();
    let (to, contract_address) = match &input.tx.call {
        ContractTransaction::Call(c) => (Some(H160::from(c.dest)), None),
        _ => (
            None,
            mine().find_map(|r| match &r.event {
                RuntimeEvent::Revive(pallet_revive::Event::Instantiated { deployer, contract })
                    if *deployer == from =>
                {
                    Some(*contract)
                }
                _ => None,
            }),
        ),
    };
    let logs = block_logs(input.events, input.number, input.eth_hash, input.hashes)
        .into_iter()
        .filter(|log| log.transaction_hash == input.tx_hash)
        .collect();
    let gas_used = gas_for_fee(fee, input.gas_price);
    ReceiptInfo {
        block_hash: input.eth_hash,
        block_number: U256::from(input.number),
        contract_address,
        cumulative_gas_used: gas_used,
        effective_gas_price: input.gas_price,
        from,
        gas_used,
        logs,
        status: Some(U256::from(u8::from(success))),
        to,
        transaction_hash: input.tx_hash,
        transaction_index: U256::from(input.index),
        r#type: Some(pallet_revive::evm::Byte(0)),
        ..Default::default()
    }
}

/// The logs (`ContractEmitted` events) of a block, numbered in execution order.
pub fn block_logs(
    events: &crate::chain::Events,
    number: u64,
    eth_hash: H256,
    hashes: &[H256],
) -> Vec<Log> {
    let mut logs = Vec::new();
    for record in events {
        let frame_system::Phase::ApplyExtrinsic(index) = record.phase else {
            continue;
        };
        if let RuntimeEvent::Revive(pallet_revive::Event::ContractEmitted {
            contract,
            data,
            topics,
        }) = &record.event
        {
            let transaction_hash = usize::try_from(index)
                .ok()
                .and_then(|i| hashes.get(i))
                .copied()
                .unwrap_or_default();
            logs.push(Log {
                address: *contract,
                block_hash: eth_hash,
                block_number: U256::from(number),
                data: Some(Bytes(data.clone())),
                log_index: U256::from(logs.len()),
                removed: false,
                topics: topics.clone(),
                transaction_hash,
                transaction_index: U256::from(index),
            });
        }
    }
    logs
}

/// An `eth_getLogs` filter. Unlike revive's `Filter`, a topic position may be `null` (any topic).
#[derive(Clone, Debug, Default, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LogFilter {
    /// Contract address or addresses (none: every contract).
    pub address: Option<AddressOrAddresses>,
    /// First block of the range (default `latest`).
    pub from_block: Option<BlockNumberOrTag>,
    /// Last block of the range (default `latest`).
    pub to_block: Option<BlockNumberOrTag>,
    /// A single block instead of a range.
    pub block_hash: Option<H256>,
    /// Topics by position: `null` (any), one topic, or a list of alternatives.
    pub topics: Option<Vec<Option<FilterTopic>>>,
}

/// Whether `log` passes the address and topic filters.
pub fn matches(filter: &LogFilter, log: &Log) -> bool {
    let address_ok = match &filter.address {
        None => true,
        Some(AddressOrAddresses::Address(a)) => *a == log.address,
        Some(AddressOrAddresses::Addresses(list)) => list.is_empty() || list.contains(&log.address),
    };
    let topics_ok = filter.topics.as_ref().is_none_or(|topics| {
        topics.iter().enumerate().all(|(i, wanted)| match wanted {
            // `null`: any topic (or none) at this position.
            None => true,
            Some(FilterTopic::Single(t)) => log.topics.get(i) == Some(t),
            Some(FilterTopic::Multiple(options)) => {
                options.is_empty() || log.topics.get(i).is_some_and(|t| options.contains(t))
            }
        })
    });
    address_ok && topics_ok
}

/// Methods refused with a pointer to the wallet: nothing here signs for anyone.
pub const SIGNING_METHODS: &[&str] = &[
    "eth_sendTransaction",
    "eth_sign",
    "eth_signTransaction",
    "eth_signTypedData",
    "eth_signTypedData_v3",
    "eth_signTypedData_v4",
    "personal_sign",
    "personal_sendTransaction",
    "personal_unlockAccount",
    "personal_newAccount",
    "personal_listAccounts",
    "personal_ecRecover",
];

/// Wraps a method: logs only its name, duration and outcome class (never parameters or
/// results; spec evm/eth-rpc "适配器的边界").
async fn timed<T>(
    name: &'static str,
    fut: impl core::future::Future<Output = RpcResult<T>>,
) -> RpcResult<T> {
    let start = Instant::now();
    let result = fut.await;
    let ms = start.elapsed().as_millis();
    match &result {
        Ok(_) => log::debug!(target: "eth-rpc", "{name} ok {ms}ms"),
        Err(e) => log::debug!(target: "eth-rpc", "{name} error {} {ms}ms", e.code()),
    }
    result
}

/// Parses positional parameters into a tuple of `arity` elements. Trailing optional parameters
/// may be omitted: missing positions read as `null`.
fn params<T: serde::de::DeserializeOwned>(p: &Params<'_>, arity: usize) -> RpcResult<T> {
    let invalid = || error(INVALID_PARAMS, "invalid parameters");
    let mut list: Vec<Value> = match p.as_str() {
        None => Vec::new(),
        Some(raw) => serde_json::from_str(raw).map_err(|_| invalid())?,
    };
    if list.len() > arity {
        return Err(invalid());
    }
    list.resize(arity, Value::Null);
    serde_json::from_value(Value::Array(list)).map_err(|_| invalid())
}

macro_rules! method {
    ($module:ident, $name:literal, |$ctx:ident, $p:ident| $body:expr) => {
        $module
            .register_async_method($name, |$p, $ctx, _| async move {
                timed($name, async move { $body }).await
            })
            .map_err(|e| anyhow!("register {}: {e}", $name))?;
    };
}

/// Builds the RPC module.
///
/// # Errors
///
/// Only if a method were registered twice.
pub fn module<N: Node>(ctx: Arc<Ctx<N>>) -> anyhow::Result<RpcModule<Ctx<N>>> {
    let mut m = RpcModule::from_arc(ctx);
    method!(
        m,
        "web3_clientVersion",
        |_ctx, _p| Ok::<_, ErrorObjectOwned>(format!("ac-eth-rpc/{}", env!("CARGO_PKG_VERSION")))
    );
    method!(m, "net_version", |_ctx, _p| Ok::<_, ErrorObjectOwned>(
        EVM_CHAIN_ID.to_string()
    ));
    method!(m, "eth_chainId", |_ctx, _p| Ok::<_, ErrorObjectOwned>(
        hex_u64(EVM_CHAIN_ID)
    ));
    method!(m, "eth_syncing", |_ctx, _p| Ok::<_, ErrorObjectOwned>(
        false
    ));
    method!(m, "eth_accounts", |_ctx, _p| Ok::<_, ErrorObjectOwned>(
        Vec::<H160>::new()
    ));
    method!(m, "eth_blockNumber", |ctx, _p| ctx
        .best()
        .await
        .map(|(n, _)| hex_u64(n)));
    method!(m, "eth_gasPrice", |ctx, _p| {
        let (_, at) = ctx.best().await?;
        ctx.chain.gas_price(at).await.map_err(internal)
    });
    method!(m, "eth_maxPriorityFeePerGas", |_ctx, _p| Ok::<
        _,
        ErrorObjectOwned,
    >(U256::zero()));
    method!(m, "eth_feeHistory", |ctx, p| {
        let (count, newest, percentiles): (U256, BlockNumberOrTag, Option<Vec<f64>>) =
            params(&p, 3)?;
        let count = u64::try_from(count).unwrap_or(1024).clamp(1, 1024);
        let newest = ctx.resolve_number_or_tag(Some(newest)).await?;
        let oldest = newest.saturating_add(1).saturating_sub(count);
        let (_, at) = ctx.best().await?;
        let price = ctx.chain.gas_price(at).await.map_err(internal)?;
        let blocks = usize::try_from(newest.saturating_sub(oldest).saturating_add(1)).unwrap_or(1);
        let mut result = json!({
            "oldestBlock": hex_u64(oldest),
            "baseFeePerGas": vec![price; blocks.saturating_add(1)],
            "gasUsedRatio": vec![0.0; blocks],
        });
        if let Some(p) = percentiles {
            result["reward"] = json!(vec![vec![U256::zero(); p.len()]; blocks]);
        }
        Ok(result)
    });
    method!(m, "eth_getBalance", |ctx, p| {
        let (address, block): (H160, Option<BlockNumberOrTagOrHash>) = params(&p, 2)?;
        let (_, at) = ctx.resolve(&block.unwrap_or_default()).await?;
        ctx.chain.balance(address, at).await.map_err(internal)
    });
    method!(m, "eth_getTransactionCount", |ctx, p| {
        let (address, block): (H160, Option<BlockNumberOrTagOrHash>) = params(&p, 2)?;
        let (_, at) = ctx.resolve(&block.unwrap_or_default()).await?;
        ctx.chain
            .nonce(address, at)
            .await
            .map(|n| hex_u64(u64::from(n)))
            .map_err(internal)
    });
    method!(m, "eth_getCode", |ctx, p| {
        let (address, block): (H160, Option<BlockNumberOrTagOrHash>) = params(&p, 2)?;
        let (_, at) = ctx.resolve(&block.unwrap_or_default()).await?;
        ctx.chain
            .code(address, at)
            .await
            .map(|c| hex_bytes(&c))
            .map_err(internal)
    });
    method!(m, "eth_getStorageAt", |ctx, p| {
        let (address, slot, block): (H160, U256, Option<BlockNumberOrTagOrHash>) = params(&p, 3)?;
        let (_, at) = ctx.resolve(&block.unwrap_or_default()).await?;
        ctx.chain
            .storage_at(address, slot.to_big_endian(), at)
            .await
            .map(|w| hex_bytes(&w))
            .map_err(internal)
    });
    method!(m, "eth_call", |ctx, p| {
        let (tx, block): (GenericTransaction, Option<BlockNumberOrTagOrHash>) = params(&p, 2)?;
        ctx.call(tx, block.unwrap_or_default()).await
    });
    method!(m, "eth_estimateGas", |ctx, p| {
        let (tx, block): (GenericTransaction, Option<BlockNumberOrTagOrHash>) = params(&p, 2)?;
        ctx.estimate_gas(tx, block).await
    });
    method!(m, "eth_getBlockByNumber", |ctx, p| {
        let (block, full): (BlockNumberOrTag, Option<bool>) = params(&p, 2)?;
        let block = match block {
            BlockNumberOrTag::BlockTag(t) => BlockNumberOrTagOrHash::BlockTag(t),
            BlockNumberOrTag::U256(n) => BlockNumberOrTagOrHash::BlockNumber(n),
        };
        match ctx.resolve(&block).await {
            Ok((n, hash)) => ctx.block(n, hash, full.unwrap_or(false)).await.map(Some),
            Err(e) if e.code() == INVALID_PARAMS => Ok(None),
            Err(e) => Err(e),
        }
    });
    method!(m, "eth_getBlockByHash", |ctx, p| {
        let (hash, full): (H256, Option<bool>) = params(&p, 2)?;
        match ctx.resolve(&BlockNumberOrTagOrHash::BlockHash(hash)).await {
            Ok((n, native)) => ctx.block(n, native, full.unwrap_or(false)).await.map(Some),
            Err(e) if e.code() == INVALID_PARAMS => Ok(None),
            Err(e) => Err(e),
        }
    });
    method!(m, "eth_getTransactionByHash", |ctx, p| {
        let (hash,): (H256,) = params(&p, 1)?;
        ctx.transaction(hash).await
    });
    method!(m, "eth_getTransactionReceipt", |ctx, p| {
        let (hash,): (H256,) = params(&p, 1)?;
        ctx.receipt(hash).await.and_then(|r| to_value(&r))
    });
    method!(m, "eth_getLogs", |ctx, p| {
        let (filter,): (LogFilter,) = params(&p, 1)?;
        ctx.logs(filter).await
    });
    method!(m, "eth_sendRawTransaction", |ctx, p| {
        let (bytes,): (Bytes,) = params(&p, 1)?;
        ctx.send_raw(bytes).await
    });
    for name in SIGNING_METHODS {
        m.register_method(name, |_, _, _| -> RpcResult<()> {
            Err(error(REFUSED, USE_WALLET))
        })
        .map_err(|e| anyhow!("register {name}: {e}"))?;
    }
    Ok(m)
}
