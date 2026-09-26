//! Loading the local Aura-PQ authority key.
//!
//! Keys come either from a password-encrypted file (`ac-crypto` format v1) or, on development
//! and local chains only, from a public development name. Secret material never appears in logs
//! or error messages.

use std::path::{Path, PathBuf};

use ac_crypto::keystore::{EncryptedSecret, SecretKind};
use ac_crypto::sig::{SecretSeed, SigningKey};
use ac_primitives::aura_pq::AUTHORITY_ALG;
use sc_service::ChainType;

/// Command-line options selecting the authority key.
#[derive(Debug, Clone, clap::Args)]
pub struct PqKeyArgs {
    /// Encrypted ML-DSA-65 authority key file (see `ac-node pq-key generate`).
    #[arg(
        long,
        value_name = "PATH",
        requires = "pq_password_file",
        conflicts_with = "dev_key"
    )]
    pub pq_key_file: Option<PathBuf>,

    /// File containing the passphrase of `--pq-key-file` (a trailing newline is ignored).
    #[arg(long, value_name = "PATH")]
    pub pq_password_file: Option<PathBuf>,

    /// Public development key name such as `alice`; only on development and local chains.
    #[arg(long, value_name = "NAME")]
    pub dev_key: Option<String>,
}

/// Reads a passphrase file, dropping one trailing line ending.
///
/// # Errors
///
/// A message naming the file if it cannot be read.
pub fn read_password(path: &Path) -> Result<Vec<u8>, String> {
    let mut bytes =
        std::fs::read(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    if bytes.last() == Some(&b'\n') {
        bytes.pop();
        if bytes.last() == Some(&b'\r') {
            bytes.pop();
        }
    }
    Ok(bytes)
}

/// A validator's key: the ML-DSA-65 signing key used for block seals and AC-BFT messages
/// (distinct signing contexts), and its seed, from which per-epoch randomness secrets are derived
/// (design D8/D11 of m2-finality). Its `Debug` output is redacted; the seed is wiped on drop.
pub struct ValidatorKey {
    /// Signing key.
    pub signing: SigningKey,
    seed: SecretSeed,
}

impl ValidatorKey {
    fn from_seed(seed: SecretSeed) -> Result<Self, ac_crypto::Error> {
        Ok(Self {
            signing: SigningKey::from_seed(AUTHORITY_ALG, &seed)?,
            seed,
        })
    }

    /// The commit–reveal secret for `epoch` of the chain `genesis`.
    ///
    /// # Errors
    ///
    /// Only if the built-in hashing context were malformed (never in practice).
    pub fn randomness_secret(
        &self,
        genesis: &[u8; 32],
        epoch: u64,
    ) -> Result<ac_crypto::RandomnessSecret, ac_crypto::Error> {
        ac_crypto::randomness_secret(&self.seed, genesis, epoch)
    }
}

impl std::fmt::Debug for ValidatorKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ValidatorKey(<redacted>)")
    }
}

/// Decrypts an authority key file. Every failure after parsing reports only "decryption failed"
/// or a format problem, never key material.
///
/// # Errors
///
/// A human-readable message.
pub fn load_key_file(path: &Path, password: &[u8]) -> Result<ValidatorKey, String> {
    let json = std::fs::read_to_string(path)
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let file = EncryptedSecret::from_json(&json).map_err(|e| format!("{}: {e}", path.display()))?;
    let expected = match (file.kind(), file.public_key()) {
        (SecretKind::SigningSeed, Some(pk)) if pk.alg() == AUTHORITY_ALG => pk.clone(),
        _ => {
            return Err(format!(
                "{} is not an ML-DSA-65 authority key",
                path.display()
            ));
        }
    };
    let seed = file
        .decrypt(password)
        .map_err(|_| format!("{}: decryption failed", path.display()))?;
    let key = ValidatorKey::from_seed(SecretSeed::new(*seed))
        .map_err(|_| format!("{}: decryption failed", path.display()))?;
    if key
        .signing
        .public_key()
        .map_err(|_| "decryption failed".to_string())?
        != expected
    {
        return Err(format!("{}: decryption failed", path.display()));
    }
    Ok(key)
}

/// The development authority key of `name`.
///
/// # Errors
///
/// Only if key derivation failed (never in practice).
pub fn dev_key(name: &str) -> Result<ValidatorKey, String> {
    let seed = ac_crypto::dev_seed(name).map_err(|e| e.to_string())?;
    ValidatorKey::from_seed(seed).map_err(|e| e.to_string())
}

