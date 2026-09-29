//! Work settlement for the wallet (spec `clients/wallet-cli`, requirement "工作结算子命令"):
//! receipt files, building a work report from receipts and vouchers, and `WorkApi` queries.
//!
//! A receipt file is JSON: the SCALE-encoded receipt body in hex, plus the provider's and the
//! gateway's key and signature once they signed, and readable copies of the main fields.

use std::collections::BTreeMap;

use anyhow::{Context, Result, bail};
use parity_scale_codec::{Decode, Encode};
use sp_runtime::AccountId32;

use ac_crypto::sig::SigningKey;
use ac_crypto::{PqPublicKey, PqSignature};
use ac_primitives::emission::EpochIndex;
use ac_primitives::encode_address;
use ac_primitives::market::receipt::{
    RECEIPT_CONTEXT, ReceiptBody, ReceiptContext, SignedReceipt, check_receipt,
};
use ac_primitives::market::receipt_tree;
use ac_primitives::market::work::JobKind;
use ac_primitives::market::{
    EpochWork, Held, LifetimeWork, MicroUsd, ModelId, PricePerMTok, ProviderWork, ReportEntry,
    ReportRecord, SignedVoucher, WorkParams,
};
use ac_runtime::Balance;

use crate::NodeClient;

/// One party's signature in a receipt file.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Party {
    /// Hex of the AlgId-tagged public key.
    pub key: String,
    /// Hex of the AlgId-tagged signature.
    pub signature: String,
}

/// A receipt file.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ReceiptFile {
    /// Hex of the SCALE-encoded [`ReceiptBody`]; the only field that counts.
    pub body: String,
    /// Provider's signature, once signed.
    pub provider: Option<Party>,
    /// Gateway's signature, once signed.
    pub gateway: Option<Party>,
    /// Readable copy of the main fields (ignored when loading).
    #[serde(default)]
    pub summary: BTreeMap<String, String>,
}

fn hex0x(bytes: &[u8]) -> String {
    format!("0x{}", hex::encode(bytes))
}

fn unhex(text: &str) -> Result<Vec<u8>> {
    hex::decode(text.trim().trim_start_matches("0x")).context("bad hex")
}

impl ReceiptFile {
    /// An unsigned receipt file for `body`.
    #[must_use]
    pub fn new(body: &ReceiptBody) -> Self {
        let mut summary = BTreeMap::new();
        summary.insert("gateway".into(), encode_address(body.gateway.as_ref()));
        summary.insert("provider".into(), encode_address(body.provider.as_ref()));
        summary.insert("model".into(), format!("{:?}", body.model));
        summary.insert("fee".into(), crate::market::format_usd(body.fee));
        summary.insert("inTokens".into(), body.in_tokens.to_string());
        summary.insert("outTokens".into(), body.out_tokens.to_string());
        summary.insert("requestId".into(), hex0x(&body.request_id));
        Self {
            body: hex0x(&body.encode()),
            provider: None,
            gateway: None,
            summary,
        }
    }

    /// The receipt body.
    ///
    /// # Errors
    ///
    /// Bad hex or encoding.
    pub fn body(&self) -> Result<ReceiptBody> {
        let bytes = unhex(&self.body)?;
        ReceiptBody::decode(&mut &bytes[..]).context("not a receipt body")
    }

    /// Signs as whichever party `key`'s account is (provider or gateway).
    ///
    /// # Errors
    ///
    /// The account is neither party; signing fails.
    pub fn sign(&mut self, account: &AccountId32, key: &SigningKey) -> Result<()> {
        let body = self.body()?;
        let payload = body.payload()?;
        let mut rng = ac_crypto::OsRng::new()?;
        let party = Party {
            key: hex0x(&key.public_key()?.to_canonical()),
            signature: hex0x(
                &key.sign(&payload, RECEIPT_CONTEXT, &mut rng)?
                    .to_canonical(),
            ),
        };
        if *account == body.provider {
            self.provider = Some(party.clone());
        }
        if *account == body.gateway {
            self.gateway = Some(party);
        }
        if *account != body.provider && *account != body.gateway {
            bail!(
                "{} is neither the receipt's provider nor its gateway",
                encode_address(account.as_ref())
            );
        }
        Ok(())
    }

