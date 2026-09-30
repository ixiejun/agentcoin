//! `ac-mock-engine`: run the deterministic test engine; prints `listening on <addr>`.

use std::time::Duration;

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
        },
    )
    .await?;
    println!("listening on {}", engine.addr);
    std::future::pending::<()>().await;
    Ok(())
}
