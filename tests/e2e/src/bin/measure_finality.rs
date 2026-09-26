//! Measures AC-BFT finality latency on a local network (task 8.6 of `m2-finality`); run through
//! `scripts/measure-finality.sh`.
//!
//! Usage: `measure-finality <authorities> <seconds>`. Starts that many authorities on a
//! generated chain spec, waits until they finalize, then for `<seconds>` records on the first
//! node, over WebSocket, when each block is imported and when it is finalized (a finalized head
//! finalizes all its ancestors). Prints one line with the number of blocks and the P50, P95 and
//! maximum of finalized-time − import-time.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use ac_e2e::{Heads, Testnet, TestnetConfig, write_spec};

fn number(header: &serde_json::Value) -> Option<u64> {
    u64::from_str_radix(header["number"].as_str()?.trim_start_matches("0x"), 16).ok()
}

/// The `p`-th percentile (0–100) of sorted `values`, nearest rank.
fn percentile(sorted: &[f64], p: usize) -> f64 {
    if sorted.is_empty() {
        return f64::NAN;
    }
    let rank = (sorted.len().saturating_mul(p)).div_ceil(100).max(1);
    sorted
        .get(rank.saturating_sub(1))
        .copied()
        .unwrap_or(f64::NAN)
}

#[tokio::main]
async fn main() -> Result<(), String> {
    let mut args = std::env::args().skip(1);
    let authorities: usize = args
        .next()
        .and_then(|a| a.parse().ok())
        .ok_or("usage: measure-finality <authorities> <seconds>")?;
    let seconds: u64 = args
        .next()
        .and_then(|a| a.parse().ok())
        .ok_or("usage: measure-finality <authorities> <seconds>")?;

    let dir = std::env::temp_dir().join(format!("ac-measure-{}", std::process::id()));
    let spec = write_spec(&dir, authorities)?;
    let net = Testnet::start_with(
        TestnetConfig {
            label: format!("measure-{authorities}"),
            chain: spec.display().to_string(),
            authorities,
            args: Vec::new(),
        },
        Duration::from_secs(120),
    )
    .await?;
    let all: Vec<usize> = (0..authorities).collect();
    // Stable operation: every node finalizing.
    net.wait_finalized(&all, 5, Duration::from_secs(180))
        .await?;

    let node = net.nodes.first().ok_or("no nodes")?;
    let (_imports_client, mut imports) = node
        .subscribe(Heads::All)
        .await
        .map_err(|e| e.to_string())?;
    let (_finality_client, mut finality) = node
        .subscribe(Heads::Finalized)
        .await
        .map_err(|e| e.to_string())?;
    let mut imported: BTreeMap<u64, Instant> = BTreeMap::new();
    let mut latencies = Vec::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(seconds);
    loop {
        tokio::select! {
            () = tokio::time::sleep_until(deadline) => break,
            header = imports.next() => {
                let header = header.ok_or("import subscription closed")?.map_err(|e| e.to_string())?;
                if let Some(n) = number(&header) {
                    imported.entry(n).or_insert_with(Instant::now);
                }
            }
            header = finality.next() => {
                let header = header.ok_or("finality subscription closed")?.map_err(|e| e.to_string())?;
                let Some(n) = number(&header) else { continue };
                let now = Instant::now();
                // Only blocks whose import was observed during the measurement.
                let done: Vec<u64> = imported.range(..=n).map(|(k, _)| *k).collect();
                for k in done {
                    if let Some(at) = imported.remove(&k) {
                        latencies.push(now.duration_since(at).as_secs_f64());
                    }
                }
            }
        }
    }
    drop(net);
    let _ = std::fs::remove_dir_all(&dir);

    latencies.sort_by(f64::total_cmp);
    println!(
        "{authorities} nodes: {} blocks finalized, P50 {:.3} s, P95 {:.3} s, max {:.3} s",
        latencies.len(),
        percentile(&latencies, 50),
        percentile(&latencies, 95),
        latencies.last().copied().unwrap_or(f64::NAN),
    );
    if latencies.is_empty() {
        return Err("no block was finalized during the measurement".into());
    }
    Ok(())
}
