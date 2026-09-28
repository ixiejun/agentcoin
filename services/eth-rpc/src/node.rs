//! Access to an AgentCoin node through its public JSON-RPC (design D9 of m4-evm).
//!
//! [`Node`] is the only way the adapter touches the chain, so the Ethereum methods can be tested
//! against an in-memory node. [`RpcNode`] implements it over HTTP.

use anyhow::{Context, Result, anyhow};
use async_trait::async_trait;
use jsonrpsee::core::client::ClientT;
use jsonrpsee::http_client::{HttpClient, HttpClientBuilder};
use jsonrpsee::rpc_params;
use sp_core::H256;

/// What the adapter needs from a node.
#[async_trait]
pub trait Node: Send + Sync + 'static {
    /// Hash of the best block.
    async fn best_hash(&self) -> Result<H256>;
    /// Hash of the latest AC-BFT finalized block.
    async fn finalized_hash(&self) -> Result<H256>;
    /// Hash of the canonical block at `number`, if any.
    async fn block_hash(&self, number: u64) -> Result<Option<H256>>;
    /// Number of the block `hash`, if the node knows it.
    async fn block_number(&self, hash: H256) -> Result<Option<u64>>;
    /// The encoded extrinsics of block `hash`, if the node knows it.
    async fn block_extrinsics(&self, hash: H256) -> Result<Option<Vec<Vec<u8>>>>;
    /// Calls runtime API `method` with SCALE `args` in the state of block `at`.
    async fn state_call(&self, method: &str, args: Vec<u8>, at: H256) -> Result<Vec<u8>>;
    /// Reads storage `key` in the state of block `at`.
    async fn storage(&self, key: Vec<u8>, at: H256) -> Result<Option<Vec<u8>>>;
    /// Submits an encoded extrinsic and returns the hash the node assigns it.
    async fn submit(&self, extrinsic: Vec<u8>) -> Result<H256>;
}

/// A node reached over HTTP JSON-RPC.
pub struct RpcNode {
    rpc: HttpClient,
}

fn unhex(text: &str) -> Result<Vec<u8>> {
    Ok(hex::decode(text.trim_start_matches("0x"))?)
}

fn hash_of(text: &str) -> Result<H256> {
    let bytes = unhex(text)?;
    if bytes.len() != 32 {
        return Err(anyhow!("expected a 32-byte hash"));
    }
    Ok(H256::from_slice(&bytes))
}

impl RpcNode {
    /// Connects to `url` (for example `http://127.0.0.1:9944`).
    ///
    /// # Errors
    ///
    /// An invalid URL.
    pub fn new(url: &str) -> Result<Self> {
        Ok(Self {
            rpc: HttpClientBuilder::default()
                .max_response_size(64 * 1024 * 1024)
                .build(url)?,
        })
    }
}

#[async_trait]
impl Node for RpcNode {
    async fn best_hash(&self) -> Result<H256> {
        let hash: String = self
            .rpc
            .request("chain_getBlockHash", rpc_params![])
            .await?;
        hash_of(&hash)
    }

    async fn finalized_hash(&self) -> Result<H256> {
        let hash: String = self
            .rpc
            .request("chain_getFinalizedHead", rpc_params![])
            .await?;
        hash_of(&hash)
    }

    async fn block_hash(&self, number: u64) -> Result<Option<H256>> {
        let hash: Option<String> = self
            .rpc
            .request("chain_getBlockHash", rpc_params![number])
            .await?;
        hash.map(|h| hash_of(&h)).transpose()
    }

    async fn block_number(&self, hash: H256) -> Result<Option<u64>> {
        let header: Option<serde_json::Value> = self
            .rpc
            .request("chain_getHeader", rpc_params![hash])
            .await?;
        let Some(header) = header else {
            return Ok(None);
        };
        let number = header["number"]
            .as_str()
            .ok_or_else(|| anyhow!("header without a number"))?;
        Ok(Some(u64::from_str_radix(
            number.trim_start_matches("0x"),
            16,
        )?))
    }

    async fn block_extrinsics(&self, hash: H256) -> Result<Option<Vec<Vec<u8>>>> {
        let block: Option<serde_json::Value> = self
            .rpc
            .request("chain_getBlock", rpc_params![hash])
            .await?;
        let Some(block) = block else {
            return Ok(None);
        };
        let extrinsics = block["block"]["extrinsics"]
            .as_array()
            .ok_or_else(|| anyhow!("block without extrinsics"))?;
        extrinsics
            .iter()
            .map(|x| unhex(x.as_str().ok_or_else(|| anyhow!("extrinsic is not hex"))?))
            .collect::<Result<Vec<_>>>()
            .map(Some)
    }

    async fn state_call(&self, method: &str, args: Vec<u8>, at: H256) -> Result<Vec<u8>> {
        let hex_args = format!("0x{}", hex::encode(args));
        let result: String = self
            .rpc
            .request("state_call", rpc_params![method, hex_args, at])
            .await
            .with_context(|| format!("state_call {method}"))?;
        unhex(&result)
    }

    async fn storage(&self, key: Vec<u8>, at: H256) -> Result<Option<Vec<u8>>> {
        let key = format!("0x{}", hex::encode(key));
        let value: Option<String> = self
            .rpc
            .request("state_getStorage", rpc_params![key, at])
            .await?;
        value.map(|v| unhex(&v)).transpose()
    }

    async fn submit(&self, extrinsic: Vec<u8>) -> Result<H256> {
        let hex_xt = format!("0x{}", hex::encode(extrinsic));
        let hash: String = self
            .rpc
            .request("author_submitExtrinsic", rpc_params![hex_xt])
            .await?;
        hash_of(&hash)
    }
}
