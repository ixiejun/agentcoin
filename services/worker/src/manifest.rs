//! Data manifests and shards (spec `market/public-worker` "数据清单与分片").
//!
//! A job's manifest is a JSON document whose BLAKE3 hash is on chain: for each unit, the shard's
//! URL, BLAKE3 hash and number of items. Shards are JSON Lines: evaluation items
//! `{"context": …, "choices": [2–16 strings]}`, texts to embed or documents to clean
//! `{"text": …}`. Every byte is checked against its hash before it is used.

use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};

use ac_primitives::market::public::{MAX_CHOICES, MAX_ITEMS};

/// Most documents of a data cleaning unit.
pub const MAX_DOCUMENTS: usize = 4_096;

/// A job's data manifest.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    /// One shard per unit, in unit order.
    pub units: Vec<Shard>,
}

/// One unit's data.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Shard {
    /// Where the shard is served.
    pub url: String,
    /// BLAKE3 of the shard, hexadecimal.
    pub blake3: String,
    /// Number of items (lines).
    pub items: u32,
}

/// An evaluation item: a context and the continuations to choose from.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvalItem {
    /// The context.
    pub context: String,
    /// The candidate continuations (2–16).
    pub choices: Vec<String>,
}

#[derive(Deserialize)]
struct TextLine {
    text: String,
}

/// BLAKE3 of `bytes`.
#[must_use]
pub fn blake3(bytes: &[u8]) -> [u8; 32] {
    ac_crypto::hash::blake3_256(bytes)
}

/// Parses a manifest whose hash must be `expected`.
///
/// # Errors
///
/// A hash mismatch or malformed JSON.
pub fn parse_manifest(bytes: &[u8], expected: &[u8; 32]) -> Result<Manifest> {
    ensure!(
        blake3(bytes) == *expected,
        "the manifest does not match its hash"
    );
    serde_json::from_slice(bytes).context("malformed manifest")
}

/// Checks a shard's bytes against its manifest entry.
///
/// # Errors
///
/// A hash mismatch.
pub fn check_shard(bytes: &[u8], shard: &Shard) -> Result<()> {
    let want = hex::decode(shard.blake3.trim_start_matches("0x")).context("bad shard hash")?;
    ensure!(
        blake3(bytes).as_slice() == want.as_slice(),
        "the shard does not match its hash"
    );
    Ok(())
}

fn lines(bytes: &[u8]) -> Result<Vec<&str>> {
    let text = std::str::from_utf8(bytes).context("the shard is not UTF-8")?;
    Ok(text.lines().filter(|l| !l.trim().is_empty()).collect())
}

/// Parses an evaluation shard.
///
/// # Errors
///
/// Malformed lines, no items or more than 256, or an item without 2–16 choices.
pub fn parse_eval(bytes: &[u8]) -> Result<Vec<EvalItem>> {
    let items: Vec<EvalItem> = lines(bytes)?
        .into_iter()
        .map(|l| serde_json::from_str(l).context("malformed evaluation item"))
        .collect::<Result<_>>()?;
    ensure!(
        !items.is_empty() && items.len() <= MAX_ITEMS,
        "an evaluation unit has 1–256 items"
    );
    for item in &items {
        if item.choices.len() < 2 || item.choices.len() > usize::from(MAX_CHOICES) {
            bail!("an evaluation item has 2–16 choices");
        }
    }
    Ok(items)
}

/// Parses a shard of texts (embedding: at most 256; data cleaning: at most 4,096).
///
/// # Errors
///
/// Malformed lines, no texts or too many.
pub fn parse_texts(bytes: &[u8], max: usize) -> Result<Vec<String>> {
    let texts: Vec<String> = lines(bytes)?
        .into_iter()
        .map(|l| {
            serde_json::from_str::<TextLine>(l)
                .map(|t| t.text)
                .context("malformed text line")
        })
        .collect::<Result<_>>()?;
    ensure!(
        !texts.is_empty() && texts.len() <= max,
        "a unit has 1–{max} texts"
    );
    Ok(texts)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)] // Test code.

    use super::*;

    // Spec "分片被篡改".
    #[test]
    fn tampered_shards_and_manifests_are_refused() {
        let shard = b"{\"text\":\"hello\"}\n";
        let entry = Shard {
            url: "http://x/0".into(),
            blake3: hex::encode(blake3(shard)),
            items: 1,
        };
        check_shard(shard, &entry).unwrap();
        assert!(check_shard(b"{\"text\":\"hellO\"}\n", &entry).is_err());
        let m = serde_json::to_vec(&Manifest {
            units: vec![entry],
        })
        .unwrap();
        parse_manifest(&m, &blake3(&m)).unwrap();
        assert!(parse_manifest(&m, &[0; 32]).is_err());
    }

    #[test]
    fn shard_formats() {
        let eval = b"{\"context\":\"2+2=\",\"choices\":[\" 4\",\" 5\"]}\n";
        assert_eq!(parse_eval(eval).unwrap()[0].choices.len(), 2);
        assert!(parse_eval(b"{\"context\":\"x\",\"choices\":[\"a\"]}\n").is_err());
        assert_eq!(parse_texts(b"{\"text\":\"a\"}\n\n{\"text\":\"b\"}\n", 10).unwrap().len(), 2);
        assert!(parse_texts(b"", 10).is_err());
        assert!(parse_texts(b"not json\n", 10).is_err());
    }
}
