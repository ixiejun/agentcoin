//! Node RPC client: chain facts, account state, submission and inclusion tracking.
//!
//! State is read with `state_call` (runtime APIs) and `state_getStorage` on SCALE-encoded keys,
//! so no address format other than `atc1…` is involved.

use std::time::{Duration, Instant};

use ac_crypto::PqPublicKey;
use ac_runtime::transaction::ChainContext;
use ac_runtime::{Hash, Runtime, RuntimeEvent, UncheckedExtrinsic};
use anyhow::{Context, Result, anyhow, bail};
use jsonrpsee::core::client::ClientT;
use jsonrpsee::http_client::{HttpClient, HttpClientBuilder};
use jsonrpsee::rpc_params;
use parity_scale_codec::{Decode, Encode};
use sp_core::H256;
use sp_runtime::AccountId32;

/// Result of a submitted transaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Inclusion {
    /// Hash of the block that contains the transaction.
    pub block_hash: H256,
    /// Position of the transaction in that block.
    pub index: u32,
    /// Whether the call dispatched successfully (the fee is charged either way).
    pub success: bool,
}

/// The BLAKE3-256 hash of an encoded transaction: the hash the node reports for it.
#[must_use]
pub fn transaction_hash(xt: &UncheckedExtrinsic) -> H256 {
    <ac_primitives::Blake3Hasher as sp_runtime::traits::Hash>::hash(&xt.encode())
}

/// Result of a contract dry run.
pub type DryCall = pallet_revive::ContractResult<pallet_revive::ExecReturnValue, u128>;
/// Result of a deployment dry run.
pub type DryDeploy = pallet_revive::ContractResult<pallet_revive::InstantiateReturnValue, u128>;

/// A JSON-RPC connection to a node (cheap to clone; clones share the connection pool).
#[derive(Clone)]
pub struct NodeClient {
    rpc: HttpClient,
}

fn unhex(text: &str) -> Result<Vec<u8>> {
    Ok(hex::decode(text.trim_start_matches("0x"))?)
}

impl NodeClient {
    /// Connects to `url` (for example `http://127.0.0.1:9944`).
    ///
    /// # Errors
    ///
    /// Invalid URLs.
    pub fn new(url: &str) -> Result<Self> {
        Ok(Self {
            rpc: HttpClientBuilder::default().build(url)?,
        })
    }

    async fn state_call(&self, method: &str, args: &impl Encode) -> Result<Vec<u8>> {
        let hex_args = format!("0x{}", hex::encode(args.encode()));
        let result: String = self
            .rpc
            .request("state_call", rpc_params![method, hex_args])
            .await
            .with_context(|| format!("state_call {method}"))?;
        unhex(&result)
    }

    /// Calls runtime API `method` (e.g. `MarketApi_model`) with SCALE-encoded `args` on the
    /// latest state and returns the encoded result.
    ///
    /// # Errors
    ///
    /// RPC failures.
    pub async fn call_api(&self, method: &str, args: &impl Encode) -> Result<Vec<u8>> {
        self.state_call(method, args).await
    }

    /// Name of the error a failed transaction returned (e.g. `Providers(BelowThreshold)`), from
    /// its `ExtrinsicFailed` event; `None` if it succeeded.
    ///
    /// # Errors
    ///
    /// RPC failures or undecodable events.
    pub async fn dispatch_error(&self, block: Hash, index: u32) -> Result<Option<String>> {
        for event in self.extrinsic_events(block, index).await? {
            if let RuntimeEvent::System(frame_system::Event::ExtrinsicFailed {
                dispatch_error,
                ..
            }) = event
            {
                return Ok(Some(
                    ac_runtime::RuntimeError::from_dispatch_error(dispatch_error)
                        .map_or_else(|| format!("{dispatch_error:?}"), |e| format!("{e:?}")),
                ));
            }
        }
        Ok(None)
    }

    async fn storage(&self, key: &[u8], at: Option<H256>) -> Result<Option<Vec<u8>>> {
        let key = format!("0x{}", hex::encode(key));
        let value: Option<String> = self
            .rpc
            .request("state_getStorage", rpc_params![key, at])
            .await?;
        value.map(|v| unhex(&v)).transpose()
    }

    /// Genesis hash and runtime versions, after checking the chain hashes with BLAKE3.
    ///
    /// # Errors
    ///
    /// RPC failures or a chain with a different hashing profile.
    pub async fn chain_context(&self) -> Result<ChainContext> {
        let profile = ac_primitives::ChainProfile::decode(
            &mut &self.state_call("ChainProfileApi_profile", &()).await?[..],
        )?;
        if profile != ac_primitives::ChainProfile::AGENTCOIN {
            bail!("the node is not an AgentCoin chain (unexpected chain profile)");
        }
        let genesis: String = self
            .rpc
            .request("chain_getBlockHash", rpc_params![0u32])
            .await?;
        let version: serde_json::Value = self
            .rpc
            .request("state_getRuntimeVersion", rpc_params![])
            .await?;
        let field = |name: &str| -> Result<u32> {
            u32::try_from(
                version[name]
                    .as_u64()
                    .ok_or_else(|| anyhow!("missing {name}"))?,
            )
            .context(name.to_string())
        };
        Ok(ChainContext {
            genesis_hash: H256::from_slice(&unhex(&genesis)?),
            spec_version: field("specVersion")?,
            transaction_version: field("transactionVersion")?,
        })
    }