    /// The receipt with both signatures.
    ///
    /// # Errors
    ///
    /// A missing signature, bad hex or encoding.
    pub fn signed(&self) -> Result<SignedReceipt> {
        let decode_party = |p: &Option<Party>, who: &str| -> Result<(PqPublicKey, PqSignature)> {
            let p = p
                .as_ref()
                .with_context(|| format!("the {who} has not signed"))?;
            let key = PqPublicKey::from_canonical(&unhex(&p.key)?)
                .map_err(|e| anyhow::anyhow!("bad {who} key: {e}"))?;
            let sig = PqSignature::from_canonical(&unhex(&p.signature)?)
                .map_err(|e| anyhow::anyhow!("bad {who} signature: {e}"))?;
            Ok((key, sig))
        };
        let (provider_key, provider_sig) = decode_party(&self.provider, "provider")?;
        let (gateway_key, gateway_sig) = decode_party(&self.gateway, "gateway")?;
        Ok(SignedReceipt {
            body: self.body()?,
            provider_key,
            provider_sig,
            gateway_key,
            gateway_sig,
        })
    }

    /// Reads a receipt file.
    ///
    /// # Errors
    ///
    /// I/O or JSON errors, naming the file.
    pub fn load(path: &std::path::Path) -> Result<Self> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("cannot read {}", path.display()))?;
        serde_json::from_str(&text).with_context(|| format!("{} is not a receipt", path.display()))
    }

    /// Writes the file.
    ///
    /// # Errors
    ///
    /// I/O errors.
    pub fn save(&self, path: &std::path::Path) -> Result<()> {
        std::fs::write(path, serde_json::to_string_pretty(self)? + "\n")
            .with_context(|| format!("cannot write {}", path.display()))
    }
}

/// Chain facts a receipt is checked against.
#[derive(Clone, Debug)]
pub struct ReceiptFacts {
    /// Genesis hash.
    pub genesis: sp_core::H256,
    /// Fingerprint of the provider's registered key.
    pub provider_key: [u8; 32],
    /// Fingerprint of the gateway's registered key.
    pub gateway_key: [u8; 32],
    /// The provider's price for the receipt's model.
    pub price: Option<PricePerMTok>,
}

/// Checks a signed receipt against chain facts.
///
/// # Errors
///
/// The receipt's first broken rule.
pub fn check(receipt: &SignedReceipt, facts: &ReceiptFacts) -> Result<()> {
    check_receipt(
        receipt,
        &ReceiptContext {
            genesis: &facts.genesis,
            provider_key: &facts.provider_key,
            gateway_key: &facts.gateway_key,
            price: facts.price.as_ref(),
        },
    )
    .map_err(|e| anyhow::anyhow!("{e}"))
}

/// A work report ready to submit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Report {
    /// Merkle root of the receipts, in the given order.
    pub root: [u8; 32],
    /// Number of receipts.
    pub receipt_count: u32,
    /// Totals per (provider, model), in first-seen order.
    pub entries: Vec<ReportEntry<AccountId32>>,
}

