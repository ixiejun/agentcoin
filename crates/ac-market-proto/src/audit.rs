//! Audit evidence (m6-audit-chain design D6; spec `market/audit` "审计证据与承诺").
//!
//! A failing verdict puts only [`AuditEvidence::commitment`] on chain. The evidence itself (the
//! request's messages, the answer, the signed receipt, the TOPLOC proofs and the salt and token
//! IDs behind the receipt's content commitment) stays off chain (red line 6) and is handed to the
//! reviewers of a dispute, who check it against the commitment, check that it opens the
//! receipt's content commitment ([`AuditEvidence::check_content`]) and re-check it. The evidence
//! file is the SCALE encoding itself, so the commitment depends only on the file's bytes.
//!
//! Evidence v2 (m6-token-binding design D6) adds the [`ReceiptTokens`]: an accuser can then
//! change nothing but the answer's text without the evidence failing to open the receipt.

use ac_primitives::market::SignedReceipt;
use ac_primitives::market::audit::AuditMetric;
use parity_scale_codec::{Decode, Encode};

use crate::content::{ContentError, ReceiptTokens, check_content};
use crate::toploc::{Metric, ToplocProofs};

/// Hashing context of the evidence commitment (v2; `audit-evidence v1` is retired).
pub const EVIDENCE_CONTEXT: &str = "agentcoin 2026-10 audit-evidence v2";

/// Version of the evidence encoding.
pub const EVIDENCE_VERSION: u8 = 2;

/// Token counts the provider reported.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Encode, Decode)]
pub struct EvidenceUsage {
    /// Prompt tokens.
    pub prompt_tokens: u32,
    /// Generated tokens.
    pub completion_tokens: u32,
}

/// Everything a re-check of one audited inference needs.
#[derive(Clone, Debug, PartialEq, Eq, Encode, Decode)]
pub struct AuditEvidence {
    /// [`EVIDENCE_VERSION`].
    pub version: u8,
    /// The request's `messages`, as a compact JSON array.
    pub messages: Vec<u8>,
    /// The answer's text (UTF-8).
    pub output: Vec<u8>,
    /// The answer's finish reason.
    pub finish_reason: Vec<u8>,
    /// The answer's token counts.
    pub usage: EvidenceUsage,
    /// The receipt, signed by provider and gateway.
    pub receipt: SignedReceipt,
    /// The proofs returned with the receipt, if any.
    pub proofs: Option<ToplocProofs>,
    /// The salt and token IDs returned with the receipt, present exactly when the receipt has
    /// a content commitment.
    pub tokens: Option<ReceiptTokens>,
}

/// Why evidence bytes are not usable.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EvidenceError {
    /// The bytes do not decode, or trail data after the evidence.
    Decode,
    /// The evidence has another version.
    Version(u8),
    /// The bytes do not match the expected commitment.
    CommitmentMismatch,
}

impl core::fmt::Display for EvidenceError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Decode => f.write_str("malformed evidence"),
            Self::Version(v) => write!(f, "unsupported evidence version {v}"),
            Self::CommitmentMismatch => f.write_str("evidence does not match the commitment"),
        }
    }
}

impl std::error::Error for EvidenceError {}

/// Commitment to evidence bytes: `derive("agentcoin 2026-10 audit-evidence v2", bytes)`.
///
/// # Errors
///
/// Only if the published hashing context were rejected, which does not happen.
pub fn commit(bytes: &[u8]) -> Result<[u8; 32], ac_crypto::Error> {
    ac_crypto::hash::derive(EVIDENCE_CONTEXT, bytes)
}

impl AuditEvidence {
    /// The evidence file: the SCALE encoding.
    #[must_use]
    pub fn to_bytes(&self) -> Vec<u8> {
        self.encode()
    }

    /// The commitment a failing verdict carries.
    ///
    /// # Errors
    ///
    /// See [`commit`].
    pub fn commitment(&self) -> Result<[u8; 32], ac_crypto::Error> {
        commit(&self.to_bytes())
    }

