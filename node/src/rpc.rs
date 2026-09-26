//! RPC extensions on top of the SDK defaults.

use std::sync::Arc;

use ac_runtime::{AccountId, Nonce, opaque::Block};
use jsonrpsee::RpcModule;
use sc_transaction_pool_api::TransactionPool;
use sp_api::ProvideRuntimeApi;
use sp_block_builder::BlockBuilder;
use sp_blockchain::{Error as BlockChainError, HeaderBackend, HeaderMetadata};

/// Dependencies of the RPC extensions.
pub struct FullDeps<C, P> {
    /// Client.
    pub client: Arc<C>,
    /// Transaction pool.
    pub pool: Arc<P>,
}

/// Builds the RPC extensions.
///
/// # Errors
///
/// Fails if two RPC methods are registered under the same name.
pub fn create_full<C, P>(
    deps: FullDeps<C, P>,
) -> Result<RpcModule<()>, Box<dyn std::error::Error + Send + Sync>>
where
    C: ProvideRuntimeApi<Block>
        + HeaderBackend<Block>
        + HeaderMetadata<Block, Error = BlockChainError>
        + Send
        + Sync
        + 'static,
    C::Api: substrate_frame_rpc_system::AccountNonceApi<Block, AccountId, Nonce>,
    C::Api: BlockBuilder<Block>,
    P: TransactionPool + 'static,
{
    use substrate_frame_rpc_system::{System, SystemApiServer};

    let mut module = RpcModule::new(());
    let FullDeps { client, pool } = deps;
    module.merge(System::new(client, pool).into_rpc())?;
    Ok(module)
}
