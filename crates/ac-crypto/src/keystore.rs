//! Password-encrypted secret files (format v1).
//!
//! A 32-byte secret (a signing seed, wallet entropy or a KEM key seed) is encrypted with XChaCha20-Poly1305
//! under a key derived from the passphrase with Argon2id. Both primitives have 256-bit
//! symmetric strength, which stays adequate against quantum adversaries. All metadata
//! (format version, kind, algorithm, public key, KDF parameters) is bound to the ciphertext as
//! associated data, so editing any field makes decryption fail.

use alloc::string::{String, ToString};
use alloc::vec::Vec;

use argon2::{Algorithm, Argon2, Params, Version};
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{Key, XChaCha20Poly1305, XNonce};
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use crate::alg::{KemAlg, SigAlg};
use crate::error::Error;
use crate::hash::derive;
use crate::tagged::{KemPublicKey, PqPublicKey};

/// Current file format version.
pub const FORMAT_VERSION: u8 = 1;

/// Context of the associated-data hash. Never change it.
pub const KEYSTORE_AAD_CONTEXT: &str = "agentcoin 2026-09 keystore-aad v1";

const SALT_LEN: usize = 16;
const NONCE_LEN: usize = 24;
const SECRET_LEN: usize = 32;
const KDF_NAME: &str = "argon2id";
const CIPHER_NAME: &str = "xchacha20poly1305";

/// What the encrypted 32 bytes are.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SecretKind {
    /// An ML-DSA key-generation seed; the file records the algorithm and public key.
    SigningSeed,
    /// Wallet entropy (the secret behind a mnemonic).
    WalletEntropy,
    /// A KEM key-generation seed; the file records the KEM algorithm and encapsulation key.
    KemSeed,
}

impl SecretKind {
    const fn label(self) -> &'static str {
        match self {
            Self::SigningSeed => "signing-seed",
            Self::WalletEntropy => "wallet-entropy",
            Self::KemSeed => "kem-seed",
        }
    }

    const fn code(self) -> u8 {
        match self {
            Self::SigningSeed => 1,
            Self::WalletEntropy => 2,
            Self::KemSeed => 3,
        }
    }

    fn from_label(label: &str) -> Result<Self, Error> {
        match label {
            "signing-seed" => Ok(Self::SigningSeed),
            "wallet-entropy" => Ok(Self::WalletEntropy),
            "kem-seed" => Ok(Self::KemSeed),
            _ => Err(Error::InvalidKeystore),
        }
    }
}

/// Argon2id cost parameters.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KdfParams {
    m_kib: u32,
    t: u32,
    p: u32,
}

impl KdfParams {
    /// Minimum accepted parameters (and the defaults): 64 MiB, 3 passes, 1 lane.
    pub const MINIMUM: Self = Self {
        m_kib: 64 * 1024,
        t: 3,
        p: 1,
    };

    /// Memory cost in KiB.
    #[must_use]
    pub const fn memory_kib(&self) -> u32 {
        self.m_kib
    }

    /// Number of passes.
    #[must_use]
    pub const fn iterations(&self) -> u32 {
        self.t
    }

    /// Degree of parallelism.
    #[must_use]
    pub const fn parallelism(&self) -> u32 {
        self.p
    }

