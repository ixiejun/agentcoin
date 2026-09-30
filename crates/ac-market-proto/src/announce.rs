//! A gateway's announcement of its X-Wing encapsulation key, signed with its account key so
//! clients can check it against the chain (spec `market/gateway-service` "公布网关加密公钥").

use ac_crypto::hash::derive;
use ac_crypto::sig::{SigningKey, verify};
use ac_crypto::{KemPublicKey, PqPublicKey, PqSignature};
use serde::{Deserialize, Serialize};
use sp_core::H256;
use sp_runtime::AccountId32;

use crate::Error;

/// Signing context of announcements.
pub const GATEWAY_KEM_CONTEXT: &[u8] = b"agentcoin/gateway-kem/v1";

/// Hash context of the signed payload.
pub const GATEWAY_KEM_PAYLOAD_CONTEXT: &str = "agentcoin 2026-09 gateway-kem-payload v1";

/// A signed announcement.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GatewayKey {
    /// Genesis hash of the chain.
    pub genesis: H256,
    /// The gateway account.
    pub gateway: AccountId32,
    /// The gateway's encapsulation key.
    pub kem_key: KemPublicKey,
    /// The account key that signed.
    pub signer: PqPublicKey,
    /// Signature over [`payload`] under [`GATEWAY_KEM_CONTEXT`].
    pub signature: PqSignature,
}

/// `derive_key(GATEWAY_KEM_PAYLOAD_CONTEXT, genesis ‖ gateway ‖ canonical KEM key)`.
///
/// # Errors
///
/// Never in practice (the context is valid).
pub fn payload(
    genesis: H256,
    gateway: &AccountId32,
    kem_key: &KemPublicKey,
) -> Result<[u8; 32], Error> {
    let mut data = Vec::with_capacity(1300);
    data.extend_from_slice(genesis.as_bytes());
    data.extend_from_slice(gateway.as_ref());
    data.extend_from_slice(&kem_key.to_canonical());
    Ok(derive(GATEWAY_KEM_PAYLOAD_CONTEXT, &data)?)
}

impl GatewayKey {
    /// Signs an announcement with the gateway's account key.
    ///
    /// # Errors
    ///
    /// Signing failures.
    pub fn sign<R: rand_core::CryptoRng + ?Sized>(
        genesis: H256,
        gateway: AccountId32,
        kem_key: KemPublicKey,
        signer: &SigningKey,
        rng: &mut R,
    ) -> Result<Self, Error> {
        let signature = signer.sign(
            &payload(genesis, &gateway, &kem_key)?,
            GATEWAY_KEM_CONTEXT,
            rng,
        )?;
        Ok(Self {
            genesis,
            gateway,
            kem_key,
            signer: signer.public_key()?,
            signature,
        })
    }

    /// Checks the announcement for `gateway` on the chain `genesis`, whose current on-chain key
    /// is `current_key`.
    ///
    /// # Errors
    ///
    /// [`Error::InvalidAnnouncement`] for another chain, gateway or key, or a bad signature.
    pub fn verify(
        &self,
        genesis: H256,
        gateway: &AccountId32,
        current_key: &PqPublicKey,
    ) -> Result<(), Error> {
        if self.genesis != genesis || &self.gateway != gateway || &self.signer != current_key {
            return Err(Error::InvalidAnnouncement);
        }
        verify(
            &self.signer,
            &payload(self.genesis, &self.gateway, &self.kem_key)?,
            GATEWAY_KEM_CONTEXT,
            &self.signature,
        )
        .map_err(|_| Error::InvalidAnnouncement)
    }

    /// JSON form served at `GET /ac/v1/key`.
    #[must_use]
    pub fn to_json(&self) -> Vec<u8> {
        serde_json::to_vec(&Json {
            genesis: hex0x(self.genesis.as_bytes()),
            gateway: hex0x(self.gateway.as_ref()),
            kem_key: hex0x(&self.kem_key.to_canonical()),
            signer: hex0x(&self.signer.to_canonical()),
            signature: hex0x(&self.signature.to_canonical()),
        })
        .unwrap_or_default()
    }

