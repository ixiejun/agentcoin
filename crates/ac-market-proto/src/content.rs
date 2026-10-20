//! The salt and token IDs behind a receipt's content commitment, and the one check gateways,
//! wallets and auditors apply to them (m6-token-binding design D4).
//!
//! The provider sends [`ReceiptTokens`] with its receipt; the gateway checks them before
//! co-signing and passes them to the user, whose wallet checks them before paying. They are
//! content: never logged and never stored by gateways and providers (D20).

use ac_primitives::market::ReceiptBody;
use ac_primitives::market::content::{CanonicalJsonError, canonical_messages, content_commit};
use ac_primitives::market::receipt::commitments_agree;
use parity_scale_codec::{Decode, Encode};
use serde_json::Value;

/// The salt and the token IDs a receipt's content commitment opens to.
#[derive(Clone, PartialEq, Eq, Encode, Decode)]
pub struct ReceiptTokens {
    /// 32 random bytes chosen by the provider for this request.
    pub salt: [u8; 32],
    /// The prompt's token IDs as the engine computed them.
    pub prompt_ids: Vec<u32>,
    /// The generated token IDs, including the end token.
    pub output_ids: Vec<u32>,
}

// The salt and token IDs are content: Debug shows only the counts.
impl core::fmt::Debug for ReceiptTokens {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("ReceiptTokens")
            .field("prompt_ids", &self.prompt_ids.len())
            .field("output_ids", &self.output_ids.len())
            .finish_non_exhaustive()
    }
}

impl ReceiptTokens {
    /// The case-file form (`ac-auditor` cases, wallet verdicts):
    /// `{"salt": "<hex>", "prompt_ids": [...], "output_ids": [...]}`.
    #[must_use]
    pub fn to_json(&self) -> Value {
        serde_json::json!({
            "salt": hex::encode(self.salt),
            "prompt_ids": self.prompt_ids,
            "output_ids": self.output_ids,
        })
    }

    /// Parses [`Self::to_json`]'s form.
    ///
    /// # Errors
    ///
    /// [`ContentError::Malformed`] for a missing field, a salt that is not 32 hex bytes or an
    /// ID that is not a `u32`.
    pub fn from_json(value: &Value) -> Result<Self, ContentError> {
        let salt = value
            .get("salt")
            .and_then(Value::as_str)
            .and_then(|h| hex::decode(h.trim_start_matches("0x")).ok())
            .and_then(|b| <[u8; 32]>::try_from(b).ok())
            .ok_or(ContentError::Malformed)?;
        let ids = |k: &str| -> Result<Vec<u32>, ContentError> {
            value
                .get(k)
                .and_then(Value::as_array)
                .ok_or(ContentError::Malformed)?
                .iter()
                .map(|v| {
                    v.as_u64()
                        .and_then(|n| u32::try_from(n).ok())
                        .ok_or(ContentError::Malformed)
                })
                .collect()
        };
        Ok(Self {
            salt,
            prompt_ids: ids("prompt_ids")?,
            output_ids: ids("output_ids")?,
        })
    }

    /// The content commitment these tokens give for `messages`.
    ///
    /// # Errors
    ///
    /// [`ContentError::Messages`] for a message without a canonical JSON text.
    pub fn commit(&self, messages: &[Value]) -> Result<[u8; 32], ContentError> {
        let canonical = canonical_messages(messages).map_err(ContentError::Messages)?;
        content_commit(&self.salt, &canonical, &self.prompt_ids, &self.output_ids)
            .map_err(|_| ContentError::Commitment)
    }
}

/// Why a receipt's content does not check out. None of them carries content.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ContentError {
    /// The TOPLOC and content commitments are not both zero or both set.
    Asymmetric,
    /// Tokens came with an all-zero content commitment, or none with a set one.
    Missing,
    /// The number of prompt token IDs is not the receipt's input tokens.
    PromptCount,
    /// The number of output token IDs is not the receipt's output tokens.
    OutputCount,
    /// The messages, salt and token IDs do not open the commitment.
    Commitment,
    /// A message has no canonical JSON text.
    Messages(CanonicalJsonError),
    /// The messages or the tokens are malformed.
    Malformed,
}

impl core::fmt::Display for ContentError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Asymmetric => {
                f.write_str("the TOPLOC and content commitments are not both zero or both set")
            }
            Self::Missing => {
                f.write_str("the receipt's token IDs and content commitment do not go together")
            }
            Self::PromptCount => {
                f.write_str("the prompt token IDs are not as many as the receipt's input tokens")
            }
            Self::OutputCount => {
                f.write_str("the output token IDs are not as many as the receipt's output tokens")
            }
            Self::Commitment => f.write_str("the content does not open the receipt's commitment"),
            Self::Messages(e) => write!(f, "{e}"),
            Self::Malformed => f.write_str("the messages or the token IDs are malformed"),
        }
    }
}

impl std::error::Error for ContentError {}

