//! The wallet file: encrypted wallet entropy plus the account's key bookkeeping.
//!
//! The file never contains the entropy or mnemonic in clear text (spec clients/wallet-cli).
//! The account ID is derived from key index 0 of `first_alg`; rotations move to later indices.

use std::path::Path;

use ac_crypto::keystore::{EncryptedSecret, SecretKind};
use ac_crypto::sig::SigningKey;
use ac_crypto::{OsRng, PqPublicKey, SigAlg, WalletEntropy, mnemonic, wallet_key_seed};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use sp_runtime::AccountId32;
use zeroize::Zeroizing;

/// Wallet file format version.
pub const WALLET_VERSION: u8 = 1;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WalletJson {
    version: u8,
    address: String,
    first_alg: u8,
    current_alg: u8,
    current_index: u32,
    entropy: serde_json::Value,
}

/// An opened wallet file.
pub struct Wallet {
    address: String,
    first_alg: SigAlg,
    current_alg: SigAlg,
    current_index: u32,
    entropy: EncryptedSecret,
}

/// A newly created wallet together with its one-time mnemonic.
pub struct Created {
    /// The wallet.
    pub wallet: Wallet,
    /// The 24-word mnemonic; show it once, never store it.
    pub mnemonic: Zeroizing<String>,
}

fn key_for(entropy: &WalletEntropy, alg: SigAlg, index: u32) -> Result<SigningKey> {
    Ok(SigningKey::from_seed(
        alg,
        &wallet_key_seed(entropy, alg, index)?,
    )?)
}

/// Parses a signature algorithm name (`ml-dsa-44`, `ml-dsa-65`, `ml-dsa-87`).
///
/// # Errors
///
/// Unknown names.
pub fn parse_alg(name: &str) -> Result<SigAlg> {
    Ok(match name {
        "ml-dsa-44" => SigAlg::MlDsa44,
        "ml-dsa-65" => SigAlg::MlDsa65,
        "ml-dsa-87" => SigAlg::MlDsa87,
        other => bail!("unknown algorithm {other}; use ml-dsa-44, ml-dsa-65 or ml-dsa-87"),
    })
}

impl Wallet {
    fn from_entropy(entropy: &WalletEntropy, alg: SigAlg, password: &[u8]) -> Result<Self> {
        let first = key_for(entropy, alg, 0)?;
        let account = ac_crypto::account_id(&first.public_key()?);
        let mut rng = OsRng::new()?;
        let encrypted = EncryptedSecret::encrypt(
            entropy.expose(),
            SecretKind::WalletEntropy,
            None,
            password,
            &mut rng,
        )?;
        Ok(Self {
            address: ac_primitives::encode_address(account.as_bytes()),
            first_alg: alg,
            current_alg: alg,
            current_index: 0,
            entropy: encrypted,
        })
    }

    /// Creates a wallet from fresh OS entropy.
    ///
    /// # Errors
    ///
    /// OS randomness or encryption failures.
    pub fn create(alg: SigAlg, password: &[u8]) -> Result<Created> {
        let mut rng = OsRng::new()?;
        let entropy = WalletEntropy::generate(&mut rng);
        let mnemonic = mnemonic::to_mnemonic(&entropy)?;
        Ok(Created {
            wallet: Self::from_entropy(&entropy, alg, password)?,
            mnemonic,
        })
    }

    /// Restores a wallet from its 24-word mnemonic.
    ///
    /// # Errors
    ///
    /// Invalid mnemonics and encryption failures.
    pub fn import(phrase: &str, alg: SigAlg, password: &[u8]) -> Result<Self> {
        let entropy = mnemonic::from_mnemonic(phrase).context("invalid mnemonic")?;
        Self::from_entropy(&entropy, alg, password)
    }