    /// Parses the JSON form.
    ///
    /// # Errors
    ///
    /// [`Error::InvalidAnnouncement`] for malformed input.
    pub fn from_json(bytes: &[u8]) -> Result<Self, Error> {
        let j: Json = serde_json::from_slice(bytes).map_err(|_| Error::InvalidAnnouncement)?;
        let bad = |_| Error::InvalidAnnouncement;
        let genesis: [u8; 32] = unhex(&j.genesis)?
            .try_into()
            .map_err(|_| Error::InvalidAnnouncement)?;
        let gateway: [u8; 32] = unhex(&j.gateway)?
            .try_into()
            .map_err(|_| Error::InvalidAnnouncement)?;
        Ok(Self {
            genesis: H256(genesis),
            gateway: AccountId32::new(gateway),
            kem_key: KemPublicKey::from_canonical(&unhex(&j.kem_key)?).map_err(bad)?,
            signer: PqPublicKey::from_canonical(&unhex(&j.signer)?).map_err(bad)?,
            signature: PqSignature::from_canonical(&unhex(&j.signature)?).map_err(bad)?,
        })
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Json {
    genesis: String,
    gateway: String,
    kem_key: String,
    signer: String,
    signature: String,
}

fn hex0x(bytes: &[u8]) -> String {
    let mut s = String::from("0x");
    s.push_str(&hex::encode(bytes));
    s
}

fn unhex(s: &str) -> Result<Vec<u8>, Error> {
    let digits = s.strip_prefix("0x").ok_or(Error::InvalidAnnouncement)?;
    hex::decode(digits).map_err(|_| Error::InvalidAnnouncement)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ac_crypto::kem::KemSecretKey;
    use ac_crypto::sig::SecretSeed;
    use ac_crypto::{KemAlg, OsRng, SigAlg, account_id};

    fn setup(seed: u8) -> (SigningKey, AccountId32, KemPublicKey) {
        let signer = SigningKey::from_seed(SigAlg::MlDsa44, &SecretSeed::new([seed; 32])).unwrap();
        let gateway = AccountId32::new(*account_id(&signer.public_key().unwrap()).as_bytes());
        let kem =
            KemSecretKey::from_seed(KemAlg::XWing, &SecretSeed::new([seed.wrapping_add(1); 32]))
                .unwrap()
                .public_key()
                .unwrap();
        (signer, gateway, kem)
    }

    // Scenario "客户端核对网关公钥".
    #[test]
    fn clients_check_the_announcement_against_the_chain() {
        let (signer, gateway, kem) = setup(1);
        let genesis = H256([9; 32]);
        let ann = GatewayKey::sign(
            genesis,
            gateway.clone(),
            kem,
            &signer,
            &mut OsRng::new().unwrap(),
        )
        .unwrap();
        let parsed = GatewayKey::from_json(&ann.to_json()).unwrap();
        assert_eq!(parsed, ann);
        let key = signer.public_key().unwrap();
        parsed.verify(genesis, &gateway, &key).unwrap();

        // Signed by another account's key.
        let (other, _, _) = setup(5);
        let forged = GatewayKey::sign(
            genesis,
            gateway.clone(),
            ann.kem_key.clone(),
            &other,
            &mut OsRng::new().unwrap(),
        )
        .unwrap();
        assert_eq!(
            forged.verify(genesis, &gateway, &key).unwrap_err(),
            Error::InvalidAnnouncement
        );
        // Right key, but claiming another KEM key than it signed.
        let mut swapped = ann.clone();
        swapped.kem_key = setup(7).2;
        assert_eq!(
            swapped.verify(genesis, &gateway, &key).unwrap_err(),
            Error::InvalidAnnouncement
        );
        // Another chain.
        assert!(ann.verify(H256([1; 32]), &gateway, &key).is_err());
        assert!(GatewayKey::from_json(b"{}").is_err());
    }
}