/// Resolves the authority key for a chain of `chain_type`. With `--dev` and no explicit key the
/// development key `alice` is used.
///
/// # Errors
///
/// Refuses development keys on live chains, and reports unreadable or undecryptable key files.
pub fn resolve(
    args: &PqKeyArgs,
    chain_type: &ChainType,
    dev_flag: bool,
) -> Result<Option<ValidatorKey>, String> {
    let dev_allowed = matches!(chain_type, ChainType::Development | ChainType::Local);
    match (&args.pq_key_file, &args.dev_key) {
        (Some(path), _) => {
            let password_file = args
                .pq_password_file
                .as_deref()
                .ok_or("--pq-key-file requires --pq-password-file")?;
            load_key_file(path, &read_password(password_file)?).map(Some)
        }
        (None, Some(name)) if dev_allowed => dev_key(name).map(Some),
        (None, Some(_)) => Err(
            "development keys (--dev-key) are only allowed on development and local chains".into(),
        ),
        (None, None) if dev_flag && dev_allowed => dev_key("alice").map(Some),
        (None, None) => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_key(dir: &Path, password: &[u8]) -> (PathBuf, ac_crypto::PqPublicKey) {
        let key = dev_key("keyfile-test").unwrap();
        let pk = key.signing.public_key().unwrap();
        let seed = ac_crypto::dev_seed("keyfile-test").unwrap();
        let mut rng = ac_crypto::OsRng::new().unwrap();
        let file = EncryptedSecret::encrypt(
            seed.expose(),
            SecretKind::SigningSeed,
            Some(&pk),
            password,
            &mut rng,
        )
        .unwrap();
        let path = dir.join("authority.json");
        std::fs::write(&path, file.to_json()).unwrap();
        (path, pk)
    }

    fn tmp() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ac-node-keys-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    // node/chain-spec Requirement "验证人密钥加载": encrypted key file round trip.
    #[test]
    fn loads_encrypted_key() {
        let dir = tmp();
        let (path, pk) = write_key(&dir, b"secret");
        assert_eq!(
            load_key_file(&path, b"secret")
                .unwrap()
                .signing
                .public_key()
                .unwrap(),
            pk
        );
    }

    // Scenario "口令错误": only "decryption failed" is reported.
    #[test]
    fn wrong_password_reports_only_decryption_failure() {
        let dir = tmp();
        let (path, _) = write_key(&dir, b"secret");
        let err = load_key_file(&path, b"wrong").err().unwrap();
        assert!(err.ends_with("decryption failed"), "{err}");
    }

    // Scenario "正式链上使用开发密钥".
    #[test]
    fn dev_keys_are_refused_on_live_chains() {
        let args = PqKeyArgs {
            pq_key_file: None,
            pq_password_file: None,
            dev_key: Some("alice".into()),
        };
        assert!(resolve(&args, &ChainType::Live, false).is_err());
        assert!(resolve(&args, &ChainType::Local, false).unwrap().is_some());
        let none = PqKeyArgs {
            pq_key_file: None,
            pq_password_file: None,
            dev_key: None,
        };
        assert!(
            resolve(&none, &ChainType::Development, true)
                .unwrap()
                .is_some()
        );
        assert!(resolve(&none, &ChainType::Live, true).unwrap().is_none());
    }

    // Scenario "开发密钥可复现".
    #[test]
    fn dev_keys_are_reproducible() {
        assert_eq!(
            dev_key("alice").unwrap().signing.public_key().unwrap(),
            dev_key("alice").unwrap().signing.public_key().unwrap()
        );
    }

    // Task 7.5: block seals, AC-BFT votes and randomness come from one key: the signing key
    // and the randomness secrets both derive from the loaded seed.
    #[test]
    fn one_key_for_seals_votes_and_randomness() {
        let dir = tmp();
        let (path, pk) = write_key(&dir, b"secret");
        let key = load_key_file(&path, b"secret").unwrap();
        assert_eq!(key.signing.public_key().unwrap(), pk);
        let seed = ac_crypto::dev_seed("keyfile-test").unwrap();
        let genesis = [7u8; 32];
        for epoch in [0, 1, 42] {
            assert_eq!(
                key.randomness_secret(&genesis, epoch).unwrap().expose(),
                ac_crypto::randomness_secret(&seed, &genesis, epoch)
                    .unwrap()
                    .expose()
            );
        }
        assert_eq!(format!("{key:?}"), "ValidatorKey(<redacted>)");
    }

    // Task 7.5: a key file of another algorithm is refused.
    #[test]
    fn non_ml_dsa_65_key_is_refused() {
        let dir = tmp();
        let seed = ac_crypto::dev_seed("keyfile-44").unwrap();
        let pk = SigningKey::from_seed(ac_crypto::SigAlg::MlDsa44, &seed)
            .unwrap()
            .public_key()
            .unwrap();
        let mut rng = ac_crypto::OsRng::new().unwrap();
        let file = EncryptedSecret::encrypt(
            seed.expose(),
            SecretKind::SigningSeed,
            Some(&pk),
            b"secret",
            &mut rng,
        )
        .unwrap();
        let path = dir.join("ml-dsa-44.json");
        std::fs::write(&path, file.to_json()).unwrap();
        let err = load_key_file(&path, b"secret").err().unwrap();
        assert!(err.contains("not an ML-DSA-65 authority key"), "{err}");
    }

    #[test]
    fn password_file_trailing_newline_is_ignored() {
        let dir = tmp();
        let path = dir.join("pw");
        std::fs::write(&path, b"secret\r\n").unwrap();
        assert_eq!(read_password(&path).unwrap(), b"secret");
    }
}
