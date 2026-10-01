//! `ac-mock-engine`: run the deterministic test engine; prints `listening on <addr>`.

use std::time::Duration;

use ac_market_proto::engine::EngineMode;
use clap::Parser;

#[derive(Parser)]
#[command(about = "Deterministic OpenAI-compatible engine for AgentCoin tests")]
struct Args {
    /// Address to listen on.
    #[arg(long, default_value = "127.0.0.1:0")]
    listen: String,
    /// Model names served.
    #[arg(long = "model", default_value = "mock-model")]
    models: Vec<String>,
    /// Milliseconds before the first token.
    #[arg(long, default_value_t = 20)]
    ttft_ms: u64,
    /// Milliseconds between tokens.
    #[arg(long, default_value_t = 5)]
    token_ms: u64,
    /// Drop every stream after this many tokens.
    #[arg(long)]
    fail_after: Option<u64>,
    /// Play the vLLM TOPLOC plugin: send pseudo-activations to this provider socket.
    #[arg(long)]
    toploc_socket: Option<std::path::PathBuf>,
    /// With --toploc-socket: send only half of the decode segments (no proof results).
    #[arg(long)]
    toploc_half_decode: bool,
    /// With --toploc-socket: `prove` (a provider's engine) or `verify` (an auditor's).
    #[arg(long, default_value = "prove", value_parser = ["prove", "verify"])]
    toploc_mode: String,
    /// Seed of the pseudo-activations (another seed plays another model).
    #[arg(long, default_value_t = 0)]
    model_seed: u64,
    /// Play the model of this seed once --switch-file exists (a provider starting to cheat).
    #[arg(long, requires = "switch_file")]
    switch_seed: Option<u64>,
    /// See --switch-seed.
    #[arg(long, requires = "switch_seed")]
    switch_file: Option<std::path::PathBuf>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let a = Args::parse();
    let engine = ac_mock_engine::spawn(
        &a.listen,
        ac_mock_engine::Config {
            models: a.models,
            ttft: Duration::from_millis(a.ttft_ms),
            token_interval: Duration::from_millis(a.token_ms),
            fail_after: a.fail_after,
            toploc: a.toploc_socket.map(|socket| ac_mock_engine::PluginConfig {
                socket,
                half_decode: a.toploc_half_decode,
                mode: if a.toploc_mode == "verify" {
                    EngineMode::Verify
                } else {
                    EngineMode::Prove
                },
            }),
            model_seed: a.model_seed,
            switch: a.switch_seed.zip(a.switch_file),
        },
    )
    .await?;
    println!("listening on {}", engine.addr);
    std::future::pending::<()>().await;
    Ok(())
}
