//! The evidence of the agent's verdicts that carry a commitment (failures and verdicts judged by
//! the thresholds), kept until no dispute can use it (spec `market/auditor-agent` "证据保存与交
//! 付"; design D6 of m6-auditor-agent, D11 of m6-audit-sprt). One file per verdict, named by its
//! round, provider and commitment; readable by the owner only. Files named by round and
//! commitment alone (before m6-audit-sprt) are still read.

use std::path::{Path, PathBuf};

use ac_primitives::market::audit::RoundIndex;
use anyhow::{Context, Result};
use sp_runtime::AccountId32;

/// One stored piece of evidence.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Stored {
    /// The verdict's round.
    pub round: RoundIndex,
    /// The audited provider (unknown for files of older agents).
    pub provider: Option<AccountId32>,
    /// The evidence commitment.
    pub commitment: [u8; 32],
}

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

    fn path(&self, s: &Stored) -> PathBuf {
        let name = match &s.provider {
            Some(p) => format!(
                "{}-{}-{}.bin",
                s.round,
                hex::encode(p),
                hex::encode(s.commitment)
            ),
            None => format!("{}-{}.bin", s.round, hex::encode(s.commitment)),
        };
        self.dir.join(name)
    }

    /// Stores the evidence of a verdict of `round` on `provider`.
    ///
    /// # Errors
    ///
    /// The file cannot be written.
    pub fn put(
        &self,
        round: RoundIndex,
        provider: &AccountId32,
        commitment: &[u8; 32],
        bytes: &[u8],
    ) -> Result<()> {
        let path = self.path(&Stored {
            round,
            provider: Some(provider.clone()),
            commitment: *commitment,
        });
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
            .find(|s| s.commitment == *commitment)
            .and_then(|s| std::fs::read(self.path(&s)).ok())
    }

    /// Every stored piece of evidence, in order.
    #[must_use]
    pub fn list(&self) -> Vec<Stored> {
        let Ok(entries) = std::fs::read_dir(&self.dir) else {
            return Vec::new();
        };
        let mut out: Vec<Stored> = entries
            .flatten()
            .filter_map(|e| parse(&e.file_name().into_string().ok()?))
            .collect();
        out.sort_unstable();
        out
    }

    /// Deletes one evidence file.
    pub fn remove(&self, s: &Stored) {
        let _ = std::fs::remove_file(self.path(s));
    }
}

/// The evidence a file name stands for.
fn parse(name: &str) -> Option<Stored> {
    let stem = name.strip_suffix(".bin")?;
    let mut parts = stem.split('-');
    let round = parts.next()?.parse().ok()?;
    let (provider, hex_c) = match (parts.next()?, parts.next()) {
        (p, Some(c)) => {
            let p: [u8; 32] = hex::decode(p).ok()?.try_into().ok()?;
            (Some(AccountId32::new(p)), c)
        }
        (c, None) => (None, c),
    };
    if parts.next().is_some() {
        return None;
    }
    let commitment: [u8; 32] = hex::decode(hex_c).ok()?.try_into().ok()?;
    Some(Stored {
        round,
        provider,
        commitment,
    })
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
        let p = AccountId32::new([5; 32]);
        s.put(7, &p, &[1; 32], b"one").unwrap();
        s.put(9, &p, &[2; 32], b"two").unwrap();
        // A file of an older agent, without the provider.
        std::fs::write(
            dir.join(format!("8-{}.bin", hex::encode([3u8; 32]))),
            b"old",
        )
        .unwrap();
        let stored = |round, provider: Option<&AccountId32>, c| Stored {
            round,
            provider: provider.cloned(),
            commitment: [c; 32],
        };
        assert_eq!(
            s.list(),
            vec![
                stored(7, Some(&p), 1),
                stored(8, None, 3),
                stored(9, Some(&p), 2)
            ]
        );
        assert_eq!(s.get(&[2; 32]).unwrap(), b"two");
        assert_eq!(s.get(&[3; 32]).unwrap(), b"old");
        assert!(s.get(&[4; 32]).is_none());
        s.remove(&stored(7, Some(&p), 1));
        s.remove(&stored(8, None, 3));
        assert_eq!(s.list(), vec![stored(9, Some(&p), 2)]);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let name = format!("9-{}-{}.bin", hex::encode(p), hex::encode([2u8; 32]));
            let mode = std::fs::metadata(dir.join(name))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }
}
