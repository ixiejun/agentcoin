//! The worker's configuration file (JSON).
//!
//! ```json
//! {
//!   "node": "http://127.0.0.1:9944",
//!   "wallet": "worker.json", "password_file": "worker.pass",
//!   "data_dir": "worker-data",
//!   "engines": [{"model": "0x…", "engine": "http://127.0.0.1:8000", "engine_model": "qwen"}],
//!   "poll_ms": 1000
//! }
//! ```
//!
//! Relative paths are taken from the file's directory.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Deserialize;

/// One engine and the model it runs.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EngineConfig {
    /// On-chain model ID (`0x` + 64 hex digits).
    pub model: String,
    /// The engine's base URL.
    pub engine: String,
    /// The model's name on the engine.
    pub engine_model: String,
}

/// The whole configuration.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// Node RPC URL.
    pub node: String,
    /// The worker's wallet.
    pub wallet: PathBuf,
    /// Its password file.
    pub password_file: PathBuf,
    /// Where executed units are kept.
    pub data_dir: PathBuf,
    /// Engines, one per model the worker runs (data cleaning needs none).
    #[serde(default)]
    pub engines: Vec<EngineConfig>,
    /// Milliseconds between ticks.
    #[serde(default = "default_poll")]
    pub poll_ms: u64,
}

const fn default_poll() -> u64 {
    1_000
}

impl Config {
    /// Reads a configuration file.
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
        for p in [&mut c.wallet, &mut c.password_file, &mut c.data_dir] {
            if p.is_relative() {
                *p = base.join(&*p);
            }
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
        let dir = std::env::temp_dir().join(format!("ac-worker-config-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("c.json");
        std::fs::write(
            &file,
            r#"{"node":"http://n","wallet":"w.json","password_file":"/abs/p","data_dir":"d",
               "engines":[{"model":"0x01","engine":"http://e","engine_model":"m"}]}"#,
        )
        .unwrap();
        let c = Config::load(&file).unwrap();
        assert_eq!(c.wallet, dir.join("w.json"));
        assert_eq!(c.password_file, PathBuf::from("/abs/p"));
        assert_eq!(c.data_dir, dir.join("d"));
        assert_eq!(c.poll_ms, 1_000);
        std::fs::write(&file, r#"{"node":"x","bogus":1}"#).unwrap();
        assert!(Config::load(&file).is_err());
    }
}
