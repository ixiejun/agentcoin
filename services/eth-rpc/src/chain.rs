//! Typed views of chain data: runtime API calls, events and contract transactions.

use ac_primitives::evm::{ContractTransaction, classify_call};
use ac_runtime::{AccountId, Balance, Hash, Runtime, RuntimeEvent, UncheckedExtrinsic};
use anyhow::{Result, anyhow};
use pallet_revive::{AddressMapper, H160, U256};
use parity_scale_codec::{Decode, Encode};
use sp_core::H256;
use sp_runtime::generic::Preamble;
use sp_runtime::traits::ExtensionVariant;
use sp_runtime::traits::Hash as _;

use crate::node::Node;

/// Ref-time weight of one unit of "gas" as shown to Ethereum tools: 10,000 ps. Contract
/// transactions pay native fees; gas figures only translate weight and fees for tools.
pub const REF_TIME_PER_GAS: u64 = 10_000;

/// Event records of a block.
pub type Events = Vec<frame_system::EventRecord<RuntimeEvent, Hash>>;

/// The EVM address of an account (`keccak256(account)[12..]`, the runtime's mapping).
pub fn address_of(account: &AccountId) -> H160 {
    <Runtime as pallet_revive::Config>::AddressMapper::to_address(account)
}

/// The BLAKE3-256 hash of an encoded extrinsic: the hash the node reports for it.
pub fn extrinsic_hash(extrinsic: &[u8]) -> H256 {
    ac_primitives::Blake3Hasher::hash(extrinsic)
}

/// A decoded contract transaction.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContractTx {
    /// Signer.
    pub from: AccountId,
    /// Account nonce the transaction used.
    pub nonce: u32,
    /// The contract call or deployment.
    pub call: ContractTransaction,
}

/// Why a raw transaction was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// The bytes are not an AgentCoin extrinsic (e.g. an RLP Ethereum transaction).
    NotAgentCoin,
    /// An AgentCoin transaction without an account signature.
    Unsigned,
    /// Not a contract call or EVM deployment.
    NotContract,
}

/// Decodes `bytes` as an ML-DSA-signed contract transaction.
///
/// # Errors
///
/// A [`Refusal`] saying why the bytes are not one.
pub fn decode_contract_tx(bytes: &[u8]) -> Result<ContractTx, Refusal> {
    let xt = UncheckedExtrinsic::decode(&mut &bytes[..]).map_err(|_| Refusal::NotAgentCoin)?;
    let (who, nonce) = match &xt.preamble {
        Preamble::General(ExtensionVariant::V0(extensions)) => {
            let pallet_pq_accounts::PqAuth::Signed { who, .. } = &extensions.0.0 else {
                return Err(Refusal::Unsigned);
            };
            (who.clone(), extensions.7.0)
        }
        _ => return Err(Refusal::Unsigned),
    };
    let call = classify_call(&xt.function.encode()).map_err(|_| Refusal::NotContract)?;
    Ok(ContractTx {
        from: who,
        nonce,
        call,
    })
}

/// A read-only execution request: who runs it, the value sent, and the call data (or init code).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DryRun {
    /// Caller.
    pub origin: AccountId,
    /// Value sent, in the smallest unit.
    pub value: Balance,
    /// Call data, or EVM init code for a deployment.
    pub data: Vec<u8>,
}

/// Runtime API and storage reads, all at a given block.
pub struct Chain<N> {
    node: N,
}

impl<N: Node> Chain<N> {
    /// Wraps a node.
    pub fn new(node: N) -> Self {
        Self { node }
    }

    /// The node.
    pub fn node(&self) -> &N {
        &self.node
    }

    async fn api<R: Decode>(&self, method: &str, args: impl Encode, at: H256) -> Result<R> {
        let raw = self.node.state_call(method, args.encode(), at).await?;
        Ok(R::decode(&mut &raw[..])?)
    }

    /// Events of block `at`.
    ///
    /// # Errors
    ///
    /// Node failures or undecodable events.
    pub async fn events(&self, at: H256) -> Result<Events> {
        // `System::Events`: twox128("System") ‖ twox128("Events").
        let key = [
            sp_io::hashing::twox_128(b"System"),
            sp_io::hashing::twox_128(b"Events"),
        ]
        .concat();
        // Absent (not empty) when a block has no events, e.g. genesis.
        let Some(raw) = self.node.storage(key, at).await? else {
            return Ok(Events::new());
        };
        Ok(Events::decode(&mut &raw[..])?)
    }

    /// EVM balance (spendable balance) of `address`.
    ///
    /// # Errors
    ///
    /// Node failures.
    pub async fn balance(&self, address: H160, at: H256) -> Result<U256> {
        self.api("ReviveApi_balance", address, at).await
    }

