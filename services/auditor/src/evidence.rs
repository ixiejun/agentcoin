//! Audit evidence (m6-audit-chain; spec `market/auditor-agent` "审计证据"): a failing re-check's
//! case exported as the evidence a verdict commits to, and a case rebuilt from evidence for the
//! reviewers of a dispute. Neither ever logs the evidence's content.

use ac_market_proto::audit::{AuditEvidence, audit_metric, evidence_from_case};
use ac_primitives::market::audit::{FailReason as ChainFail, InconclusiveReason, VerdictOutcome};
use anyhow::{Context, Result};
use parity_scale_codec::Encode;

use crate::case::{CaseUsage, FailReason, Inconclusive, Outcome, RecheckCase};

/// The evidence of `case`: its messages as compact JSON, its answer, usage, receipt and proofs
/// (through `ac-market-proto`, as wallets compute it).
///
/// # Errors
///
/// A receipt or proofs that do not decode.
pub fn evidence_of(case: &RecheckCase) -> Result<AuditEvidence> {
    let value = serde_json::to_value(case).context("case")?;
    evidence_from_case(&value).map_err(|e| anyhow::anyhow!("{e}"))
}

/// The re-check case of `evidence`, for the model named `engine_model` on the re-check engine.
///
/// # Errors
///
/// Messages that are not JSON, or an answer or finish reason that is not UTF-8.
pub fn case_of(evidence: &AuditEvidence, engine_model: &str) -> Result<RecheckCase> {
    Ok(RecheckCase {
        model: format!("0x{}", hex::encode(evidence.receipt.body.model.0)),
        engine_model: engine_model.to_owned(),
        messages: serde_json::from_slice(&evidence.messages).context("messages")?,
        output: String::from_utf8(evidence.output.clone()).context("output")?,
        finish_reason: String::from_utf8(evidence.finish_reason.clone())
            .context("finish reason")?,
        usage: CaseUsage {
            prompt_tokens: evidence.usage.prompt_tokens,
            completion_tokens: evidence.usage.completion_tokens,
        },
        receipt: hex::encode(evidence.receipt.encode()),
        toploc: evidence.proofs.as_ref().map(|p| hex::encode(p.encode())),
    })
}

/// The on-chain form of a re-check's outcome, as a verdict carries it.
#[must_use]
pub fn onchain_outcome(outcome: &Outcome) -> VerdictOutcome {
    match outcome {
        Outcome::Pass => VerdictOutcome::Pass,
        Outcome::Fail(f) => VerdictOutcome::Fail(match f {
            FailReason::NoProof => ChainFail::NoProof,
            FailReason::CommitmentMismatch => ChainFail::CommitmentMismatch,
            FailReason::ParamsOrChunks => ChainFail::ParamsOrChunks,
            FailReason::Threshold { chunk, metric } => ChainFail::Threshold {
                chunk: u32::try_from(*chunk).unwrap_or(u32::MAX),
                metric: audit_metric(*metric),
            },
        }),
        Outcome::Inconclusive(i) => VerdictOutcome::Inconclusive(match i {
            Inconclusive::InputMismatch => InconclusiveReason::InputMismatch,
            Inconclusive::UnsupportedPrecision => InconclusiveReason::Precision,
            Inconclusive::Tokens => InconclusiveReason::Tokens,
            Inconclusive::Engine => InconclusiveReason::Engine,
        }),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)] // Test code.

    use super::*;
    use ac_market_proto::toploc::Metric;

    #[test]
    fn outcomes_map_to_their_chain_form() {
        assert_eq!(onchain_outcome(&Outcome::Pass), VerdictOutcome::Pass);
        assert_eq!(
            onchain_outcome(&Outcome::Fail(FailReason::Threshold {
                chunk: 3,
                metric: Metric::MantissaMedian
            })),
            VerdictOutcome::Fail(ChainFail::Threshold {
                chunk: 3,
                metric: ac_primitives::market::audit::AuditMetric::MantissaMedian
            })
        );
        assert_eq!(
            onchain_outcome(&Outcome::Inconclusive(Inconclusive::Tokens)),
            VerdictOutcome::Inconclusive(InconclusiveReason::Tokens)
        );
    }
}