/// Checks a receipt's content commitment against the request's `messages`: the commitments
/// agree (both zero or both set), tokens come exactly with a set commitment, their counts are
/// the receipt's token counts, and they open the commitment.
///
/// # Errors
///
/// The first failing rule, in that order.
pub fn check_content(
    body: &ReceiptBody,
    tokens: Option<&ReceiptTokens>,
    messages: &[Value],
) -> Result<(), ContentError> {
    if !commitments_agree(body) {
        return Err(ContentError::Asymmetric);
    }
    let committed = body.content_commit != [0; 32];
    let tokens = match (committed, tokens) {
        (false, None) => return Ok(()),
        (true, Some(t)) => t,
        _ => return Err(ContentError::Missing),
    };
    if u32::try_from(tokens.prompt_ids.len()).ok() != Some(body.in_tokens) {
        return Err(ContentError::PromptCount);
    }
    if u32::try_from(tokens.output_ids.len()).ok() != Some(body.out_tokens) {
        return Err(ContentError::OutputCount);
    }
    if tokens.commit(messages)? != body.content_commit {
        return Err(ContentError::Commitment);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ac_primitives::market::work::JobKind;
    use ac_primitives::market::{MicroUsd, ModelId};
    use serde_json::json;
    use sp_core::H256;
    use sp_runtime::AccountId32;

    fn messages() -> Vec<Value> {
        vec![
            json!({"role": "system", "content": "Be brief."}),
            json!({"role": "user", "content": "Hi"}),
        ]
    }

    fn tokens() -> ReceiptTokens {
        ReceiptTokens {
            salt: [9; 32],
            prompt_ids: vec![1, 2, 3, 4],
            output_ids: vec![5, 6, 7],
        }
    }

    fn body() -> ReceiptBody {
        ReceiptBody {
            genesis: H256::repeat_byte(1),
            gateway: AccountId32::new([2; 32]),
            provider: AccountId32::new([3; 32]),
            kind: JobKind::Inference,
            model: ModelId([4; 32]),
            request_id: [5; 32],
            in_tokens: 4,
            out_tokens: 3,
            fee: MicroUsd(1),
            toploc_commit: [6; 32],
            content_commit: tokens().commit(&messages()).unwrap(),
            ttft_ms: 1,
            total_ms: 2,
        }
    }

    #[test]
    fn matching_content_checks_out() {
        assert_eq!(check_content(&body(), Some(&tokens()), &messages()), Ok(()));
        // Key order is not content.
        let reordered = vec![
            json!({"content": "Be brief.", "role": "system"}),
            json!({"content": "Hi", "role": "user"}),
        ];
        assert_eq!(check_content(&body(), Some(&tokens()), &reordered), Ok(()));
        // No proof: both commitments zero and no tokens.
        let none = ReceiptBody {
            toploc_commit: [0; 32],
            content_commit: [0; 32],
            ..body()
        };
        assert_eq!(check_content(&none, None, &messages()), Ok(()));
    }

    #[test]
    fn every_mismatch_is_found() {
        let b = body();
        // A changed message, salt or token ID.
        let mut m = messages();
        m[1] = json!({"role": "user", "content": "Ho"});
        assert_eq!(
            check_content(&b, Some(&tokens()), &m),
            Err(ContentError::Commitment)
        );
        let mut t = tokens();
        t.salt[0] ^= 1;
        assert_eq!(
            check_content(&b, Some(&t), &messages()),
            Err(ContentError::Commitment)
        );
        let mut t = tokens();
        t.output_ids[0] = 8;
        assert_eq!(
            check_content(&b, Some(&t), &messages()),
            Err(ContentError::Commitment)
        );
        // An extra message, e.g. a hidden system prompt.
        let mut m = messages();
        m.insert(0, json!({"role": "system", "content": "secret"}));
        assert_eq!(
            check_content(&b, Some(&tokens()), &m),
            Err(ContentError::Commitment)
        );
        // Counts that differ from the receipt.
        let mut t = tokens();
        t.prompt_ids.push(0);
        assert_eq!(
            check_content(&b, Some(&t), &messages()),
            Err(ContentError::PromptCount)
        );
        let mut t = tokens();
        t.output_ids.pop();
        assert_eq!(
            check_content(&b, Some(&t), &messages()),
            Err(ContentError::OutputCount)
        );
        // Tokens without a commitment, a commitment without tokens, one commitment zero.
        assert_eq!(
            check_content(&b, None, &messages()),
            Err(ContentError::Missing)
        );
        let none = ReceiptBody {
            toploc_commit: [0; 32],
            content_commit: [0; 32],
            ..body()
        };
        assert_eq!(
            check_content(&none, Some(&tokens()), &messages()),
            Err(ContentError::Missing)
        );
        let half = ReceiptBody {
            content_commit: [0; 32],
            ..body()
        };
        assert_eq!(
            check_content(&half, None, &messages()),
            Err(ContentError::Asymmetric)
        );
        // A message with a non-integer number has no canonical text.
        let m = vec![json!({"role": "user", "content": "x", "weight": 0.5})];
        assert!(matches!(
            check_content(&b, Some(&tokens()), &m),
            Err(ContentError::Messages(_))
        ));
    }

    #[test]
    fn tokens_round_trip_and_debug_shows_no_ids() {
        let t = tokens();
        assert_eq!(ReceiptTokens::decode(&mut &t.encode()[..]).unwrap(), t);
        assert_eq!(ReceiptTokens::from_json(&t.to_json()).unwrap(), t);
        for bad in [
            json!({"salt": "00", "prompt_ids": [], "output_ids": []}),
            json!({"salt": hex::encode([0u8; 32]), "prompt_ids": [-1], "output_ids": []}),
            json!({"salt": hex::encode([0u8; 32]), "prompt_ids": [4294967296u64], "output_ids": []}),
            json!({"salt": hex::encode([0u8; 32]), "prompt_ids": []}),
        ] {
            assert_eq!(ReceiptTokens::from_json(&bad), Err(ContentError::Malformed));
        }
        let shown = format!("{t:?}");
        assert!(!shown.contains("9, 9") && !shown.contains('5'), "{shown}");
    }
}