    /// Nonce of the account behind `address`.
    ///
    /// # Errors
    ///
    /// Node failures.
    pub async fn nonce(&self, address: H160, at: H256) -> Result<u32> {
        self.api("ReviveApi_nonce", address, at).await
    }

    /// Code at `address` (empty for accounts).
    ///
    /// # Errors
    ///
    /// Node failures.
    pub async fn code(&self, address: H160, at: H256) -> Result<Vec<u8>> {
        self.api("ReviveApi_code", address, at).await
    }

    /// Storage slot `key` of `address` as a 32-byte word (zero when unset or not a contract).
    ///
    /// # Errors
    ///
    /// Node failures.
    pub async fn storage_at(&self, address: H160, key: [u8; 32], at: H256) -> Result<[u8; 32]> {
        let result: pallet_revive::GetStorageResult = self
            .api("ReviveApi_get_storage", (address, key), at)
            .await?;
        let mut word = [0u8; 32];
        if let Ok(Some(value)) = result {
            let start = 32usize.saturating_sub(value.len());
            if let (Some(dst), Some(src)) = (word.get_mut(start..), value.get(..32 - start)) {
                dst.copy_from_slice(src);
            }
        }
        Ok(word)
    }

    /// Dry run of `request` calling contract `dest`.
    ///
    /// # Errors
    ///
    /// Node failures.
    pub async fn dry_call(
        &self,
        request: DryRun,
        dest: H160,
        at: H256,
    ) -> Result<pallet_revive::ContractResult<pallet_revive::ExecReturnValue, Balance>> {
        let args = (
            request.origin,
            dest,
            request.value,
            Option::<sp_runtime::Weight>::None,
            Option::<Balance>::None,
            request.data,
        );
        self.api("ReviveApi_call", args, at).await
    }

    /// Dry run of `request` deploying its data as EVM init code.
    ///
    /// # Errors
    ///
    /// Node failures.
    pub async fn dry_instantiate(
        &self,
        request: DryRun,
        at: H256,
    ) -> Result<pallet_revive::ContractResult<pallet_revive::InstantiateReturnValue, Balance>> {
        let args = (
            request.origin,
            request.value,
            Option::<sp_runtime::Weight>::None,
            Option::<Balance>::None,
            pallet_revive::Code::Upload(request.data),
            Vec::<u8>::new(),
            Option::<[u8; 32]>::None,
        );
        self.api("ReviveApi_instantiate", args, at).await
    }

    /// The account behind `address` (its mapped account or its fallback account).
    ///
    /// # Errors
    ///
    /// Node failures.
    pub async fn account_id(&self, address: H160, at: H256) -> Result<AccountId> {
        self.api("ReviveApi_account_id", address, at).await
    }

    /// Revive's Ethereum view of block `at` (hash as seen by `BLOCKHASH`, gas limit, author…).
    ///
    /// # Errors
    ///
    /// Node failures.
    pub async fn eth_block(&self, at: H256) -> Result<pallet_revive::evm::Block> {
        self.api("ReviveApi_eth_block", (), at).await
    }

    /// Price of one gas unit in wei (the fee of [`REF_TIME_PER_GAS`] of weight).
    ///
    /// # Errors
    ///
    /// Node failures.
    pub async fn gas_price(&self, at: H256) -> Result<U256> {
        let weight = sp_runtime::Weight::from_parts(REF_TIME_PER_GAS, 0);
        let fee: Balance = self
            .api("TransactionPaymentApi_query_weight_to_fee", weight, at)
            .await?;
        Ok(U256::from(fee.max(1)))
    }

    /// Resolves a block by number to its hash.
    ///
    /// # Errors
    ///
    /// Node failures, or an unknown block.
    pub async fn hash_at(&self, number: u64) -> Result<H256> {
        self.node
            .block_hash(number)
            .await?
            .ok_or_else(|| anyhow!("unknown block {number}"))
    }
}

/// Gas equivalent of a weight's ref time, rounded up.
pub fn gas_of(ref_time: u64) -> u64 {
    ref_time.div_ceil(REF_TIME_PER_GAS)
}

/// Gas equivalent of a fee at `gas_price`, rounded up (so `gas × price ≥ fee` and the difference
/// is less than one gas unit's price).
pub fn gas_for_fee(fee: Balance, gas_price: U256) -> U256 {
    let fee = U256::from(fee);
    if gas_price.is_zero() {
        return U256::zero();
    }
    let (quotient, remainder) = fee.div_mod(gas_price);
    if remainder.is_zero() {
        quotient
    } else {
        quotient.saturating_add(U256::one())
    }
}