    const fn meets_minimum(&self) -> bool {
        self.m_kib >= Self::MINIMUM.m_kib && self.t >= Self::MINIMUM.t && self.p >= Self::MINIMUM.p
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct KdfJson {
    name: String,
    m_kib: u32,
    t: u32,
    p: u32,
    salt: String,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CipherJson {
    name: String,
    nonce: String,
    ciphertext: String,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileJson {
    version: u8,
    kind: String,
    alg: Option<u8>,
    public_key: Option<String>,
    kdf: KdfJson,
    cipher: CipherJson,
}

/// The public key a file records next to its secret.
#[derive(Clone, Debug, PartialEq, Eq)]
enum RecordedKey {
    Signing(PqPublicKey),
    Kem(KemPublicKey),
}

impl RecordedKey {
    fn alg_id(&self) -> u8 {
        match self {
            Self::Signing(pk) => pk.alg().id(),
            Self::Kem(pk) => pk.alg().id(),
        }
    }

    fn to_canonical(&self) -> Vec<u8> {
        match self {
            Self::Signing(pk) => pk.to_canonical(),
            Self::Kem(pk) => pk.to_canonical(),
        }
    }

    /// Marker byte in the associated data: signing keys keep the original `1`.
    const fn aad_marker(&self) -> u8 {
        match self {
            Self::Signing(_) => 1,
            Self::Kem(_) => 2,
        }
    }

    /// Whether `kind` must record this sort of key (wallet entropy records none).
    const fn matches(key: Option<&Self>, kind: SecretKind) -> bool {
        matches!(
            (kind, key),
            (SecretKind::SigningSeed, Some(Self::Signing(_)))
                | (SecretKind::KemSeed, Some(Self::Kem(_)))
                | (SecretKind::WalletEntropy, None)
        )
    }
}

/// A parsed, well-formed encrypted secret file.
#[derive(Clone)]
pub struct EncryptedSecret {
    kind: SecretKind,
    public_key: Option<RecordedKey>,
    kdf: KdfParams,
    salt: [u8; SALT_LEN],
    nonce: [u8; NONCE_LEN],
    ciphertext: Vec<u8>,
}

impl core::fmt::Debug for EncryptedSecret {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("EncryptedSecret")
            .field("kind", &self.kind)
            .field("public_key", &self.public_key)
            .field("kdf", &self.kdf)
            .finish_non_exhaustive()
    }
}

impl EncryptedSecret {
    /// Encrypts `secret` under `passphrase` with the default (minimum) KDF parameters.
    ///
    /// `public_key` must be given for [`SecretKind::SigningSeed`] and omitted for
    /// [`SecretKind::WalletEntropy`].
    ///
    /// # Errors
    ///
    /// [`Error::InvalidKeystore`] if the public key does not match the kind, or an
    /// encryption failure.
    pub fn encrypt<R: rand_core::CryptoRng + ?Sized>(
        secret: &[u8; SECRET_LEN],
        kind: SecretKind,
        public_key: Option<&PqPublicKey>,
        passphrase: &[u8],
        rng: &mut R,
    ) -> Result<Self, Error> {
        let key = public_key.cloned().map(RecordedKey::Signing);
        Self::encrypt_with(secret, (kind, key), passphrase, KdfParams::MINIMUM, rng)
    }

    /// Encrypts a KEM key seed ([`SecretKind::KemSeed`]) whose encapsulation key is
    /// `public_key`, with the default (minimum) KDF parameters.
    ///
    /// # Errors
    ///
    /// An encryption failure.
    pub fn encrypt_kem<R: rand_core::CryptoRng + ?Sized>(
        secret: &[u8; SECRET_LEN],
        public_key: &KemPublicKey,
        passphrase: &[u8],
        rng: &mut R,
    ) -> Result<Self, Error> {
        Self::encrypt_with(
            secret,
            (
                SecretKind::KemSeed,
                Some(RecordedKey::Kem(public_key.clone())),
            ),
            passphrase,
            KdfParams::MINIMUM,
            rng,
        )
    }

    fn encrypt_with<R: rand_core::CryptoRng + ?Sized>(
        secret: &[u8; SECRET_LEN],
        (kind, public_key): (SecretKind, Option<RecordedKey>),
        passphrase: &[u8],
        kdf: KdfParams,
        rng: &mut R,
    ) -> Result<Self, Error> {
        if !RecordedKey::matches(public_key.as_ref(), kind) {
            return Err(Error::InvalidKeystore);
        }
        let mut salt = [0u8; SALT_LEN];
        let mut nonce = [0u8; NONCE_LEN];
        rng.fill_bytes(&mut salt);
        rng.fill_bytes(&mut nonce);
        let mut file = Self {
            kind,
            public_key,
            kdf,
            salt,
            nonce,
            ciphertext: Vec::new(),
        };
        let key = file.derive_key(passphrase)?;
        let aad = file.associated_data()?;
        file.ciphertext = cipher(&key)
            .encrypt(
                &XNonce::from(nonce),
                Payload {
                    msg: secret,
                    aad: &aad,
                },
            )
            .map_err(|_| Error::InvalidKeystore)?;
        Ok(file)
    }

    /// Decrypts the secret.
    ///
    /// # Errors
    ///
    /// [`Error::DecryptionFailed`] for a wrong passphrase or any tampering; no partial
    /// plaintext is ever returned.
    pub fn decrypt(&self, passphrase: &[u8]) -> Result<Zeroizing<[u8; SECRET_LEN]>, Error> {
        let key = self.derive_key(passphrase)?;
        let aad = self.associated_data()?;
        let plain = Zeroizing::new(
            cipher(&key)
                .decrypt(
                    &XNonce::from(self.nonce),
                    Payload {
                        msg: &self.ciphertext,
                        aad: &aad,
                    },
                )
                .map_err(|_| Error::DecryptionFailed)?,
        );
        let mut out = Zeroizing::new([0u8; SECRET_LEN]);
        if plain.len() != SECRET_LEN {
            return Err(Error::DecryptionFailed);
        }
        out.copy_from_slice(&plain);
        Ok(out)
    }

    /// What the file contains.
    #[must_use]
    pub const fn kind(&self) -> SecretKind {
        self.kind
    }

    /// The recorded public key (signing seeds only), readable without the passphrase.
    #[must_use]
    pub const fn public_key(&self) -> Option<&PqPublicKey> {
        match &self.public_key {
            Some(RecordedKey::Signing(pk)) => Some(pk),
            _ => None,
        }
    }

    /// The recorded encapsulation key (KEM seeds only), readable without the passphrase.
    #[must_use]
    pub const fn kem_public_key(&self) -> Option<&KemPublicKey> {
        match &self.public_key {
            Some(RecordedKey::Kem(pk)) => Some(pk),
            _ => None,
        }
    }

    /// The KDF parameters of this file.
    #[must_use]
    pub const fn kdf_params(&self) -> KdfParams {
        self.kdf
    }

    /// Serializes to the JSON file format v1.
    #[must_use]
    pub fn to_json(&self) -> String {
        let file = FileJson {
            version: FORMAT_VERSION,
            kind: self.kind.label().to_string(),
            alg: self.public_key.as_ref().map(RecordedKey::alg_id),
            public_key: self
                .public_key
                .as_ref()
                .map(|pk| hex::encode(pk.to_canonical())),
            kdf: KdfJson {
                name: KDF_NAME.to_string(),
                m_kib: self.kdf.m_kib,
                t: self.kdf.t,
                p: self.kdf.p,
                salt: hex::encode(self.salt),
            },
            cipher: CipherJson {
                name: CIPHER_NAME.to_string(),
                nonce: hex::encode(self.nonce),
                ciphertext: hex::encode(&self.ciphertext),
            },
        };
        // Serializing plain strings and integers cannot fail.
        serde_json::to_string_pretty(&file).unwrap_or_default()
    }

    /// Parses the JSON file format v1.
    ///
    /// # Errors
    ///
    /// [`Error::InvalidKeystore`] for malformed files or unsupported versions and algorithms,
    /// [`Error::WeakKdfParams`] if the KDF parameters are below [`KdfParams::MINIMUM`].
    pub fn from_json(json: &str) -> Result<Self, Error> {
        let file: FileJson = serde_json::from_str(json).map_err(|_| Error::InvalidKeystore)?;
        if file.version != FORMAT_VERSION
            || file.kdf.name != KDF_NAME
            || file.cipher.name != CIPHER_NAME
        {
            return Err(Error::InvalidKeystore);
        }
        let kind = SecretKind::from_label(&file.kind)?;
        let public_key = match (&file.alg, &file.public_key) {
            (Some(alg), Some(pk)) => {
                let bytes = hex::decode(pk).map_err(|_| Error::InvalidKeystore)?;
                if kind == SecretKind::KemSeed {
                    let pk = KemPublicKey::from_canonical(&bytes)?;
                    if pk.alg() != KemAlg::from_id(*alg)? {
                        return Err(Error::InvalidKeystore);
                    }
                    Some(RecordedKey::Kem(pk))
                } else {
                    let pk = PqPublicKey::from_canonical(&bytes)?;
                    if pk.alg() != SigAlg::from_id(*alg)? {
                        return Err(Error::InvalidKeystore);
                    }
                    Some(RecordedKey::Signing(pk))
                }
            }
            (None, None) => None,
            _ => return Err(Error::InvalidKeystore),
        };
        if !RecordedKey::matches(public_key.as_ref(), kind) {
            return Err(Error::InvalidKeystore);
        }
        let kdf = KdfParams {
            m_kib: file.kdf.m_kib,
            t: file.kdf.t,
            p: file.kdf.p,
        };
        if !kdf.meets_minimum() {
            return Err(Error::WeakKdfParams);
        }
        Ok(Self {
            kind,
            public_key,
            kdf,
            salt: decode_array(&file.kdf.salt)?,
            nonce: decode_array(&file.cipher.nonce)?,
            ciphertext: hex::decode(&file.cipher.ciphertext).map_err(|_| Error::InvalidKeystore)?,
        })
    }

    fn derive_key(&self, passphrase: &[u8]) -> Result<Zeroizing<[u8; 32]>, Error> {
        let params = Params::new(self.kdf.m_kib, self.kdf.t, self.kdf.p, Some(32))
            .map_err(|_| Error::InvalidKeystore)?;
        let mut key = Zeroizing::new([0u8; 32]);
        Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
            .hash_password_into(passphrase, &self.salt, key.as_mut_slice())
            .map_err(|_| Error::InvalidKeystore)?;
        Ok(key)
    }

    /// `derive_key(KEYSTORE_AAD_CONTEXT, fixed-layout encoding of every metadata field)`.
    fn associated_data(&self) -> Result<[u8; 32], Error> {
        let mut data = Vec::with_capacity(64);
        data.push(FORMAT_VERSION);
        data.push(self.kind.code());
        match &self.public_key {
            Some(pk) => {
                let canonical = pk.to_canonical();
                data.push(pk.aad_marker());
                let len = u32::try_from(canonical.len()).map_err(|_| Error::InvalidKeystore)?;
                data.extend_from_slice(&len.to_le_bytes());
                data.extend_from_slice(&canonical);
            }
            None => data.push(0),
        }
        data.extend_from_slice(&self.kdf.m_kib.to_le_bytes());
        data.extend_from_slice(&self.kdf.t.to_le_bytes());
        data.extend_from_slice(&self.kdf.p.to_le_bytes());
        data.extend_from_slice(&self.salt);
        derive(KEYSTORE_AAD_CONTEXT, &data)
    }
}

fn cipher(key: &[u8; 32]) -> XChaCha20Poly1305 {
    XChaCha20Poly1305::new(&Key::from(*key))
}

fn decode_array<const N: usize>(s: &str) -> Result<[u8; N], Error> {
    hex::decode(s)
        .map_err(|_| Error::InvalidKeystore)?
        .try_into()
        .map_err(|_| Error::InvalidKeystore)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::sig::{SecretSeed, SigningKey};

    const TEST_KDF: KdfParams = KdfParams::MINIMUM;

    pub(crate) struct TestRng(pub u64);
    impl rand_core::TryRng for TestRng {
        type Error = core::convert::Infallible;
        fn try_next_u32(&mut self) -> Result<u32, Self::Error> {
            Ok(u32::try_from(self.try_next_u64()? >> 32).unwrap())
        }
        fn try_next_u64(&mut self) -> Result<u64, Self::Error> {
            self.0 = self
                .0
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            Ok(self.0)
        }
        fn try_fill_bytes(&mut self, dst: &mut [u8]) -> Result<(), Self::Error> {
            for b in dst {
                *b = u8::try_from(self.try_next_u64()? >> 56).unwrap();
            }
            Ok(())
        }
    }
    impl rand_core::TryCryptoRng for TestRng {}

    fn signing_file(pass: &[u8], rng: &mut TestRng) -> (EncryptedSecret, SecretSeed, PqPublicKey) {
        let seed = SecretSeed::new([9; 32]);
        let pk = SigningKey::from_seed(SigAlg::MlDsa65, &seed)
            .unwrap()
            .public_key()
            .unwrap();
        let file = EncryptedSecret::encrypt_with(
            seed.expose(),
            (
                SecretKind::SigningSeed,
                Some(RecordedKey::Signing(pk.clone())),
            ),
            pass,
            TEST_KDF,
            rng,
        )
        .unwrap();
        (file, seed, pk)
    }

    // Scenario "加密再解密得到相同种子".
    #[test]
    fn round_trip_through_json() {
        let (file, seed, pk) = signing_file(b"correct horse", &mut TestRng(1));
        let parsed = EncryptedSecret::from_json(&file.to_json()).unwrap();
        let plain = parsed.decrypt(b"correct horse").unwrap();
        assert_eq!(&*plain, seed.expose());
        let derived = SigningKey::from_seed(SigAlg::MlDsa65, &SecretSeed::new(*plain))
            .unwrap()
            .public_key()
            .unwrap();
        assert_eq!(parsed.public_key(), Some(&derived));
        assert_eq!(derived, pk);
    }

    // Scenario "口令错误".
    #[test]
    fn wrong_passphrase_fails() {
        let (file, _, _) = signing_file(b"right", &mut TestRng(2));
        assert_eq!(file.decrypt(b"wrong").unwrap_err(), Error::DecryptionFailed);
    }

    // Scenario "篡改公钥字段".
    #[test]
    fn tampered_public_key_fails() {
        let (file, _, _) = signing_file(b"pw", &mut TestRng(3));
        let other_pk = SigningKey::from_seed(SigAlg::MlDsa65, &SecretSeed::new([1; 32]))
            .unwrap()
            .public_key()
            .unwrap();
        let mut json: serde_json::Value = serde_json::from_str(&file.to_json()).unwrap();
        json["public_key"] = hex::encode(other_pk.to_canonical()).into();
        let tampered = EncryptedSecret::from_json(&json.to_string()).unwrap();
        assert_eq!(
            tampered.decrypt(b"pw").unwrap_err(),
            Error::DecryptionFailed
        );

        // Changing the algorithm byte alone is rejected as malformed (it no longer matches the key).
        let mut json: serde_json::Value = serde_json::from_str(&file.to_json()).unwrap();
        json["alg"] = 1.into();
        assert!(EncryptedSecret::from_json(&json.to_string()).is_err());
    }

    // Scenario "同一口令两次加密结果不同".
    #[test]
    fn fresh_salt_and_nonce_each_time() {
        let mut rng = TestRng(4);
        let (a, _, _) = signing_file(b"pw", &mut rng);
        let (b, _, _) = signing_file(b"pw", &mut rng);
        assert_ne!(a.salt, b.salt);
        assert_ne!(a.ciphertext, b.ciphertext);
    }

    // Scenario "参数过低的文件".
    #[test]
    fn weak_kdf_parameters_are_rejected() {
        let (file, _, _) = signing_file(b"pw", &mut TestRng(5));
        let mut json: serde_json::Value = serde_json::from_str(&file.to_json()).unwrap();
        json["kdf"]["m_kib"] = (8 * 1024).into();
        assert_eq!(
            EncryptedSecret::from_json(&json.to_string()).unwrap_err(),
            Error::WeakKdfParams
        );
    }

    #[test]
    fn kind_and_public_key_must_agree() {
        let pk = SigningKey::from_seed(SigAlg::MlDsa44, &SecretSeed::new([2; 32]))
            .unwrap()
            .public_key()
            .unwrap();
        let mut rng = TestRng(6);
        assert!(
            EncryptedSecret::encrypt(&[0; 32], SecretKind::SigningSeed, None, b"", &mut rng)
                .is_err()
        );
        assert!(
            EncryptedSecret::encrypt(
                &[0; 32],
                SecretKind::WalletEntropy,
                Some(&pk),
                b"",
                &mut rng
            )
            .is_err()
        );
    }

    // Scenario "KEM 密钥种子" (m5-gateway-provider 1.1).
    #[cfg(feature = "kem")]
    #[test]
    fn kem_seed_round_trip() {
        use crate::kem::KemSecretKey;
        let seed = SecretSeed::new([5; 32]);
        let pk = KemSecretKey::from_seed(KemAlg::XWing, &seed)
            .unwrap()
            .public_key()
            .unwrap();
        let file =
            EncryptedSecret::encrypt_kem(seed.expose(), &pk, b"pw", &mut TestRng(8)).unwrap();
        let parsed = EncryptedSecret::from_json(&file.to_json()).unwrap();
        assert_eq!(parsed.kind(), SecretKind::KemSeed);
        assert_eq!(parsed.kem_public_key(), Some(&pk));
        assert_eq!(parsed.public_key(), None);
        let plain = parsed.decrypt(b"pw").unwrap();
        assert_eq!(&*plain, seed.expose());
        let derived = KemSecretKey::from_seed(KemAlg::XWing, &SecretSeed::new(*plain))
            .unwrap()
            .public_key()
            .unwrap();
        assert_eq!(derived, pk);

        // Read as a signing-key file: the kind or the key type does not match.
        let mut json: serde_json::Value = serde_json::from_str(&file.to_json()).unwrap();
        json["kind"] = "signing-seed".into();
        assert!(EncryptedSecret::from_json(&json.to_string()).is_err());
        // A tampered encapsulation key fails authentication.
        let other = KemSecretKey::from_seed(KemAlg::XWing, &SecretSeed::new([6; 32]))
            .unwrap()
            .public_key()
            .unwrap();
        let mut json: serde_json::Value = serde_json::from_str(&file.to_json()).unwrap();
        json["public_key"] = hex::encode(other.to_canonical()).into();
        let tampered = EncryptedSecret::from_json(&json.to_string()).unwrap();
        assert_eq!(
            tampered.decrypt(b"pw").unwrap_err(),
            Error::DecryptionFailed
        );
    }

    // Requirement "秘密材料保护": Debug output never contains ciphertext or secrets.
    #[test]
    fn debug_is_redacted() {
        let (file, _, _) = signing_file(b"pw", &mut TestRng(7));
        let s = alloc::format!("{file:?}");
        assert!(!s.contains(&hex::encode(&file.ciphertext)));
    }
}
