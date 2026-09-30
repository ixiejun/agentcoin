//! Co-signed receipts kept until their challenge period is over (spec "费用计算与收据签名").
//! Files hold receipts only: never prompts or outputs.

use std::path::{Path, PathBuf};

use ac_primitives::market::SignedReceipt;
use anyhow::{Context, Result};
use parity_scale_codec::{Decode, Encode};
use serde_json::{Value, json};

/// A directory of receipts, one JSON file per request ID.
#[derive(Clone, Debug)]
pub struct Store {
    dir: PathBuf,
}

impl Store {
    /// Opens (creating) `dir`.
    ///
    /// # Errors
    ///
    /// If the directory cannot be created.
    pub fn open(dir: &Path) -> Result<Self> {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        Ok(Self {
            dir: dir.to_path_buf(),
        })
    }

    /// Saves a receipt stamped with the block it was stored at (write, then rename).
    ///
    /// # Errors
    ///
    /// Write failures.
    pub fn save(&self, receipt: &SignedReceipt, block: u64) -> Result<()> {
        let name = hex::encode(receipt.body.request_id);
        let tmp = self.dir.join(format!("{name}.tmp"));
        let body =
            json!({ "block": block, "receipt": format!("0x{}", hex::encode(receipt.encode())) });
        std::fs::write(&tmp, body.to_string())
            .with_context(|| format!("writing {}", tmp.display()))?;
        std::fs::rename(&tmp, self.dir.join(format!("{name}.json")))
            .context("storing the receipt")?;
        Ok(())
    }

    /// Every stored receipt with its block.
    ///
    /// # Errors
    ///
    /// Unreadable directories; malformed files are skipped.
    pub fn list(&self) -> Result<Vec<(u64, SignedReceipt, PathBuf)>> {
        let mut out = Vec::new();
        for entry in std::fs::read_dir(&self.dir)? {
            let path = entry?.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let Some((block, receipt)) =
                std::fs::read_to_string(&path).ok().and_then(|t| parse(&t))
            else {
                continue;
            };
            out.push((block, receipt, path));
        }
        Ok(out)
    }

    /// Deletes receipts stored before `block`; returns how many.
    ///
    /// # Errors
    ///
    /// Unreadable directories or failed deletions.
    pub fn prune(&self, block: u64) -> Result<usize> {
        let mut n = 0usize;
        for (at, _, path) in self.list()? {
            if at < block {
                std::fs::remove_file(&path)?;
                n = n.saturating_add(1);
            }
        }
        Ok(n)
    }
}

fn parse(text: &str) -> Option<(u64, SignedReceipt)> {
    let v: Value = serde_json::from_str(text).ok()?;
    let bytes = hex::decode(v.get("receipt")?.as_str()?.trim_start_matches("0x")).ok()?;
    Some((
        v.get("block")?.as_u64()?,
        SignedReceipt::decode(&mut &bytes[..]).ok()?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ac_crypto::sig::{SecretSeed, SigningKey};
    use ac_crypto::{OsRng, SigAlg};
    use ac_primitives::market::receipt::RECEIPT_CONTEXT;
    use ac_primitives::market::work::JobKind;
    use ac_primitives::market::{MicroUsd, ModelId, ReceiptBody};
    use sp_core::H256;
    use sp_runtime::AccountId32;

    fn receipt(id: u8) -> SignedReceipt {
        let key = SigningKey::from_seed(SigAlg::MlDsa44, &SecretSeed::new([1; 32])).unwrap();
        let body = ReceiptBody {
            genesis: H256([0; 32]),
            gateway: AccountId32::new([2; 32]),
            provider: AccountId32::new([3; 32]),
            kind: JobKind::Inference,
            model: ModelId([4; 32]),
            request_id: [id; 32],
            in_tokens: 1,
            out_tokens: 1,
            fee: MicroUsd(1),
            toploc_commit: [0; 32],
            ttft_ms: 1,
            total_ms: 2,
        };
        let sig = key
            .sign(
                &body.payload().unwrap(),
                RECEIPT_CONTEXT,
                &mut OsRng::new().unwrap(),
            )
            .unwrap();
        SignedReceipt {
            body,
            provider_key: key.public_key().unwrap(),
            provider_sig: sig.clone(),
            gateway_key: key.public_key().unwrap(),
            gateway_sig: sig,
        }
    }

    // Receipts survive a restart and are pruned by block.
    #[test]
    fn persists_across_restarts_and_prunes() {
        let dir = std::env::temp_dir().join(format!("ac-provider-store-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let store = Store::open(&dir).unwrap();
        store.save(&receipt(1), 10).unwrap();
        store.save(&receipt(2), 20).unwrap();
        drop(store);
        let reopened = Store::open(&dir).unwrap();
        let mut listed: Vec<(u64, u8)> = reopened
            .list()
            .unwrap()
            .into_iter()
            .map(|(b, r, _)| (b, r.body.request_id[0]))
            .collect();
        listed.sort_unstable();
        assert_eq!(listed, [(10, 1), (20, 2)]);
        assert_eq!(
            listed.first().map(|(_, r)| receipt(*r).body.request_id),
            Some([1; 32])
        );
        assert_eq!(reopened.prune(15).unwrap(), 1);
        assert_eq!(reopened.list().unwrap().len(), 1);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