    /// Decodes evidence bytes, rejecting trailing data and other versions.
    ///
    /// # Errors
    ///
    /// [`EvidenceError::Decode`] or [`EvidenceError::Version`].
    pub fn from_bytes(mut bytes: &[u8]) -> Result<Self, EvidenceError> {
        let e = Self::decode(&mut bytes).map_err(|_| EvidenceError::Decode)?;
        if !bytes.is_empty() {
            return Err(EvidenceError::Decode);
        }
        if e.version != EVIDENCE_VERSION {
            return Err(EvidenceError::Version(e.version));
        }
        Ok(e)
    }

    /// Decodes evidence bytes after checking them against `commitment`.
    ///
    /// # Errors
    ///
    /// [`EvidenceError::CommitmentMismatch`] first, then the errors of [`Self::from_bytes`].
    pub fn open(bytes: &[u8], commitment: &[u8; 32]) -> Result<Self, EvidenceError> {
        let c = commit(bytes).map_err(|_| EvidenceError::CommitmentMismatch)?;
        if c != *commitment {
            return Err(EvidenceError::CommitmentMismatch);
        }
        Self::from_bytes(bytes)
    }

    /// The request's messages.
    ///
    /// # Errors
    ///
    /// [`ContentError::Malformed`] if they are not a JSON array.
    pub fn parsed_messages(&self) -> Result<Vec<serde_json::Value>, ContentError> {
        match serde_json::from_slice(&self.messages) {
            Ok(serde_json::Value::Array(m)) => Ok(m),
            _ => Err(ContentError::Malformed),
        }
    }

    /// Checks that the messages, salt and token IDs open the receipt's content commitment (see
    /// [`check_content`]). Evidence that does not is the accuser's fault, never the provider's.
    ///
    /// # Errors
    ///
    /// The [`ContentError`] of the first failing rule.
    pub fn check_content(&self) -> Result<(), ContentError> {
        check_content(
            &self.receipt.body,
            self.tokens.as_ref(),
            &self.parsed_messages()?,
        )
    }
}

/// The evidence of a re-check case given as JSON (the `ac-auditor` case format: `messages`,
/// `output`, `finish_reason`, `usage`, `receipt` and optional `toploc`, the last two as hex
/// SCALE, and optional `tokens`, see [`ReceiptTokens::from_json`]). Wallets and auditors both go
/// through this function, so their commitments agree.
///
/// # Errors
///
/// [`EvidenceError::Decode`] for a missing field or a bad receipt, proofs or tokens.
pub fn evidence_from_case(case: &serde_json::Value) -> Result<AuditEvidence, EvidenceError> {
    let text = |k: &str| {
        case.get(k)
            .and_then(serde_json::Value::as_str)
            .ok_or(EvidenceError::Decode)
    };
    let hex_field =
        |h: &str| hex::decode(h.trim_start_matches("0x")).map_err(|_| EvidenceError::Decode);
    let usage = case.get("usage").ok_or(EvidenceError::Decode)?;
    let count = |k: &str| {
        usage
            .get(k)
            .and_then(serde_json::Value::as_u64)
            .and_then(|n| u32::try_from(n).ok())
            .ok_or(EvidenceError::Decode)
    };
    let receipt = SignedReceipt::decode(&mut &hex_field(text("receipt")?)?[..])
        .map_err(|_| EvidenceError::Decode)?;
    let proofs = match case.get("toploc").and_then(serde_json::Value::as_str) {
        Some(h) => {
            Some(ToplocProofs::decode(&mut &hex_field(h)?[..]).map_err(|_| EvidenceError::Decode)?)
        }
        None => None,
    };
    let tokens = match case.get("tokens") {
        Some(t) if !t.is_null() => {
            Some(ReceiptTokens::from_json(t).map_err(|_| EvidenceError::Decode)?)
        }
        _ => None,
    };
    Ok(AuditEvidence {
        version: EVIDENCE_VERSION,
        messages: serde_json::to_vec(case.get("messages").ok_or(EvidenceError::Decode)?)
            .map_err(|_| EvidenceError::Decode)?,
        output: text("output")?.as_bytes().to_vec(),
        finish_reason: text("finish_reason")?.as_bytes().to_vec(),
        usage: EvidenceUsage {
            prompt_tokens: count("prompt_tokens")?,
            completion_tokens: count("completion_tokens")?,
        },
        receipt,
        proofs,
        tokens,
    })
}

