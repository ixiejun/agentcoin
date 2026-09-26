//! Built-in chain specifications.

use ac_runtime::WASM_BINARY;
use sc_service::{ChainType, Properties};

/// Chain specification type of this node.
pub type ChainSpec = sc_service::GenericChainSpec;

fn properties() -> Properties {
    let mut properties = Properties::new();
    properties.insert("tokenSymbol".into(), "ATC".into());
    properties.insert("tokenDecimals".into(), 18.into());
    properties
}

fn wasm() -> Result<&'static [u8], String> {
    WASM_BINARY.ok_or_else(|| "the runtime WASM binary was not built".to_string())
}

/// Single-node development chain.
pub fn development() -> Result<ChainSpec, String> {
    Ok(ChainSpec::builder(wasm()?, Default::default())
        .with_name("AgentCoin Development")
        .with_id("agentcoin_dev")
        .with_chain_type(ChainType::Development)
        .with_genesis_config_preset_name(sp_genesis_builder::DEV_RUNTIME_PRESET)
        .with_properties(properties())
        .build())
}

/// Three-node local test network.
pub fn local_testnet() -> Result<ChainSpec, String> {
    Ok(ChainSpec::builder(wasm()?, Default::default())
        .with_name("AgentCoin Local Testnet")
        .with_id("agentcoin_local")
        .with_chain_type(ChainType::Local)
        .with_genesis_config_preset_name(sp_genesis_builder::LOCAL_TESTNET_RUNTIME_PRESET)
        .with_properties(properties())
        .build())
}
