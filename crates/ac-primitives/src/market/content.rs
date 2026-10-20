//! Content commitment of receipts v2 (m6-token-binding design D2; spec `market/work-settlement`,
//! "收据").
//!
//! ```text
//! content_commit = derive("agentcoin 2026-10 receipt-content v1",
//!                         SCALE(salt: [u8; 32], messages: Vec<Vec<u8>>,
//!                               prompt_ids: Vec<u32>, output_ids: Vec<u32>))
//! ```
//!
//! Each message is committed as its canonical JSON text ([`canonical_json`], with `std`): keys
//! in byte order, no whitespace, strings with the minimal JSON escapes and every other character
//! as UTF-8, integers in decimal. Gateways, wallets and auditors each compute it from the
//! messages they parsed, so the gateway re-serializing a request does not change it. The salt
//! keeps the commitment, which goes on chain with a verdict, from confirming a guessed prompt
//! (D20).
//!
//! Python computes the same commitment (`scripts/calibrate-toploc.py`); both check
//! `tests/vectors/receipt_content.json`.

use alloc::vec::Vec;
use parity_scale_codec::Encode;

/// Hashing context of the content commitment.
pub const RECEIPT_CONTENT_CONTEXT: &str = "agentcoin 2026-10 receipt-content v1";

/// The content commitment of a request: its salt, its messages as canonical JSON texts, and the
/// prompt and output token IDs the engine computed (the output IDs include the end token).
///
/// # Errors
///
/// Only if the published hashing context were rejected, which does not happen.
pub fn content_commit(
    salt: &[u8; 32],
    messages: &[Vec<u8>],
    prompt_ids: &[u32],
    output_ids: &[u32],
) -> Result<[u8; 32], ac_crypto::Error> {
    let encoded = (salt, messages, prompt_ids, output_ids).encode();
    ac_crypto::hash::derive(RECEIPT_CONTENT_CONTEXT, &encoded)
}

#[cfg(feature = "std")]
pub use json::{CanonicalJsonError, canonical_json, canonical_messages};

#[cfg(feature = "std")]
mod json {
    use alloc::string::String;
    use alloc::vec::Vec;
    use core::fmt::Write;

    use serde_json::Value;

    /// Why a value has no canonical JSON text.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    #[non_exhaustive]
    pub enum CanonicalJsonError {
        /// A number that is not an integer: floating-point texts differ between libraries, so
        /// such messages are not committed (the provider then sends no proof).
        NonIntegerNumber,
    }

    impl core::fmt::Display for CanonicalJsonError {
        fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
            match self {
                Self::NonIntegerNumber => {
                    f.write_str("a message holds a non-integer number, which is not committed")
                }
            }
        }
    }

    impl std::error::Error for CanonicalJsonError {}

    /// The canonical JSON text of `value`: object keys sorted by their UTF-8 bytes, no
    /// whitespace, strings escaping only `"`, `\` and control characters (`\b`, `\f`, `\n`,
    /// `\r`, `\t`, else `\u00xx` in lowercase hex), integers in decimal.
    ///
    /// # Errors
    ///
    /// [`CanonicalJsonError::NonIntegerNumber`] for a number that is not an integer.
    pub fn canonical_json(value: &Value) -> Result<Vec<u8>, CanonicalJsonError> {
        let mut out = String::new();
        write_value(&mut out, value)?;
        Ok(out.into_bytes())
    }

    /// The canonical JSON texts of a request's messages, in order.
    ///
    /// # Errors
    ///
    /// As [`canonical_json`], for the first message that has no canonical text.
    pub fn canonical_messages(messages: &[Value]) -> Result<Vec<Vec<u8>>, CanonicalJsonError> {
        messages.iter().map(canonical_json).collect()
    }

    fn write_value(out: &mut String, value: &Value) -> Result<(), CanonicalJsonError> {
        match value {
            Value::Null => out.push_str("null"),
            Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            Value::Number(n) => {
                if let Some(i) = n.as_i64() {
                    let _ = write!(out, "{i}");
                } else if let Some(u) = n.as_u64() {
                    let _ = write!(out, "{u}");
                } else {
                    return Err(CanonicalJsonError::NonIntegerNumber);
                }
            }
            Value::String(s) => write_string(out, s),
            Value::Array(items) => {
                out.push('[');
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    write_value(out, item)?;
                }
                out.push(']');
            }
            Value::Object(map) => {
                let mut entries: Vec<(&String, &Value)> = map.iter().collect();
                entries.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
                out.push('{');
                for (i, (key, item)) in entries.into_iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    write_string(out, key);
                    out.push(':');
                    write_value(out, item)?;
                }
                out.push('}');
            }
        }
        Ok(())
    }

    fn write_string(out: &mut String, s: &str) {
        out.push('"');
        for c in s.chars() {
            match c {
                '"' => out.push_str("\\\""),
                '\\' => out.push_str("\\\\"),
                '\u{08}' => out.push_str("\\b"),
                '\u{0c}' => out.push_str("\\f"),
                '\n' => out.push_str("\\n"),
                '\r' => out.push_str("\\r"),
                '\t' => out.push_str("\\t"),
                c if u32::from(c) < 0x20 => {
                    let _ = write!(out, "\\u{:04x}", u32::from(c));
                }
                c => out.push(c),
            }
        }
        out.push('"');
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn commit(salt: u8, messages: &[&str], prompt: &[u32], output: &[u32]) -> [u8; 32] {
        let messages: Vec<Vec<u8>> = messages.iter().map(|m| m.as_bytes().to_vec()).collect();
        content_commit(&[salt; 32], &messages, prompt, output).unwrap()
    }

    // Scenario "内容承诺可打开": changing a character, a token ID or a salt byte changes it.
    #[test]
    fn every_part_changes_the_commitment() {
        let base = commit(1, &[r#"{"content":"hi","role":"user"}"#], &[1, 2], &[3, 4]);
        assert_eq!(
            base,
            commit(1, &[r#"{"content":"hi","role":"user"}"#], &[1, 2], &[3, 4])
        );
        assert_ne!(
            base,
            commit(1, &[r#"{"content":"ho","role":"user"}"#], &[1, 2], &[3, 4])
        );
        assert_ne!(
            base,
            commit(2, &[r#"{"content":"hi","role":"user"}"#], &[1, 2], &[3, 4])
        );
        assert_ne!(
            base,
            commit(1, &[r#"{"content":"hi","role":"user"}"#], &[1, 2], &[3, 5])
        );
        // Token IDs moved from the prompt to the output are another commitment.
        assert_ne!(
            base,
            commit(1, &[r#"{"content":"hi","role":"user"}"#], &[1], &[2, 3, 4])
        );
    }

    #[cfg(feature = "std")]
    #[test]
    fn canonical_json_rules() {
        let v: serde_json::Value = serde_json::from_str(
            r#"{ "role": "user", "b": [1, -2, true, null], "a": "x\u0001\"\\/é\n" }"#,
        )
        .unwrap();
        assert_eq!(
            String::from_utf8(canonical_json(&v).unwrap()).unwrap(),
            "{\"a\":\"x\\u0001\\\"\\\\/é\\n\",\"b\":[1,-2,true,null],\"role\":\"user\"}"
        );
        let float: serde_json::Value = serde_json::from_str(r#"{"t": 0.5}"#).unwrap();
        assert_eq!(
            canonical_json(&float),
            Err(CanonicalJsonError::NonIntegerNumber)
        );
    }
}