/// Reviewer → accuser: the evidence of a dispute's verdict (m6-auditor-agent design D6). Sent
/// inside a sealed channel, so the accuser knows the reviewer's account from the handshake.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Encode, Decode)]
pub struct EvidenceRequest {
    /// The dispute the reviewer was drawn for.
    pub dispute: u64,
    /// The commitment the accuser's verdict carries.
    pub commitment: [u8; 32],
}

/// Accuser → reviewer. The accuser never says why it refuses, so a refusal does not reveal
/// whether it holds the evidence.
#[derive(Clone, Debug, PartialEq, Eq, Encode, Decode)]
pub enum EvidenceResponse {
    /// The evidence file (see [`AuditEvidence::to_bytes`]).
    #[codec(index = 0)]
    Evidence(Vec<u8>),
    /// The requester is not entitled to it, or there is none.
    #[codec(index = 1)]
    Refused,
}

/// The on-chain form of a re-check metric.
#[must_use]
pub fn audit_metric(m: Metric) -> AuditMetric {
    match m {
        Metric::ExpMismatches => AuditMetric::ExpMismatches,
        Metric::NoMatchingExponent => AuditMetric::NoMatchingExponent,
        Metric::MantissaMean => AuditMetric::MantissaMean,
        Metric::MantissaMedian => AuditMetric::MantissaMedian,
        Metric::NoChunks => AuditMetric::NoChunks,
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::indexing_slicing)] // Test code.

    use super::*;
    use ac_primitives::market::work::JobKind;
    use ac_primitives::market::{MicroUsd, ModelId, ReceiptBody};
    use sp_core::H256;
    use sp_runtime::AccountId32;

    pub fn sample(proofs: bool) -> AuditEvidence {
        let key = |n: &str| {
            ac_crypto::sig::SigningKey::from_seed(
                ac_crypto::SigAlg::MlDsa44,
                &ac_crypto::dev_seed(n).unwrap(),
            )
            .unwrap()
        };
        let (p, g) = (key("provider"), key("gateway"));
        let messages = br#"[{"content":"hi","role":"user"}]"#.to_vec();
        let tokens = ReceiptTokens {
            salt: [8; 32],
            prompt_ids: (100..112).collect(),
            output_ids: vec![7, 8, 2],
        };
        let parsed: Vec<serde_json::Value> = serde_json::from_slice(&messages).unwrap();
        let body = ReceiptBody {
            genesis: H256([1; 32]),
            gateway: AccountId32::new([2; 32]),
            provider: AccountId32::new([3; 32]),
            kind: JobKind::Inference,
            model: ModelId([4; 32]),
            request_id: [5; 32],
            in_tokens: 12,
            out_tokens: 3,
            fee: MicroUsd(7),
            toploc_commit: if proofs { [6; 32] } else { [0; 32] },
            content_commit: if proofs {
                tokens.commit(&parsed).unwrap()
            } else {
                [0; 32]
            },
            ttft_ms: 10,
            total_ms: 20,
        };
        let payload = body.payload().unwrap();
        let ctx = ac_primitives::market::receipt::RECEIPT_CONTEXT;
        let receipt = SignedReceipt {
            provider_sig: p.sign_deterministic(&payload, ctx).unwrap(),
            provider_key: p.public_key().unwrap(),
            gateway_sig: g.sign_deterministic(&payload, ctx).unwrap(),
            gateway_key: g.public_key().unwrap(),
            body,
        };
        AuditEvidence {
            version: EVIDENCE_VERSION,
            messages,
            output: b"hello there".to_vec(),
            finish_reason: b"stop".to_vec(),
            usage: EvidenceUsage {
                prompt_tokens: 12,
                completion_tokens: 3,
            },
            receipt,
            proofs: proofs.then(|| ToplocProofs {
                decode_batching_size: 32,
                topk: 128,
                skip_prefill: false,
                proofs: vec![vec![0xab; 258]],
            }),
            tokens: proofs.then_some(tokens),
        }
    }

    // Regression vector of evidence v2 (m6-token-binding design D6): the commitment of the
    // sample with proofs. The evidence format is published: never change it.
    #[test]
    fn evidence_v2_vector() {
        let c = sample(true).commitment().unwrap();
        assert_eq!(hex::encode(c), VECTOR_COMMITMENT_V2);
    }

    const VECTOR_COMMITMENT_V2: &str =
        "e20ac97fd5ec1404f9eed8a05500056cd7792d6f235ef16107ca7450be9d60aa";

    // Spec market/auditor-agent "审计证据", scenario "证据可以打开收据的内容承诺".
    #[test]
    fn evidence_opens_the_receipt_content() {
        assert_eq!(sample(true).check_content(), Ok(()));
        assert_eq!(sample(false).check_content(), Ok(()));
    }

    // Spec market/auditor-agent "审计证据", scenario "证据中的内容被改动": a changed message,
    // token ID or salt byte does not open the receipt, even with a fresh evidence commitment.
    #[test]
    fn altered_content_does_not_open_the_receipt() {
        let mut e = sample(true);
        e.messages = br#"[{"role":"user","content":"ho"}]"#.to_vec();
        assert_eq!(e.check_content(), Err(ContentError::Commitment));
        let mut e = sample(true);
        e.tokens.as_mut().unwrap().prompt_ids[3] ^= 1;
        assert_eq!(e.check_content(), Err(ContentError::Commitment));
        let mut e = sample(true);
        e.tokens.as_mut().unwrap().salt[31] ^= 1;
        assert_eq!(e.check_content(), Err(ContentError::Commitment));
        let mut e = sample(true);
        e.tokens = None;
        assert_eq!(e.check_content(), Err(ContentError::Missing));
        let mut e = sample(true);
        e.messages = b"{}".to_vec();
        assert_eq!(e.check_content(), Err(ContentError::Malformed));
        // The answer's text is not committed: changing it leaves the content open.
        let mut e = sample(true);
        e.output = b"something else".to_vec();
        assert_eq!(e.check_content(), Ok(()));
    }

    #[test]
    fn cases_carry_the_tokens() {
        let e = sample(true);
        let case = serde_json::json!({
            "messages": serde_json::from_slice::<serde_json::Value>(&e.messages).unwrap(),
            "output": "hello there",
            "finish_reason": "stop",
            "usage": {"prompt_tokens": 12, "completion_tokens": 3},
            "receipt": hex::encode(e.receipt.encode()),
            "toploc": hex::encode(e.proofs.as_ref().unwrap().encode()),
            "tokens": e.tokens.as_ref().unwrap().to_json(),
        });
        assert_eq!(evidence_from_case(&case).unwrap(), e);
        let mut bad = case.clone();
        bad["tokens"]["salt"] = serde_json::json!("00");
        assert_eq!(evidence_from_case(&bad), Err(EvidenceError::Decode));
    }

    #[test]
    fn round_trip() {
        for proofs in [false, true] {
            let e = sample(proofs);
            let bytes = e.to_bytes();
            assert_eq!(AuditEvidence::from_bytes(&bytes).unwrap(), e);
            assert_eq!(
                AuditEvidence::open(&bytes, &e.commitment().unwrap()).unwrap(),
                e
            );
        }
    }

    #[test]
    fn tampering_changes_the_commitment() {
        // Spec market/audit "审计证据与承诺", scenario "篡改证据".
        let e = sample(true);
        let c = e.commitment().unwrap();
        let mut t = e.clone();
        t.output[0] ^= 1;
        assert_ne!(t.commitment().unwrap(), c);
        assert_eq!(
            AuditEvidence::open(&t.to_bytes(), &c),
            Err(EvidenceError::CommitmentMismatch)
        );
    }

    #[test]
    fn trailing_bytes_and_other_versions_are_rejected() {
        let mut bytes = sample(false).to_bytes();
        bytes.push(0);
        assert_eq!(
            AuditEvidence::from_bytes(&bytes),
            Err(EvidenceError::Decode)
        );
        // Version 1 (without token IDs) is retired.
        let mut e = sample(false);
        e.version = 1;
        assert_eq!(
            AuditEvidence::from_bytes(&e.to_bytes()),
            Err(EvidenceError::Version(1))
        );
    }
}
