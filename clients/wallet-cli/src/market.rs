//! Inference-market commands (m5-market-registry design D10): input parsing, market calls,
//! `MarketApi` queries and vouchers.

use anyhow::{Context, Result, anyhow, bail};
use parity_scale_codec::{Decode, Encode};
use serde::Deserialize;
use sp_runtime::AccountId32;

use ac_crypto::KemPublicKey;
use ac_primitives::market::model::{Lineage, LineageKind, QuantType};
use ac_primitives::market::records::{ModelPrice, Tier};
use ac_primitives::market::voucher::VOUCHER_CONTEXT;
use ac_primitives::market::{
    AtcPerUsd, ChannelRecord, GatewayRecord, MicroUsd, ModelId, ModelManifest, ModelRecord,
    PricePerMTok, ProviderRecord, SignedVoucher, VoucherBody, VoucherCheck, VoucherError,
};
use ac_runtime::{Balance, BlockNumber};

use crate::client::NodeClient;
use crate::wallet::Wallet;

/// Decimal places of dollar amounts (micro-dollars).
pub const USD_DECIMALS: usize = 6;

/// Parses a decimal dollar amount with at most six decimal places (`0.5`, `100`, `.000001`).
///
/// # Errors
///
/// Empty input, signs or other characters, more than six decimals, overflow.
pub fn parse_usd(text: &str) -> Result<MicroUsd> {
    let text = text.trim().trim_start_matches('$');
    let (whole, frac) = text.split_once('.').unwrap_or((text, ""));
    if whole.is_empty() && frac.is_empty() {
        bail!("empty dollar amount");
    }
    if !whole
        .chars()
        .chain(frac.chars())
        .all(|c| c.is_ascii_digit())
    {
        bail!("dollar amount must be a non-negative decimal number: {text}");
    }
    if frac.len() > USD_DECIMALS {
        bail!("at most {USD_DECIMALS} decimal places in dollar amounts: {text}");
    }
    let whole: u128 = if whole.is_empty() {
        0
    } else {
        whole.parse().context("dollar amount too large")?
    };
    let frac_digits = format!("{frac:0<width$}", width = USD_DECIMALS);
    let frac: u128 = frac_digits.parse().context("bad fraction")?;
    whole
        .checked_mul(1_000_000)
        .and_then(|w| w.checked_add(frac))
        .map(MicroUsd)
        .context("dollar amount too large")
}

/// Formats micro-dollars as `$1.25`.
#[must_use]
pub fn format_usd(amount: MicroUsd) -> String {
    let (whole, frac) = (amount.0 / 1_000_000, amount.0 % 1_000_000);
    if frac == 0 {
        return format!("${whole}");
    }
    format!("${whole}.{}", format!("{frac:06}").trim_end_matches('0'))
}

fn parse_hex32(text: &str) -> Result<[u8; 32]> {
    let bytes = hex::decode(text.trim().trim_start_matches("0x")).context("bad hex")?;
    <[u8; 32]>::try_from(bytes).map_err(|_| anyhow!("expected 32 bytes: {text}"))
}

/// Parses a model ID (`0x` + 64 hex digits).
///
/// # Errors
///
/// Bad hex or length.
pub fn parse_model_id(text: &str) -> Result<ModelId> {
    parse_hex32(text).map(ModelId)
}

/// Parses a tier: `t1` or `t2` (`t0` is reserved and refused by the chain).
///
/// # Errors
///
/// Unknown names.
pub fn parse_tier(text: &str) -> Result<Tier> {
    match text.to_ascii_lowercase().as_str() {
        "t0" => Ok(Tier::T0),
        "t1" => Ok(Tier::T1),
        "t2" => Ok(Tier::T2),
        _ => bail!("tier must be t1 or t2: {text}"),
    }
}

/// Parses an encryption key: the hex of its AlgId-tagged canonical encoding.
///
/// # Errors
///
/// Bad hex, an unknown algorithm or a wrong length.
pub fn parse_kem_key(text: &str) -> Result<KemPublicKey> {
    let bytes = hex::decode(text.trim().trim_start_matches("0x")).context("bad hex")?;
    KemPublicKey::from_canonical(&bytes).map_err(|e| anyhow!("bad encryption key: {e}"))
}