/// Builds a report from named receipts of `gateway`: the Merkle root and the totals.
///
/// # Errors
///
/// Receipts of another gateway, a non-inference receipt, no receipts, or overflow; the error
/// names the file.
pub fn build_report(gateway: &AccountId32, receipts: &[(String, SignedReceipt)]) -> Result<Report> {
    if receipts.is_empty() {
        bail!("a report needs at least one receipt");
    }
    let mut leaves = Vec::with_capacity(receipts.len());
    let mut order: Vec<(AccountId32, ModelId)> = Vec::new();
    let mut totals: BTreeMap<(AccountId32, ModelId), (u128, u64, u64)> = BTreeMap::new();
    for (name, r) in receipts {
        let b = &r.body;
        if b.gateway != *gateway {
            bail!("{name}: the receipt belongs to another gateway");
        }
        if b.kind != JobKind::Inference {
            bail!("{name}: only inference receipts can be settled");
        }
        leaves.push(receipt_tree::leaf(r).map_err(|e| anyhow::anyhow!("{name}: {e}"))?);
        let key = (b.provider.clone(), b.model);
        if !totals.contains_key(&key) {
            order.push(key.clone());
        }
        let t = totals.entry(key).or_insert((0, 0, 0));
        t.0 = t.0.checked_add(b.fee.0).context("fee overflow")?;
        t.1 = t.1.saturating_add(u64::from(b.in_tokens));
        t.2 = t.2.saturating_add(u64::from(b.out_tokens));
    }
    let root = receipt_tree::root(&leaves).map_err(|e| anyhow::anyhow!("{e}"))?;
    let entries = order
        .into_iter()
        .filter_map(|key| {
            let (usd, i, o) = *totals.get(&key)?;
            Some(ReportEntry {
                kind: JobKind::Inference,
                provider: key.0,
                model: key.1,
                usd: MicroUsd(usd),
                in_tokens: i,
                out_tokens: o,
            })
        })
        .collect();
    Ok(Report {
        root,
        receipt_count: u32::try_from(receipts.len()).context("too many receipts")?,
        entries,
    })
}

/// Checks locally, before submitting, that the entries' dollars equal the vouchers'
/// increments (each increment as the chain's voucher check reports it).
///
/// # Errors
///
/// The totals differ; the message gives both.
pub fn check_totals(report: &Report, increments: &[(String, MicroUsd)]) -> Result<()> {
    let entries: u128 = report.entries.iter().map(|e| e.usd.0).sum();
    let vouchers: u128 = increments.iter().map(|(_, u)| u.0).sum();
    if entries != vouchers {
        let detail: Vec<String> = increments
            .iter()
            .map(|(n, u)| format!("{n}: {}", crate::market::format_usd(*u)))
            .collect();
        bail!(
            "the receipts total {} but the vouchers authorize {} more ({})",
            crate::market::format_usd(MicroUsd(entries)),
            crate::market::format_usd(MicroUsd(vouchers)),
            detail.join(", ")
        );
    }
    Ok(())
}

/// A report as the chain stores it.
pub type StoredReport = ReportRecord<AccountId32, Balance>;

impl NodeClient {
    async fn work_api<T: Decode>(&self, method: &str, args: &impl Encode) -> Result<T> {
        let raw = self.call_api(&format!("WorkApi_{method}"), args).await?;
        Ok(T::decode(&mut &raw[..])?)
    }

    /// Settlement parameters.
    ///
    /// # Errors
    ///
    /// RPC failures.
    pub async fn work_params(&self) -> Result<WorkParams> {
        self.work_api("params", &()).await
    }

    /// A report.
    ///
    /// # Errors
    ///
    /// RPC failures.
    pub async fn work_report(&self, id: u64) -> Result<Option<StoredReport>> {
        self.work_api("report", &id).await
    }

    /// What gateways hold for `who`.
    ///
    /// # Errors
    ///
    /// RPC failures.
    pub async fn work_held(
        &self,
        who: &AccountId32,
    ) -> Result<Vec<(EpochIndex, AccountId32, Held<Balance>)>> {
        self.work_api("held", who).await
    }

    /// `who`'s unclaimed work by epoch.
    ///
    /// # Errors
    ///
    /// RPC failures.
    pub async fn work_of(&self, who: &AccountId32) -> Result<Vec<(EpochIndex, ProviderWork)>> {
        self.work_api("work", who).await
    }

