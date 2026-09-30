//! The provider's X-Wing key: generated once, stored password-encrypted (spec "加密密钥管理").

use std::path::Path;

use ac_crypto::kem::KemSecretKey;
use ac_crypto::keystore::{EncryptedSecret, SecretKind};
use ac_crypto::sig::SecretSeed;
use ac_crypto::{KemAlg, KemPublicKey, OsRng};
use anyhow::{Context, Result, bail};

/// Generates a key, writes it encrypted to `path` (which must not exist) and returns the
/// encapsulation key to register.
///
/// # Errors
///
/// An existing file, write failures or randomness failures.
pub fn generate(path: &Path, password: &[u8]) -> Result<KemPublicKey> {
    if path.exists() {
        bail!("{} already exists", path.display());
    }
    let mut rng = OsRng::new()?;
    let seed = SecretSeed::generate(&mut rng);
    let public = KemSecretKey::from_seed(KemAlg::XWing, &seed)?.public_key()?;
    let file = EncryptedSecret::encrypt_kem(seed.expose(), &public, password, &mut rng)?;
    std::fs::write(path, file.to_json()).with_context(|| format!("writing {}", path.display()))?;
    Ok(public)
}

/// Loads and decrypts a key file.
///
/// # Errors
///
/// Unreadable or malformed files, a wrong password, or a file that is not a KEM key.
pub fn load(path: &Path, password: &[u8]) -> Result<(KemSecretKey, KemPublicKey)> {
    let json =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let file = EncryptedSecret::from_json(&json)
        .map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))?;
    if file.kind() != SecretKind::KemSeed {
        bail!("{} is not a KEM key file", path.display());
    }
    let recorded = file
        .kem_public_key()
        .context("the key file records no public key")?
        .clone();
    let seed = file
        .decrypt(password)
        .map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))?;
    let secret = KemSecretKey::from_seed(recorded.alg(), &SecretSeed::new(*seed))?;
    if secret.public_key()? != recorded {
        bail!(
            "{}: the key does not match its recorded public key",
            path.display()
        );
    }
    Ok((secret, recorded))
}

/// `0x`-prefixed hex of the tagged key, as `ac-wallet market provider register --kem-key` takes.
#[must_use]
pub fn encode(key: &KemPublicKey) -> String {
    format!("0x{}", hex::encode(key.to_canonical()))
}

#[cfg(test)]
mod tests {
    use super::*;

    // Scenario "同一文件得到同一公钥".
    #[test]
    fn the_same_file_gives_the_same_key() {
        let dir = std::env::temp_dir().join(format!("ac-provider-keys-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("kem.json");
        let _ = std::fs::remove_file(&path);
        let generated = generate(&path, b"pw").unwrap();
        assert!(generate(&path, b"pw").is_err(), "never overwrites");
        let (_, a) = load(&path, b"pw").unwrap();
        let (_, b) = load(&path, b"pw").unwrap();
        assert_eq!((a.clone(), b), (generated.clone(), generated));
        assert!(encode(&a).starts_with("0x01"));
        assert!(load(&path, b"wrong").is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