    /// Next nonce of `who` (from the latest state; the pool is not consulted).
    ///
    /// # Errors
    ///
    /// RPC failures.
    pub async fn nonce(&self, who: &AccountId32) -> Result<u32> {
        let raw = self
            .state_call("AccountNonceApi_account_nonce", who)
            .await?;
        Ok(u32::decode(&mut &raw[..])?)
    }

    /// Registered key and rotation count of `who`, or `None` before its first transaction.
    ///
    /// # Errors
    ///
    /// RPC failures.
    pub async fn current_key(&self, who: &AccountId32) -> Result<Option<(PqPublicKey, u32)>> {
        let raw = self.state_call("PqAccountsApi_current_key", who).await?;
        Ok(Option::<(PqPublicKey, u32)>::decode(&mut &raw[..])?)
    }

    /// Free balance of `who` in smallest units.
    ///
    /// # Errors
    ///
    /// RPC failures.
    pub async fn free_balance(&self, who: &AccountId32) -> Result<u128> {
        let key = frame_system::Account::<Runtime>::hashed_key_for(who);
        Ok(match self.storage(&key, None).await? {
            Some(raw) => {
                frame_system::AccountInfo::<u32, pallet_balances::AccountData<u128>>::decode(
                    &mut &raw[..],
                )?
                .data
                .free
            }
            None => 0,
        })
    }

    /// Storage keys under `prefix`, at most `count`, after `start` (latest state).
    ///
    /// # Errors
    ///
    /// RPC failures.
    pub async fn storage_keys_paged(
        &self,
        prefix: &[u8],
        count: u32,
        start: Option<&[u8]>,
    ) -> Result<Vec<Vec<u8>>> {
        let prefix = format!("0x{}", hex::encode(prefix));
        let start = start.map(|s| format!("0x{}", hex::encode(s)));
        let keys: Vec<String> = self
            .rpc
            .request("state_getKeysPaged", rpc_params![prefix, count, start])
            .await?;
        keys.iter().map(|k| unhex(k)).collect()
    }

    /// Number of the best block.
    ///
    /// # Errors
    ///
    /// RPC failures.
    pub async fn best_block(&self) -> Result<u64> {
        self.best_number().await
    }

    /// The providers pallet's parameters (thresholds, heartbeat interval, unbonding).
    ///
    /// # Errors
    ///
    /// RPC failures or an undecodable value.
    pub async fn provider_params(&self) -> Result<pallet_providers::ProviderParams> {
        let key = pallet_providers::Params::<Runtime>::hashed_key();
        match self.storage(&key, None).await? {
            Some(raw) => Ok(pallet_providers::ProviderParams::decode(&mut &raw[..])?),
            None => Ok(pallet_providers::ProviderParams::default()),
        }
    }

    /// Emission epoch length in blocks (published key `Emission::EpochLength`).
    ///
    /// # Errors
    ///
    /// RPC failures, or a chain without the key.
    pub async fn epoch_length(&self) -> Result<u64> {
        let key = [
            sp_io::hashing::twox_128(b"Emission"),
            sp_io::hashing::twox_128(b"EpochLength"),
        ]
        .concat();
        let raw = self
            .storage(&key, None)
            .await?
            .context("Emission::EpochLength is missing")?;
        Ok(u64::decode(&mut &raw[..])?)
    }

    async fn best_number(&self) -> Result<u64> {
        let header: serde_json::Value = self.rpc.request("chain_getHeader", rpc_params![]).await?;
        let number = header["number"]
            .as_str()
            .ok_or_else(|| anyhow!("bad header"))?;
        Ok(u64::from_str_radix(number.trim_start_matches("0x"), 16)?)
    }