    /// Loads a wallet file.
    ///
    /// # Errors
    ///
    /// Unreadable or malformed files.
    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("cannot read wallet {}", path.display()))?;
        let json: WalletJson = serde_json::from_str(&text).context("malformed wallet file")?;
        if json.version != WALLET_VERSION {
            bail!("unsupported wallet version {}", json.version);
        }
        let entropy = EncryptedSecret::from_json(&json.entropy.to_string())?;
        if entropy.kind() != SecretKind::WalletEntropy {
            bail!("wallet file does not contain wallet entropy");
        }
        ac_primitives::decode_address(&json.address).context("bad address in wallet file")?;
        Ok(Self {
            address: json.address,
            first_alg: SigAlg::from_id(json.first_alg)?,
            current_alg: SigAlg::from_id(json.current_alg)?,
            current_index: json.current_index,
            entropy,
        })
    }

    /// Writes the wallet file; refuses to overwrite unless `overwrite`.
    ///
    /// # Errors
    ///
    /// I/O errors or an existing file.
    pub fn save(&self, path: &Path, overwrite: bool) -> Result<()> {
        if path.exists() && !overwrite {
            bail!("{} already exists; refusing to overwrite", path.display());
        }
        let json = WalletJson {
            version: WALLET_VERSION,
            address: self.address.clone(),
            first_alg: self.first_alg.id(),
            current_alg: self.current_alg.id(),
            current_index: self.current_index,
            entropy: serde_json::from_str(&self.entropy.to_json())?,
        };
        std::fs::write(path, serde_json::to_string_pretty(&json)?)
            .with_context(|| format!("cannot write {}", path.display()))
    }

    /// The account address (`atc1…`).
    #[must_use]
    pub fn address(&self) -> &str {
        &self.address
    }

    /// The account ID.
    ///
    /// # Errors
    ///
    /// Only for a corrupted address (checked at load time).
    pub fn account(&self) -> Result<AccountId32> {
        Ok(AccountId32::new(ac_primitives::decode_address(
            &self.address,
        )?))
    }

    /// Algorithm and derivation index of the current key.
    #[must_use]
    pub fn current(&self) -> (SigAlg, u32) {
        (self.current_alg, self.current_index)
    }

    /// Decrypts the entropy and derives the key at `(alg, index)`.
    ///
    /// # Errors
    ///
    /// Wrong password ("decryption failed").
    pub fn key(&self, password: &[u8], alg: SigAlg, index: u32) -> Result<SigningKey> {
        let raw = self.entropy.decrypt(password).context("wrong password")?;
        key_for(&WalletEntropy::new(*raw), alg, index)
    }

    /// The current signing key.
    ///
    /// # Errors
    ///
    /// Wrong password.
    pub fn current_key(&self, password: &[u8]) -> Result<SigningKey> {
        self.key(password, self.current_alg, self.current_index)
    }

    /// Records a completed rotation to `(alg, index)`.
    pub fn set_current(&mut self, alg: SigAlg, index: u32) {
        self.current_alg = alg;
        self.current_index = index;
    }

    /// The public key of the account's first key (carried by the first transaction).
    ///
    /// # Errors
    ///
    /// Wrong password.
    pub fn first_public_key(&self, password: &[u8]) -> Result<PqPublicKey> {
        Ok(self.key(password, self.first_alg, 0)?.public_key()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("ac-wallet-unit-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(name)
    }

    // Requirement "创建与导入钱包" / Scenario "新建后可导入".
    #[test]
    fn created_wallet_can_be_imported() {
        let created = Wallet::create(SigAlg::MlDsa44, b"pw").unwrap();
        assert_eq!(created.mnemonic.split(' ').count(), 24);
        let imported = Wallet::import(&created.mnemonic, SigAlg::MlDsa44, b"other").unwrap();
        assert_eq!(imported.address(), created.wallet.address());
        assert!(created.wallet.address().starts_with("atc1"));
    }

    // Requirement "创建与导入钱包" / Scenario "钱包文件无明文".
    #[test]
    fn wallet_file_has_no_plaintext_secrets() {
        let created = Wallet::create(SigAlg::MlDsa44, b"pw").unwrap();
        let path = tmp("plain.json");
        let _ = std::fs::remove_file(&path);
        created.wallet.save(&path, false).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        let entropy = mnemonic::from_mnemonic(&created.mnemonic).unwrap();
        assert!(!text.contains(&hex::encode(entropy.expose())));
        // Single words can collide with JSON field names ("current_alg") or hex, so look
        // for the phrase and every adjacent pair of words instead.
        assert!(!text.contains(created.mnemonic.as_str()));
        let words: Vec<&str> = created.mnemonic.split(' ').collect();
        for pair in words.windows(2) {
            let pair = pair.join(" ");
            assert!(
                !text.contains(&pair),
                "mnemonic words {pair} in wallet file"
            );
        }
        let loaded = Wallet::load(&path).unwrap();
        assert_eq!(loaded.address(), created.wallet.address());
        assert!(created.wallet.save(&path, false).is_err());
        assert!(loaded.current_key(b"wrong").is_err());
    }

    #[test]
    fn algorithm_names() {
        assert_eq!(parse_alg("ml-dsa-65").unwrap(), SigAlg::MlDsa65);
        assert!(parse_alg("ed25519").is_err());
    }
}
