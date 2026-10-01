//! The evidence of the agent's failing verdicts, kept until no dispute can use it (spec
//! `market/auditor-agent` "证据保存与交付"; design D6). One file per verdict, named by its round
//! and commitment; readable by the owner only.

use std::path::{Path, PathBuf};

use ac_primitives::market::audit::RoundIndex;
use anyhow::{Context, Result};

/// Evidence files in a directory.
#[derive(Clone, Debug)]
pub struct EvidenceStore {
    dir: PathBuf,
}

impl EvidenceStore {
    /// Opens (creating) the directory.
    ///
    /// # Errors
    ///
    /// The directory cannot be created.
    pub fn open(dir: &Path) -> Result<Self> {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        Ok(Self {
            dir: dir.to_path_buf(),
        })
    }

    fn path(&self, round: RoundIndex, commitment: &[u8; 32]) -> PathBuf {
        self.dir
            .join(format!("{round}-{}.bin", hex::encode(commitment)))
    }

    /// Stores the evidence of a verdict of `round`.
    ///
    /// # Errors
    ///
    /// The file cannot be written.
    pub fn put(&self, round: RoundIndex, commitment: &[u8; 32], bytes: &[u8]) -> Result<()> {
        let path = self.path(round, commitment);
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, bytes).with_context(|| format!("writing {}", tmp.display()))?;
        restrict(&tmp)?;
        std::fs::rename(&tmp, &path).with_context(|| format!("writing {}", path.display()))
    }

    /// The evidence of `commitment`, whatever its round.
    #[must_use]
    pub fn get(&self, commitment: &[u8; 32]) -> Option<Vec<u8>> {
        self.list()
            .into_iter()
            .find(|(_, c)| c == commitment)
            .and_then(|(r, c)| std::fs::read(self.path(r, &c)).ok())
    }

    /// Every stored (round, commitment).
    #[must_use]
    pub fn list(&self) -> Vec<(RoundIndex, [u8; 32])> {
        let Ok(entries) = std::fs::read_dir(&self.dir) else {
            return Vec::new();
        };
        let mut out: Vec<(RoundIndex, [u8; 32])> = entries
            .flatten()
            .filter_map(|e| {
                let name = e.file_name().into_string().ok()?;
                let stem = name.strip_suffix(".bin")?;
                let (round, hex_c) = stem.split_once('-')?;
                let c: [u8; 32] = hex::decode(hex_c).ok()?.try_into().ok()?;
                Some((round.parse().ok()?, c))
            })
            .collect();
        out.sort_unstable();
        out
    }

    /// Deletes one evidence file.
    pub fn remove(&self, round: RoundIndex, commitment: &[u8; 32]) {
        let _ = std::fs::remove_file(self.path(round, commitment));
    }
}

#[cfg(unix)]
fn restrict(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
        .with_context(|| format!("restricting {}", path.display()))
}

#[cfg(not(unix))]
fn restrict(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)] // Test code.

    use super::*;

    #[test]
    fn put_get_list_remove() {
        let dir = std::env::temp_dir().join(format!("ac-auditor-store-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let s = EvidenceStore::open(&dir).unwrap();
        s.put(7, &[1; 32], b"one").unwrap();
        s.put(9, &[2; 32], b"two").unwrap();
        assert_eq!(s.list(), vec![(7, [1; 32]), (9, [2; 32])]);
        assert_eq!(s.get(&[2; 32]).unwrap(), b"two");
        assert!(s.get(&[3; 32]).is_none());
        s.remove(7, &[1; 32]);
        assert_eq!(s.list(), vec![(9, [2; 32])]);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(dir.join(format!("9-{}.bin", hex::encode([2u8; 32]))))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }
}