    /// Verified work of an epoch.
    ///
    /// # Errors
    ///
    /// RPC failures.
    pub async fn work_epoch(&self, epoch: EpochIndex) -> Result<EpochWork<Balance>> {
        self.work_api("epoch_work", &epoch).await
    }

    /// `who`'s lifetime work.
    ///
    /// # Errors
    ///
    /// RPC failures.
    pub async fn work_lifetime(&self, who: &AccountId32) -> Result<LifetimeWork<Balance>> {
        self.work_api("lifetime", who).await
    }

    /// Chain facts to check `body` against: genesis, both parties' key fingerprints and the
    /// provider's price for the model.
    ///
    /// # Errors
    ///
    /// RPC failures; a party without a registered key.
    pub async fn receipt_facts(&self, body: &ReceiptBody) -> Result<ReceiptFacts> {
        use ac_primitives::market::voucher::key_fingerprint;
        let genesis = self.chain_context().await?.genesis_hash;
        let fingerprint = |who: &AccountId32, k: Option<(PqPublicKey, u32)>| -> Result<[u8; 32]> {
            let (key, _) = k.with_context(|| {
                format!("{} has no registered key", encode_address(who.as_ref()))
            })?;
            Ok(key_fingerprint(&key))
        };
        let provider_key = fingerprint(&body.provider, self.current_key(&body.provider).await?)?;
        let gateway_key = fingerprint(&body.gateway, self.current_key(&body.gateway).await?)?;
        let price = self.market_provider(&body.provider).await?.and_then(|p| {
            p.models
                .iter()
                .find(|m| m.model == body.model)
                .map(|m| m.price)
        });
        Ok(ReceiptFacts {
            genesis,
            provider_key,
            gateway_key,
            price,
        })
    }
}

