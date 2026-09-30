//! Co-signed receipts kept with their TOPLOC proofs until their challenge period is over (spec
//! "费用计算与收据签名"). Files hold receipts and proofs only: never prompts, outputs or
//! activations.

use std::path::{Path, PathBuf};

use ac_market_proto::toploc::ToplocProofs;
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

    /// Saves a receipt and its proofs, stamped with the block it was stored at (write, then
    /// rename); pruning deletes both.
    ///
    /// # Errors
    ///
    /// Write failures.
    pub fn save(
        &self,
        receipt: &SignedReceipt,
        toploc: Option<&ToplocProofs>,
        block: u64,
    ) -> Result<()> {
        let name = hex::encode(receipt.body.request_id);
        let tmp = self.dir.join(format!("{name}.tmp"));
        let mut body =
            json!({ "block": block, "receipt": format!("0x{}", hex::encode(receipt.encode())) });
        if let (Some(p), Some(obj)) = (toploc, body.as_object_mut()) {
            obj.insert(
                "toploc".into(),
                format!("0x{}", hex::encode(p.encode())).into(),
            );
        }
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

    /// The TOPLOC proofs stored with a receipt (`None` for a receipt without proofs, or one
    /// stored before proofs existed).
    ///
    /// # Errors
    ///
    /// If no receipt with that request ID is stored.
    pub fn toploc_of(&self, request_id: &[u8; 32]) -> Result<Option<ToplocProofs>> {
        let path = self.dir.join(format!("{}.json", hex::encode(request_id)));
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("reading {}", path.display()))?;
        let v: Value = serde_json::from_str(&text).context("malformed receipt file")?;
        let Some(hex_proofs) = v.get("toploc").and_then(Value::as_str) else {
            return Ok(None);
        };
        let bytes = hex::decode(hex_proofs.trim_start_matches("0x")).context("malformed proofs")?;
        Ok(Some(
            ToplocProofs::decode(&mut &bytes[..]).context("malformed proofs")?,
        ))
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
        store.save(&receipt(1), None, 10).unwrap();
        store.save(&receipt(2), None, 20).unwrap();
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

    // Scenario "证明随收据保存与删除".
    #[test]
    fn proofs_are_kept_and_pruned_with_their_receipt() {
        let dir = std::env::temp_dir().join(format!("ac-provider-proofs-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let store = Store::open(&dir).unwrap();
        let proofs = ToplocProofs {
            decode_batching_size: 32,
            topk: 128,
            skip_prefill: false,
            proofs: vec![vec![0xff, 0xd9, 1, 2]],
        };
        store.save(&receipt(3), Some(&proofs), 10).unwrap();
        store.save(&receipt(4), None, 10).unwrap();
        assert_eq!(store.toploc_of(&[3; 32]).unwrap(), Some(proofs));
        assert_eq!(store.toploc_of(&[4; 32]).unwrap(), None);
        assert_eq!(store.prune(11).unwrap(), 2);
        assert!(store.toploc_of(&[3; 32]).is_err());
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