    /// Submits `xt` and waits until it is included in a block (or `timeout` passes).
    ///
    /// # Errors
    ///
    /// Rejection by the node (invalid transaction), RPC failures or the timeout.
    pub async fn submit_and_watch(
        &self,
        xt: &UncheckedExtrinsic,
        timeout: Duration,
    ) -> Result<Inclusion> {
        let bytes = xt.encode();
        let hex_xt = format!("0x{}", hex::encode(&bytes));
        let mut next = self.best_number().await?.saturating_add(1);
        let _hash: String = self
            .rpc
            .request("author_submitExtrinsic", rpc_params![hex_xt.clone()])
            .await
            .context("the node rejected the transaction")?;
        let start = Instant::now();
        while start.elapsed() < timeout {
            let best = self.best_number().await?;
            while next <= best {
                let hash: String = self
                    .rpc
                    .request("chain_getBlockHash", rpc_params![next])
                    .await?;
                let block: serde_json::Value = self
                    .rpc
                    .request("chain_getBlock", rpc_params![hash.clone()])
                    .await?;
                let extrinsics = block["block"]["extrinsics"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default();
                if let Some(index) = extrinsics.iter().position(|x| x.as_str() == Some(&hex_xt)) {
                    let block_hash = H256::from_slice(&unhex(&hash)?);
                    let index = u32::try_from(index)?;
                    let success = self.dispatch_succeeded(block_hash, index).await?;
                    return Ok(Inclusion {
                        block_hash,
                        index,
                        success,
                    });
                }
                next = next.saturating_add(1);
            }
            tokio::time::sleep(Duration::from_millis(300)).await;
        }
        bail!("transaction not included within {timeout:?}")
    }

    /// Events emitted by the transaction at `index` of `block`, in order.
    ///
    /// # Errors
    ///
    /// RPC failures or undecodable events.
    pub async fn extrinsic_events(&self, block: Hash, index: u32) -> Result<Vec<RuntimeEvent>> {
        // `System::Events` is a well-known storage value: twox128("System") ‖ twox128("Events").
        let key = [
            sp_io::hashing::twox_128(b"System"),
            sp_io::hashing::twox_128(b"Events"),
        ]
        .concat();
        // Absent (not empty) when a block has no events.
        let Some(raw) = self.storage(&key, Some(block)).await? else {
            return Ok(Vec::new());
        };
        let events = Vec::<frame_system::EventRecord<RuntimeEvent, Hash>>::decode(&mut &raw[..])?;
        Ok(events
            .into_iter()
            .filter(|r| r.phase == frame_system::Phase::ApplyExtrinsic(index))
            .map(|r| r.event)
            .collect())
    }

    async fn dispatch_succeeded(&self, block: Hash, index: u32) -> Result<bool> {
        for event in self.extrinsic_events(block, index).await? {
            match event {
                RuntimeEvent::System(frame_system::Event::ExtrinsicSuccess { .. }) => {
                    return Ok(true);
                }
                RuntimeEvent::System(frame_system::Event::ExtrinsicFailed { .. }) => {
                    return Ok(false);
                }
                _ => {}
            }
        }
        bail!("no dispatch outcome recorded for the transaction")
    }

    /// Dry run of a contract call from `origin` on the latest state (nothing is submitted).
    ///
    /// # Errors
    ///
    /// RPC failures.
    pub async fn dry_call(
        &self,
        origin: &AccountId32,
        dest: pallet_revive::H160,
        value: u128,
        data: Vec<u8>,
    ) -> Result<DryCall> {
        let args = (
            origin,
            dest,
            value,
            Option::<sp_runtime::Weight>::None,
            Option::<u128>::None,
            data,
        );
        let raw = self.state_call("ReviveApi_call", &args).await?;
        Ok(DryCall::decode(&mut &raw[..])?)
    }

    /// Dry run of an EVM deployment of `init_code` (constructor arguments appended) from
    /// `origin` on the latest state.
    ///
    /// # Errors
    ///
    /// RPC failures.
    pub async fn dry_deploy(
        &self,
        origin: &AccountId32,
        value: u128,
        init_code: Vec<u8>,
    ) -> Result<DryDeploy> {
        let args = (
            origin,
            value,
            Option::<sp_runtime::Weight>::None,
            Option::<u128>::None,
            pallet_revive::Code::Upload(init_code),
            Vec::<u8>::new(),
            Option::<[u8; 32]>::None,
        );
        let raw = self.state_call("ReviveApi_instantiate", &args).await?;
        Ok(DryDeploy::decode(&mut &raw[..])?)
    }

    /// Code at EVM address `address` on the latest state (empty for accounts).
    ///
    /// # Errors
    ///
    /// RPC failures.
    pub async fn evm_code(&self, address: pallet_revive::H160) -> Result<Vec<u8>> {
        let raw = self.state_call("ReviveApi_code", &address).await?;
        Ok(Vec::<u8>::decode(&mut &raw[..])?)
    }

    /// Submits `xt` without waiting and returns the hash the node assigns it.
    ///
    /// # Errors
    ///
    /// Rejection by the node or RPC failures.
    pub async fn submit(&self, xt: &UncheckedExtrinsic) -> Result<H256> {
        let hex_xt = format!("0x{}", hex::encode(xt.encode()));
        let hash: String = self
            .rpc
            .request("author_submitExtrinsic", rpc_params![hex_xt])
            .await
            .context("the node rejected the transaction")?;
        Ok(H256::from_slice(&unhex(&hash)?))
    }
}