/// Decodes vouchers given as hex (the output of `voucher sign`) or as files holding it.
///
/// # Errors
///
/// Unreadable files or bad vouchers, naming the argument.
pub fn load_vouchers(args: &[String]) -> Result<Vec<(String, SignedVoucher)>> {
    args.iter()
        .map(|a| {
            let path = std::path::Path::new(a);
            let text = if path.exists() {
                std::fs::read_to_string(path).with_context(|| format!("cannot read {a}"))?
            } else {
                a.clone()
            };
            let v = crate::market::decode_voucher(text.trim())
                .with_context(|| format!("{a}: not a voucher"))?;
            Ok((a.clone(), v))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ac_crypto::SigAlg;
    use ac_primitives::market::voucher::key_fingerprint;
    use sp_core::H256;

    fn key(name: &str) -> SigningKey {
        SigningKey::from_seed(SigAlg::MlDsa44, &ac_crypto::dev_seed(name).unwrap()).unwrap()
    }

    fn account(k: &SigningKey) -> AccountId32 {
        AccountId32::new(ac_crypto::account_id(&k.public_key().unwrap()).0)
    }

    fn price() -> PricePerMTok {
        PricePerMTok {
            input: MicroUsd(100_000),
            output: MicroUsd(300_000),
        }
    }

    fn body(provider: &SigningKey, gateway: &SigningKey, request: u8) -> ReceiptBody {
        ReceiptBody {
            genesis: H256::repeat_byte(1),
            gateway: account(gateway),
            provider: account(provider),
            kind: JobKind::Inference,
            model: ModelId([0x42; 32]),
            request_id: [request; 32],
            in_tokens: 1_000,
            out_tokens: 2_000,
            fee: MicroUsd(700),
            toploc_commit: [0; 32],
            ttft_ms: 100,
            total_ms: 900,
        }
    }

    fn facts(provider: &SigningKey, gateway: &SigningKey) -> ReceiptFacts {
        ReceiptFacts {
            genesis: H256::repeat_byte(1),
            provider_key: key_fingerprint(&provider.public_key().unwrap()),
            gateway_key: key_fingerprint(&gateway.public_key().unwrap()),
            price: Some(price()),
        }
    }

    fn signed_file(provider: &SigningKey, gateway: &SigningKey, b: &ReceiptBody) -> ReceiptFile {
        let mut f = ReceiptFile::new(b);
        f.sign(&account(provider), provider).unwrap();
        f.sign(&account(gateway), gateway).unwrap();
        f
    }

    // Task 6.1: both parties sign, `check` passes; a tampered fee is reported.
    #[test]
    fn receipts_are_signed_by_both_and_checked() {
        let (p, g) = (key("bob"), key("charlie"));
        let b = body(&p, &g, 1);
        let mut f = ReceiptFile::new(&b);
        assert!(f.signed().is_err(), "unsigned");
        f.sign(&account(&p), &p).unwrap();
        assert!(f.signed().unwrap_err().to_string().contains("gateway"));
        f.sign(&account(&g), &g).unwrap();
        // Round trip through JSON.
        let f: ReceiptFile = serde_json::from_str(&serde_json::to_string(&f).unwrap()).unwrap();
        check(&f.signed().unwrap(), &facts(&p, &g)).unwrap();
        // Someone else cannot sign.
        assert!(
            f.clone()
                .sign(&account(&key("dave")), &key("dave"))
                .is_err()
        );
        // A tampered fee: the chain's price gives 700.
        let mut tampered = f.clone();
        tampered.body = hex0x(
            &ReceiptBody {
                fee: MicroUsd(600),
                ..b
            }
            .encode(),
        );
        let err = check(&tampered.signed().unwrap(), &facts(&p, &g)).unwrap_err();
        assert!(err.to_string().contains("fee"), "{err}");
    }

    #[test]
    fn reports_total_by_provider_and_model() {
        let (p, q, g) = (key("bob"), key("dave"), key("charlie"));
        let files = [
            signed_file(&p, &g, &body(&p, &g, 1)),
            signed_file(&q, &g, &body(&q, &g, 2)),
            signed_file(&p, &g, &body(&p, &g, 3)),
        ];
        let receipts: Vec<(String, SignedReceipt)> = files
            .iter()
            .enumerate()
            .map(|(i, f)| (format!("r{i}.json"), f.signed().unwrap()))
            .collect();
        let r = build_report(&account(&g), &receipts).unwrap();
        assert_eq!(r.receipt_count, 3);
        assert_eq!(r.entries.len(), 2);
        assert_eq!(r.entries[0].provider, account(&p));
        assert_eq!(r.entries[0].usd, MicroUsd(1_400));
        assert_eq!(r.entries[0].in_tokens, 2_000);
        assert_eq!(r.entries[1].usd, MicroUsd(700));
        let leaves: Vec<_> = receipts
            .iter()
            .map(|(_, r)| receipt_tree::leaf(r).unwrap())
            .collect();
        assert_eq!(r.root, receipt_tree::root(&leaves).unwrap());
        check_totals(&r, &[("v1".into(), MicroUsd(2_100))]).unwrap();
    }

    // Scenario "提交前发现不一致", and errors naming the file at fault.
    #[test]
    fn inconsistencies_are_caught_before_submitting() {
        let (p, g, other) = (key("bob"), key("charlie"), key("eve"));
        let good = signed_file(&p, &g, &body(&p, &g, 1)).signed().unwrap();
        let r = build_report(&account(&g), &[("good.json".into(), good.clone())]).unwrap();
        let err = check_totals(&r, &[("v.hex".into(), MicroUsd(600))]).unwrap_err();
        assert!(err.to_string().contains("v.hex"), "{err}");
        let foreign = signed_file(&p, &other, &body(&p, &other, 2))
            .signed()
            .unwrap();
        let err = build_report(
            &account(&g),
            &[("good.json".into(), good), ("foreign.json".into(), foreign)],
        )
        .unwrap_err();
        assert!(err.to_string().contains("foreign.json"), "{err}");
        assert!(build_report(&account(&g), &[]).is_err());
    }
}