/// Parses a model price entry `MODEL_ID:INPUT_USD:OUTPUT_USD` (dollars per million tokens).
///
/// # Errors
///
/// A malformed entry.
pub fn parse_model_price(text: &str) -> Result<ModelPrice> {
    let mut parts = text.split(':');
    let (Some(id), Some(input), Some(output), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        bail!("expected MODEL_ID:INPUT_USD:OUTPUT_USD, got {text}");
    };
    Ok(ModelPrice {
        model: parse_model_id(id)?,
        price: PricePerMTok {
            input: parse_usd(input)?,
            output: parse_usd(output)?,
        },
    })
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct LineageFile {
    parent: String,
    kind: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ManifestFile {
    name: String,
    arch: String,
    quant: String,
    shards: Vec<String>,
    #[serde(default)]
    lineage: Option<LineageFile>,
    #[serde(default)]
    license_tag: String,
}

/// A model registration read from a manifest file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelFile {
    /// The manifest.
    pub manifest: ModelManifest,
    /// Declared parent, if any.
    pub lineage: Option<Lineage>,
    /// Informational licence tag.
    pub license_tag: Vec<u8>,
}

impl ModelFile {
    /// The model ID the chain will compute.
    ///
    /// # Errors
    ///
    /// An invalid manifest.
    pub fn id(&self) -> Result<ModelId> {
        self.manifest.id().map_err(|e| anyhow!("{e}"))
    }
}

/// Parses a manifest file:
/// `{ "name", "arch", "quant": "int4", "shards": ["0x…"], "lineage": { "parent", "kind" }?,
/// "licenseTag"? }`.
///
/// # Errors
///
/// Malformed JSON, unknown formats or kinds, bad hashes or bounds.
pub fn parse_manifest(json: &str) -> Result<ModelFile> {
    let file: ManifestFile = serde_json::from_str(json).context("bad manifest JSON")?;
    let quant = match file.quant.to_ascii_lowercase().as_str() {
        "bf16" => QuantType::Bf16,
        "fp16" => QuantType::Fp16,
        "fp8" => QuantType::Fp8,
        "int8" => QuantType::Int8,
        "int4" => QuantType::Int4,
        other => bail!("unknown quantization {other}: use bf16, fp16, fp8, int8 or int4"),
    };
    let shards = file
        .shards
        .iter()
        .map(|s| parse_hex32(s))
        .collect::<Result<Vec<_>>>()?;
    let manifest = ModelManifest::new(file.name.as_bytes(), file.arch.as_bytes(), quant, shards)
        .map_err(|e| anyhow!("{e}"))?;
    let lineage = file
        .lineage
        .map(|l| -> Result<Lineage> {
            let kind = match l.kind.to_ascii_lowercase().as_str() {
                "finetune" => LineageKind::Finetune,
                "quantize" => LineageKind::Quantize,
                "distill" => LineageKind::Distill,
                "merge" => LineageKind::Merge,
                other => {
                    bail!("unknown lineage kind {other}: use finetune, quantize, distill or merge")
                }
            };
            Ok(Lineage {
                parent: parse_model_id(&l.parent)?,
                kind,
            })
        })
        .transpose()?;
    if file.license_tag.len() > 64 {
        bail!("the licence tag is longer than 64 bytes");
    }
    Ok(ModelFile {
        manifest,
        lineage,
        license_tag: file.license_tag.into_bytes(),
    })
}

/// Signs a voucher: the channel's cumulative authorization for `gateway` of `cumulative`
/// dollars, with the wallet's current key.
///
/// # Errors
///
/// Wrong password, RPC failures.
pub async fn sign_voucher(
    client: &NodeClient,
    wallet: &Wallet,
    password: &[u8],
    gateway: &AccountId32,
    cumulative: MicroUsd,
) -> Result<SignedVoucher> {
    let user = wallet.account()?;
    let channel = client
        .market_channel(&user, gateway)
        .await?
        .map_or(0, |c| c.number);
    let body = VoucherBody {
        genesis: client.chain_context().await?.genesis_hash,
        user,
        gateway: gateway.clone(),
        channel,
        cumulative,
    };
    let key = wallet.current_key(password)?;
    let mut rng = ac_crypto::OsRng::new()?;
    let signature = key.sign(&body.payload()?, VOUCHER_CONTEXT, &mut rng)?;
    Ok(SignedVoucher {
        body,
        public_key: key.public_key()?,
        signature,
    })
}

/// Decodes a voucher printed by `voucher sign` (hex of its SCALE encoding).
///
/// # Errors
///
/// Bad hex or encoding.
pub fn decode_voucher(text: &str) -> Result<SignedVoucher> {
    let bytes = hex::decode(text.trim().trim_start_matches("0x")).context("bad hex")?;
    SignedVoucher::decode(&mut &bytes[..]).context("not a voucher")
}

/// A provider record as the chain stores it.
pub type Provider = ProviderRecord<Balance, BlockNumber>;
/// A gateway record.
pub type Gateway = GatewayRecord<Balance, BlockNumber>;
/// A channel record.
pub type Channel = ChannelRecord<Balance, BlockNumber>;
/// A model record.
pub type Model = ModelRecord<AccountId32, Balance, BlockNumber>;

impl NodeClient {
    async fn market<T: Decode>(&self, method: &str, args: &impl Encode) -> Result<T> {
        let raw = self.call_api(&format!("MarketApi_{method}"), args).await?;
        Ok(T::decode(&mut &raw[..])?)
    }

    /// The reference rate and the block it was set at.
    ///
    /// # Errors
    ///
    /// RPC failures.
    pub async fn market_rate(&self) -> Result<Option<(AtcPerUsd, BlockNumber)>> {
        self.market("atc_per_usd", &()).await
    }

    /// A registered model.
    ///
    /// # Errors
    ///
    /// RPC failures.
    pub async fn market_model(&self, id: ModelId) -> Result<Option<Model>> {
        self.market("model", &id).await
    }

    /// Every registered model ID (paged over the `ModelRegistry::Models` storage map, whose
    /// keys end with the ID).
    ///
    /// # Errors
    ///
    /// RPC failures.
    pub async fn market_model_ids(&self) -> Result<Vec<ModelId>> {
        let prefix = [
            sp_io::hashing::twox_128(b"ModelRegistry"),
            sp_io::hashing::twox_128(b"Models"),
        ]
        .concat();
        let mut ids = Vec::new();
        let mut start: Option<Vec<u8>> = None;
        loop {
            let keys = self
                .storage_keys_paged(&prefix, 512, start.as_deref())
                .await?;
            let full = keys.len() == 512;
            for k in &keys {
                if let Some(Ok(id)) = k.get(prefix.len()..).map(<[u8; 32]>::try_from) {
                    ids.push(ModelId(id));
                }
            }
            start = keys.last().cloned();
            if !full {
                return Ok(ids);
            }
        }
    }

    /// A registered provider.
    ///
    /// # Errors
    ///
    /// RPC failures.
    pub async fn market_provider(&self, who: &AccountId32) -> Result<Option<Provider>> {
        self.market("provider", who).await
    }

    /// Whether `who` is serviceable now.
    ///
    /// # Errors
    ///
    /// RPC failures.
    pub async fn market_is_serviceable(&self, who: &AccountId32) -> Result<bool> {
        self.market("is_serviceable", who).await
    }

    /// Every serviceable provider of `model` (all pages).
    ///
    /// # Errors
    ///
    /// RPC failures.
    pub async fn market_serviceable(&self, model: ModelId) -> Result<Vec<(AccountId32, Provider)>> {
        let mut all = Vec::new();
        let mut after: Option<AccountId32> = None;
        loop {
            let page: Vec<(AccountId32, Provider)> = self
                .market("serviceable_providers", &(model, after.clone(), 256u32))
                .await?;
            let full = page.len() == 256;
            after = page.last().map(|(w, _)| w.clone());
            all.extend(page);
            if !full {
                return Ok(all);
            }
        }
    }

    /// Stake a provider of `tier` needs now.
    ///
    /// # Errors
    ///
    /// RPC failures.
    pub async fn market_provider_threshold(
        &self,
        tier: Tier,
    ) -> Result<Result<Balance, ac_primitives::market::PriceError>> {
        self.market("provider_threshold", &tier).await
    }

    /// A registered gateway.
    ///
    /// # Errors
    ///
    /// RPC failures.
    pub async fn market_gateway(&self, who: &AccountId32) -> Result<Option<Gateway>> {
        self.market("gateway", who).await
    }

    /// Stake a gateway needs now.
    ///
    /// # Errors
    ///
    /// RPC failures.
    pub async fn market_gateway_threshold(
        &self,
    ) -> Result<Result<Balance, ac_primitives::market::PriceError>> {
        self.market("gateway_threshold", &()).await
    }

    /// `user`'s channel with `gateway`.
    ///
    /// # Errors
    ///
    /// RPC failures.
    pub async fn market_channel(
        &self,
        user: &AccountId32,
        gateway: &AccountId32,
    ) -> Result<Option<Channel>> {
        self.market("channel", &(user, gateway)).await
    }

    /// Checks a voucher as redemption would.
    ///
    /// # Errors
    ///
    /// RPC failures.
    pub async fn market_check_voucher(
        &self,
        voucher: &SignedVoucher,
    ) -> Result<Result<VoucherCheck, VoucherError>> {
        self.market("check_voucher", voucher).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dollar_amounts_parse_and_format() {
        assert_eq!(parse_usd("0.5").unwrap(), MicroUsd(500_000));
        assert_eq!(parse_usd("100").unwrap(), MicroUsd(100_000_000));
        assert_eq!(parse_usd("$1.25").unwrap(), MicroUsd(1_250_000));
        assert_eq!(parse_usd(".000001").unwrap(), MicroUsd(1));
        // Spec clients/wallet-cli, Scenario "美元格式错误".
        assert!(parse_usd("0.0000001").is_err());
        assert!(parse_usd("-1").is_err());
        assert!(parse_usd("").is_err());
        assert!(parse_usd("1e3").is_err());
        assert_eq!(format_usd(MicroUsd(1_250_000)), "$1.25");
        assert_eq!(format_usd(MicroUsd(100_000_000)), "$100");
    }

    #[test]
    fn tiers_prices_and_keys_parse() {
        assert_eq!(parse_tier("T2").unwrap(), Tier::T2);
        assert!(parse_tier("t3").is_err());
        let id = format!("0x{}", "11".repeat(32));
        let entry = parse_model_price(&format!("{id}:0.1:0.2")).unwrap();
        assert_eq!(entry.model, ModelId([0x11; 32]));
        assert_eq!(entry.price.input, MicroUsd(100_000));
        assert_eq!(entry.price.output, MicroUsd(200_000));
        assert!(parse_model_price("0x11:1").is_err());
        let key = KemPublicKey::new(ac_crypto::KemAlg::XWing, &[7; 1216]).unwrap();
        let text = format!("0x{}", hex::encode(key.to_canonical()));
        assert_eq!(parse_kem_key(&text).unwrap(), key);
        assert!(parse_kem_key("0x02").is_err());
    }

    // The locally computed ID equals the published vector (spec "本地模型 ID 与链上一致" is
    // checked against a node in the e2e test).
    #[test]
    fn a_manifest_file_gives_the_published_id() {
        let json = format!(
            r#"{{"name":"Qwen2.5-0.5B-Instruct","arch":"qwen2","quant":"int4",
                "shards":["0x{}","0x{}"],"licenseTag":"apache-2.0"}}"#,
            "01".repeat(32),
            "02".repeat(32)
        );
        let file = parse_manifest(&json).unwrap();
        assert_eq!(
            format!("{:?}", file.id().unwrap()),
            "0x226bb1bf7ef7963e7bc8f666002192b287323e1343ab59f9f261cafa0fa8a1ab"
        );
        assert_eq!(file.license_tag, b"apache-2.0");
        let with_parent = json.replace(
            r#""licenseTag""#,
            &format!(
                r#""lineage":{{"parent":"0x{}","kind":"quantize"}},"licenseTag""#,
                "aa".repeat(32)
            ),
        );
        assert_eq!(
            parse_manifest(&with_parent).unwrap().lineage,
            Some(Lineage {
                parent: ModelId([0xaa; 32]),
                kind: LineageKind::Quantize
            })
        );
        assert!(parse_manifest(&json.replace("int4", "int3")).is_err());
        assert!(parse_manifest(r#"{"name":"m","arch":"a","quant":"fp8","shards":[]}"#).is_err());
        assert!(
            parse_manifest(r#"{"name":"m","arch":"a","quant":"fp8","shards":["0x01"]}"#).is_err()
        );
    }

    #[test]
    fn vouchers_round_trip_through_hex() {
        let key = ac_crypto::sig::SigningKey::from_seed(
            ac_crypto::SigAlg::MlDsa44,
            &ac_crypto::dev_seed("alice").unwrap(),
        )
        .unwrap();
        let body = VoucherBody {
            genesis: sp_core::H256::zero(),
            user: AccountId32::new([1; 32]),
            gateway: AccountId32::new([2; 32]),
            channel: 0,
            cumulative: MicroUsd(500_000),
        };
        let mut rng = ac_crypto::OsRng::new().unwrap();
        let v = SignedVoucher {
            signature: key
                .sign(&body.payload().unwrap(), VOUCHER_CONTEXT, &mut rng)
                .unwrap(),
            public_key: key.public_key().unwrap(),
            body,
        };
        let text = format!("0x{}", hex::encode(v.encode()));
        assert_eq!(decode_voucher(&text).unwrap(), v);
        assert!(decode_voucher("0x00").is_err());
    }
}
