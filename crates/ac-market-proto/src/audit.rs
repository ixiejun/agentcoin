//! Audit evidence (m6-audit-chain design D6; spec `market/audit` "审计证据与承诺").
//!
//! A failing verdict puts only [`AuditEvidence::commitment`] on chain. The evidence itself (the
//! request's messages, the answer, the signed receipt and the TOPLOC proofs) stays off chain
//! (red line 6) and is handed to the reviewers of a dispute, who check it against the
//! commitment and re-check it. The evidence file is the SCALE encoding itself, so the commitment
//! depends only on the file's bytes.

use ac_primitives::market::SignedReceipt;
use ac_primitives::market::audit::AuditMetric;
use parity_scale_codec::{Decode, Encode};

use crate::toploc::{Metric, ToplocProofs};

/// Hashing context of the evidence commitment.
pub const EVIDENCE_CONTEXT: &str = "agentcoin 2026-10 audit-evidence v1";

/// Version of the evidence encoding.
pub const EVIDENCE_VERSION: u8 = 1;

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
    /// The request's `messages`, as compact JSON bytes.
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

/// Commitment to evidence bytes: `derive("agentcoin 2026-10 audit-evidence v1", bytes)`.
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
            messages: br#"[{"role":"user","content":"hi"}]"#.to_vec(),
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
        }
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
        let mut e = sample(false);
        e.version = 2;
        assert_eq!(
            AuditEvidence::from_bytes(&e.to_bytes()),
            Err(EvidenceError::Version(2))
        );
    }
}
