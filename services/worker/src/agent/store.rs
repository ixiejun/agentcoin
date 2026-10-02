//! Executed units kept on disk until the chain prunes their record (design D10): the summary,
//! the full result and the commitment's salt, so a restarted worker can still reveal and upload.
//!
//! The salt only hides a summary until its reveal, which publishes it; it is not a long-lived
//! secret, so it is stored like the result.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use ac_primitives::market::public::{JobId, UnitIndex};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::exec::Output;

/// A unit attempt: job, unit and attempt number.
pub type Key = (JobId, UnitIndex, u8);

/// An executed unit.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Saved {
    /// The summary, hexadecimal.
    pub summary: String,
    /// BLAKE3 of the full result, hexadecimal.
    pub result_hash: String,
    /// The commitment's salt, hexadecimal.
    pub salt: String,
    /// Whether the result was uploaded.
    pub uploaded: bool,
}

impl Saved {
    /// The summary's bytes.
    ///
    /// # Errors
    ///
    /// A corrupt file.
    pub fn summary(&self) -> Result<Vec<u8>> {
        Ok(hex::decode(&self.summary)?)
    }

    /// The result hash.
    ///
    /// # Errors
    ///
    /// A corrupt file.
    pub fn result_hash(&self) -> Result<[u8; 32]> {
        hex32(&self.result_hash)
    }

    /// The salt.
    ///
    /// # Errors
    ///
    /// A corrupt file.
    pub fn salt(&self) -> Result<[u8; 32]> {
        hex32(&self.salt)
    }
}

fn hex32(s: &str) -> Result<[u8; 32]> {
    let v = hex::decode(s)?;
    <[u8; 32]>::try_from(v.as_slice()).context("not 32 bytes")
}

/// The units directory.
pub struct Store {
    dir: PathBuf,
}

impl Store {
    /// Opens (creating) `<data_dir>/units`.
    ///
    /// # Errors
    ///
    /// The directory cannot be created.
    pub fn open(data_dir: &Path) -> Result<Self> {
        let dir = data_dir.join("units");
        std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
        Ok(Self { dir })
    }

    fn base(&self, (job, unit, attempt): Key) -> PathBuf {
        self.dir.join(format!("{job}-{unit}-{attempt}"))
    }

    /// Saves an executed unit before its commitment is sent.
    ///
    /// # Errors
    ///
    /// Write failures.
    pub fn save(&self, key: Key, output: &Output, salt: &[u8; 32]) -> Result<Saved> {
        let base = self.base(key);
        std::fs::write(base.with_extension("result"), &output.result)?;
        let saved = Saved {
            summary: hex::encode(&output.summary),
            result_hash: hex::encode(output.result_hash),
            salt: hex::encode(salt),
            uploaded: false,
        };
        self.put(key, &saved)?;
        Ok(saved)
    }

    fn put(&self, key: Key, saved: &Saved) -> Result<()> {
        let path = self.base(key).with_extension("json");
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec(saved)?)?;
        std::fs::rename(&tmp, &path)?;
        Ok(())
    }

    /// An executed unit, if saved.
    #[must_use]
    pub fn get(&self, key: Key) -> Option<Saved> {
        let bytes = std::fs::read(self.base(key).with_extension("json")).ok()?;
        serde_json::from_slice(&bytes).ok()
    }

    /// A saved unit's full result.
    ///
    /// # Errors
    ///
    /// A missing or unreadable file.
    pub fn result(&self, key: Key) -> Result<Vec<u8>> {
        Ok(std::fs::read(self.base(key).with_extension("result"))?)
    }

    /// Records the upload of a saved unit's result.
    ///
    /// # Errors
    ///
    /// Write failures, or no such unit.
    pub fn mark_uploaded(&self, key: Key) -> Result<()> {
        let mut saved = self.get(key).context("no saved unit")?;
        saved.uploaded = true;
        self.put(key, &saved)
    }

    /// Forgets a unit.
    pub fn remove(&self, key: Key) {
        let base = self.base(key);
        let _ = std::fs::remove_file(base.with_extension("json"));
        let _ = std::fs::remove_file(base.with_extension("result"));
    }

    /// Every saved unit.
    #[must_use]
    pub fn keys(&self) -> BTreeSet<Key> {
        let Ok(entries) = std::fs::read_dir(&self.dir) else {
            return BTreeSet::new();
        };
        entries
            .filter_map(|e| {
                let name = e.ok()?.file_name().into_string().ok()?;
                let stem = name.strip_suffix(".json")?;
                let mut parts = stem.split('-');
                let key = (
                    parts.next()?.parse().ok()?,
                    parts.next()?.parse().ok()?,
                    parts.next()?.parse().ok()?,
                );
                parts.next().is_none().then_some(key)
            })
            .collect()
    }
}
