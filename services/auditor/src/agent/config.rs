//! The agent's configuration file (JSON; the workspace has no TOML dependency).
//!
//! ```json
//! {
//!   "node": "http://127.0.0.1:9944",
//!   "wallet": "auditor.json", "password_file": "auditor.pass",
//!   "kem_key": "auditor-kem.json",
//!   "listen": "0.0.0.0:8500", "public_endpoint": "https://auditor.example:8500",
//!   "data_dir": "auditor-data",
//!   "engines": [{"model": "0x…", "engine": "http://127.0.0.1:8000",
//!                "engine_model": "qwen", "socket": "/run/agentcoin/recheck.sock"}],
//!   "payers": [{"wallet": "payer1.json", "password_file": "payer1.pass",
//!               "gateways": ["atc1…"], "max_usd": "5"}],
//!   "prompts": {"bank": "bank.jsonl", "bank_percent": 20, "min_tokens": 32, "max_tokens": 256},
//!   "margin_percent": 25
//! }
//! ```

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Deserialize;

/// One re-check engine.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EngineConfig {
    /// On-chain model ID (`0x` + 64 hex digits).
    pub model: String,
    /// The engine's base URL (vLLM with the plugin in verify mode).
    pub engine: String,
    /// The model's name on the engine.
    pub engine_model: String,
    /// The plugin's socket.
    pub socket: PathBuf,
}

/// One payment account and the gateways it has credit channels with.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PayerConfig {
    /// Wallet file.
    pub wallet: PathBuf,
    /// Its password file.
    pub password_file: PathBuf,
    /// Gateway addresses.
    pub gateways: Vec<String>,
    /// Spending limit per gateway channel, in dollars.
    #[serde(default)]
    pub max_usd: Option<String>,
}

/// Prompt sources.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PromptConfig {
    /// A JSONL bank of conversations.
    #[serde(default)]
    pub bank: Option<PathBuf>,
    /// Share of bank prompts (percent).
    #[serde(default)]
    pub bank_percent: u8,
    /// Smallest `max_tokens`.
    #[serde(default)]
    pub min_tokens: Option<u32>,
    /// Largest `max_tokens`.
    #[serde(default)]
    pub max_tokens: Option<u32>,
}

/// The whole configuration.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// Node RPC URL.
    pub node: String,
    /// The auditor's wallet.
    pub wallet: PathBuf,
    /// Its password file.
    pub password_file: PathBuf,
    /// The X-Wing key file for the evidence service (`ac-auditor keygen`).
    pub kem_key: PathBuf,
    /// Password file of the key, if not the wallet's.
    #[serde(default)]
    pub kem_password_file: Option<PathBuf>,
    /// Where the evidence service listens.
    pub listen: String,
    /// The evidence endpoint registered on chain (how reviewers reach `listen`).
    pub public_endpoint: String,
    /// Data directory (evidence, payment state).
    pub data_dir: PathBuf,
    /// Re-check engines.
    pub engines: Vec<EngineConfig>,
    /// Payment accounts.
    pub payers: Vec<PayerConfig>,
    /// Prompts.
    #[serde(default)]
    pub prompts: PromptConfig,
    /// Share of each round kept free at its end (percent; default 25).
    #[serde(default = "default_margin")]
    pub margin_percent: u8,
    /// Seconds to wait for each engine's plugin to connect.
    #[serde(default = "default_connect_wait")]
    pub connect_wait: u64,
}

const fn default_margin() -> u8 {
    25
}

const fn default_connect_wait() -> u64 {
    120
}

impl Config {
    /// Reads a configuration file; relative paths are taken from the file's directory.
    ///
    /// # Errors
    ///
    /// An unreadable or malformed file.
    pub fn load(path: &Path) -> Result<Self> {
        let text =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        let mut c: Self = serde_json::from_str(&text)
            .with_context(|| format!("parsing the configuration {}", path.display()))?;
        let base = path.parent().unwrap_or_else(|| Path::new("."));
        let fix = |p: &mut PathBuf| {
            if p.is_relative() {
                *p = base.join(&*p);
            }
        };
        fix(&mut c.wallet);
        fix(&mut c.password_file);
        fix(&mut c.kem_key);
        if let Some(p) = c.kem_password_file.as_mut() {
            fix(p);
        }
        fix(&mut c.data_dir);
        for e in &mut c.engines {
            fix(&mut e.socket);
        }
        for p in &mut c.payers {
            fix(&mut p.wallet);
            fix(&mut p.password_file);
        }
        if let Some(b) = c.prompts.bank.as_mut() {
            fix(b);
        }
        Ok(c)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)] // Test code.

    use super::*;

    #[test]
    fn paths_are_relative_to_the_file_and_unknown_fields_are_refused() {
        let dir = std::env::temp_dir().join(format!("ac-auditor-config-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("c.json");
        std::fs::write(
            &file,
            r#"{"node":"http://n","wallet":"w.json","password_file":"/abs/p","kem_key":"k.json",
               "listen":"127.0.0.1:0","public_endpoint":"http://a","data_dir":"d",
               "engines":[{"model":"0x01","engine":"http://e","engine_model":"m","socket":"s.sock"}],
               "payers":[{"wallet":"p.json","password_file":"p.pass","gateways":["g"]}]}"#,
        )
        .unwrap();
        let c = Config::load(&file).unwrap();
        assert_eq!(c.wallet, dir.join("w.json"));
        assert_eq!(c.password_file, PathBuf::from("/abs/p"));
        assert_eq!(c.engines[0].socket, dir.join("s.sock"));
        assert_eq!(c.margin_percent, 25);
        std::fs::write(&file, r#"{"node":"x","bogus":1}"#).unwrap();
        assert!(Config::load(&file).is_err());
    }
}
