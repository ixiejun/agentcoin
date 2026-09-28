//! In-memory index of recent blocks (design D9): native and Ethereum block hashes, and where each
//! contract transaction sits. Nothing is persisted; a restart rebuilds it.

use std::collections::{BTreeMap, HashMap};

use anyhow::Result;
use sp_core::H256;

use crate::chain::{Chain, decode_contract_tx, extrinsic_hash};
use crate::node::Node;

/// One indexed block.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IndexedBlock {
    /// Native (BLAKE3) block hash.
    pub hash: H256,
    /// Ethereum block hash as seen by `BLOCKHASH`.
    pub eth_hash: H256,
    /// Hashes of the contract transactions, with their extrinsic index.
    pub contract_txs: Vec<(u32, H256)>,
}

/// Where a transaction is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TxLocation {
    /// Block number.
    pub number: u64,
    /// Native block hash.
    pub block: H256,
    /// Extrinsic index in the block.
    pub index: u32,
}

/// The index.
#[derive(Default, Debug)]
pub struct Index {
    blocks: BTreeMap<u64, IndexedBlock>,
    by_hash: HashMap<H256, u64>,
    txs: HashMap<H256, TxLocation>,
}

impl Index {
    /// Highest indexed block number.
    pub fn tip(&self) -> Option<u64> {
        self.blocks.keys().next_back().copied()
    }

    /// Lowest indexed block number.
    pub fn first(&self) -> Option<u64> {
        self.blocks.keys().next().copied()
    }

    /// The block at `number`.
    pub fn block(&self, number: u64) -> Option<&IndexedBlock> {
        self.blocks.get(&number)
    }

    /// Number of the block with native or Ethereum hash `hash`.
    pub fn number_of(&self, hash: &H256) -> Option<u64> {
        self.by_hash.get(hash).copied()
    }

    /// Location of transaction `hash`.
    pub fn tx(&self, hash: &H256) -> Option<TxLocation> {
        self.txs.get(hash).copied()
    }

    /// Adds (or replaces) block `number`.
    pub fn insert(&mut self, number: u64, block: IndexedBlock) {
        self.remove_from(number);
        self.by_hash.insert(block.hash, number);
        self.by_hash.insert(block.eth_hash, number);
        for (index, tx) in &block.contract_txs {
            self.txs.insert(
                *tx,
                TxLocation {
                    number,
                    block: block.hash,
                    index: *index,
                },
            );
        }
        self.blocks.insert(number, block);
    }

    /// Drops blocks `number` and above (after a reorganisation).
    pub fn remove_from(&mut self, number: u64) {
        let dropped = self.blocks.split_off(&number);
        for block in dropped.values() {
            self.by_hash.remove(&block.hash);
            self.by_hash.remove(&block.eth_hash);
            for (_, tx) in &block.contract_txs {
                self.txs.remove(tx);
            }
        }
    }
}

/// Reads block `number` from the node for the index.
///
/// # Errors
///
/// Node failures, or a block the node does not know.
pub async fn fetch_block<N: Node>(chain: &Chain<N>, number: u64) -> Result<IndexedBlock> {
    let hash = chain.hash_at(number).await?;
    let extrinsics = chain
        .node()
        .block_extrinsics(hash)
        .await?
        .unwrap_or_default();
    let contract_txs = extrinsics
        .iter()
        .enumerate()
        .filter(|(_, xt)| decode_contract_tx(xt).is_ok())
        .filter_map(|(i, xt)| u32::try_from(i).ok().map(|i| (i, extrinsic_hash(xt))))
        .collect();
    let eth_hash = chain.eth_block(hash).await?.hash;
    Ok(IndexedBlock {
        hash,
        eth_hash,
        contract_txs,
    })
}

/// Brings `index` up to the node's best block: backfills `depth` blocks at start, then follows
/// every new block (nothing seen after start is dropped, spec evm/eth-rpc "交易与回执"), and
/// rewinds when the canonical chain changed under it.
///
/// # Errors
///
/// Node failures (the index keeps what it had).
pub async fn catch_up<N: Node>(
    chain: &Chain<N>,
    index: &tokio::sync::RwLock<Index>,
    depth: u64,
) -> Result<()> {
    let best_hash = chain.node().best_hash().await?;
    let best = chain
        .node()
        .block_number(best_hash)
        .await?
        .unwrap_or_default();
    let (tip, first) = {
        let index = index.read().await;
        (index.tip(), index.first())
    };
    // Rewind past blocks that are no longer canonical.
    let mut next = match tip {
        None => best.saturating_sub(depth),
        Some(tip) => {
            let mut n = tip.min(best);
            loop {
                let known = index.read().await.block(n).map(|b| b.hash);
                let canonical = chain.node().block_hash(n).await?;
                if known.is_some() && known == canonical {
                    break n.saturating_add(1);
                }
                if n == 0 || Some(n) == first {
                    break n;
                }
                n = n.saturating_sub(1);
            }
        }
    };
    if tip.is_some_and(|tip| next <= tip) {
        index.write().await.remove_from(next);
    }
    while next <= best {
        let block = fetch_block(chain, next).await?;
        index.write().await.insert(next, block);
        next = next.saturating_add(1);
    }
    Ok(())
}
