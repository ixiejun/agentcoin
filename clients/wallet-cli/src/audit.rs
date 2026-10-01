//! Audits (m6-audit-chain; spec `clients/wallet-cli` "审计子命令"): reads through `AuditApi` and
//! the verdict an auditor submits from an `ac-auditor` re-check report and its case.

use ac_market_proto::audit::evidence_from_case;
use ac_primitives::market::PriceError;
use ac_primitives::market::audit::{
    AdjustableParams, AuditParams, AuditorRecord, AuditorStats, DisputeRecord, ProviderAuditStats,
    RoundIndex, VerdictOutcome, VerdictRecord,
};
use anyhow::{Context, Result, bail};
use pallet_audit::VerdictSubmission;
use parity_scale_codec::{Decode, Encode};
use serde_json::Value;
use sp_core::H256;
use sp_runtime::AccountId32;

use crate::client::NodeClient;

/// A dispute as the chain stores it.
pub type Dispute = DisputeRecord<AccountId32, u32>;

impl NodeClient {
    async fn audit_api<T: Decode>(&self, method: &str, args: &impl Encode) -> Result<T> {
        let raw = self.call_api(&format!("AuditApi_{method}"), args).await?;
        Ok(T::decode(&mut &raw[..])?)
    }

    /// The current round and the first blocks of it and of the next one.
    ///
    /// # Errors
    ///
    /// RPC failures.
    pub async fn audit_round(&self) -> Result<Option<(RoundIndex, u32, u32)>> {
        self.audit_api("round", &()).await
    }

    /// Fixed and adjustable audit parameters.
    ///
    /// # Errors
    ///
    /// RPC failures.
    pub async fn audit_params(&self) -> Result<Option<(AuditParams, AdjustableParams)>> {
        self.audit_api("params", &()).await
    }

    /// The auditors assigned to `provider` in `round`.
    ///
    /// # Errors
    ///
    /// RPC failures.
    pub async fn audit_assignment(
        &self,
        round: RoundIndex,
        provider: &AccountId32,
    ) -> Result<Vec<AccountId32>> {
        self.audit_api("assignment", &(round, provider)).await
    }

    /// The providers `auditor` is assigned to in `round`, with whether it submitted on each.
    ///
    /// # Errors
    ///
    /// RPC failures.
    pub async fn audit_assigned_to(
        &self,
        round: RoundIndex,
        auditor: &AccountId32,
    ) -> Result<Vec<(AccountId32, bool)>> {
        self.audit_api("assigned_to", &(round, auditor)).await
    }

    /// The verdicts on `provider` in `round`.
    ///
    /// # Errors
    ///
    /// RPC failures.
    pub async fn audit_verdicts(
        &self,
        round: RoundIndex,
        provider: &AccountId32,
    ) -> Result<Vec<VerdictRecord<AccountId32>>> {
        self.audit_api("verdicts", &(round, provider)).await
    }

    /// `provider`'s open dispute.
    ///
    /// # Errors
    ///
    /// RPC failures.
    pub async fn audit_open_dispute(&self, provider: &AccountId32) -> Result<Option<u64>> {
        self.audit_api("open_dispute", provider).await
    }

    /// A dispute.
    ///
    /// # Errors
    ///
    /// RPC failures.
    pub async fn audit_dispute(&self, id: u64) -> Result<Option<Dispute>> {
        self.audit_api("dispute", &id).await
    }

    /// A registered auditor.
    ///
    /// # Errors
    ///
    /// RPC failures.
    pub async fn audit_auditor(&self, who: &AccountId32) -> Result<Option<AuditorRecord<u32>>> {
        self.audit_api("auditor", who).await
    }

    /// Stake an auditor needs now.
    ///
    /// # Errors
    ///
    /// RPC failures.
    pub async fn audit_threshold(&self) -> Result<Result<u128, PriceError>> {
        self.audit_api("auditor_threshold", &()).await
    }

    /// Verdict counts of a provider.
    ///
    /// # Errors
    ///
    /// RPC failures.
    pub async fn audit_provider_stats(&self, who: &AccountId32) -> Result<ProviderAuditStats> {
        self.audit_api("provider_stats", who).await
    }

    /// Activity counts of an auditor.
    ///
    /// # Errors
    ///
    /// RPC failures.
    pub async fn audit_auditor_stats(&self, who: &AccountId32) -> Result<AuditorStats> {
        self.audit_api("auditor_stats", who).await
    }

    /// The audit pot's account and balance.
    ///
    /// # Errors
    ///
    /// RPC failures.
    pub async fn audit_pot(&self) -> Result<(AccountId32, u128)> {
        self.audit_api("pot", &()).await
    }
}

/// The verdict of `report` (one `ac-auditor recheck` output line) on the inference of `case`
/// (its case file), for `round`; with the commitment to the evidence for a failure.
///
/// # Errors
///
/// A report without an on-chain outcome or a thresholds version (a `mismatch` line, for one), or
/// a case whose receipt or proofs do not decode.
pub fn verdict_from(report: &Value, case: &Value, round: RoundIndex) -> Result<VerdictSubmission> {
    let onchain = report
        .get("onchain")
        .and_then(Value::as_str)
        .context("the report has no on-chain outcome (is it an ac-auditor recheck line?)")?;
    let outcome = VerdictOutcome::decode(&mut &hex::decode(onchain).context("outcome hex")?[..])
        .context("outcome encoding")?;
    let thresholds_version = report
        .get("thresholds_version")
        .and_then(Value::as_u64)
        .and_then(|v| u16::try_from(v).ok())
        .context("the report has no thresholds version")?;
    let evidence = evidence_from_case(case).map_err(|e| anyhow::anyhow!("case: {e}"))?;
    if let Some(request) = report.get("request").and_then(Value::as_str)
        && request != hex::encode(evidence.receipt.body.request_id)
    {
        bail!("the report is about another request than the case's receipt");
    }
    let commitment = match outcome {
        VerdictOutcome::Fail(_) => Some(evidence.commitment().map_err(|e| anyhow::anyhow!("{e}"))?),
        _ => None,
    };
    Ok(VerdictSubmission {
        provider: evidence.receipt.body.provider.clone(),
        round,
        outcome,
        thresholds_version,
        evidence: commitment,
        receipt: evidence.receipt,
    })
}

/// The seed of `round`, for showing.
///
/// # Errors
///
/// RPC failures.
pub async fn seed(client: &NodeClient, round: RoundIndex) -> Result<Option<H256>> {
    client.audit_api("seed", &round).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use ac_crypto::SigAlg;
    use ac_crypto::sig::SigningKey;
    use ac_primitives::market::audit::{AuditMetric, FailReason};
    use ac_primitives::market::receipt::RECEIPT_CONTEXT;
    use ac_primitives::market::work::JobKind;
    use ac_primitives::market::{MicroUsd, ModelId, ReceiptBody, SignedReceipt};
    use serde_json::json;

    fn receipt() -> SignedReceipt {
        let key = |n: &str| {
            SigningKey::from_seed(SigAlg::MlDsa44, &ac_crypto::dev_seed(n).unwrap()).unwrap()
        };
        let (p, g) = (key("wallet-audit-p"), key("wallet-audit-g"));
        let body = ReceiptBody {
            genesis: H256([1; 32]),
            gateway: AccountId32::new([2; 32]),
            provider: AccountId32::new([3; 32]),
            kind: JobKind::Inference,
            model: ModelId([4; 32]),
            request_id: [5; 32],
            in_tokens: 10,
            out_tokens: 20,
            fee: MicroUsd(1),
            toploc_commit: [0; 32],
            ttft_ms: 1,
            total_ms: 2,
        };
        let payload = body.payload().unwrap();
        SignedReceipt {
            provider_key: p.public_key().unwrap(),
            provider_sig: p.sign_deterministic(&payload, RECEIPT_CONTEXT).unwrap(),
            gateway_key: g.public_key().unwrap(),
            gateway_sig: g.sign_deterministic(&payload, RECEIPT_CONTEXT).unwrap(),
            body,
        }
    }

    fn case() -> Value {
        json!({
            "model": "0x04", "engine_model": "m",
            "messages": [{"role": "user", "content": "hi"}],
            "output": "hello", "finish_reason": "stop",
            "usage": {"prompt_tokens": 10, "completion_tokens": 20},
            "receipt": hex::encode(receipt().encode()),
            "toploc": null,
        })
    }

    // Spec clients/wallet-cli "审计子命令" / "从复核结果提交不通过": the verdict commits to the
    // evidence exported from the case.
    #[test]
    fn a_failing_report_commits_to_the_case_evidence() {
        let fail = VerdictOutcome::Fail(FailReason::Threshold {
            chunk: 1,
            metric: AuditMetric::ExpMismatches,
        });
        let report = json!({
            "outcome": "fail", "thresholds_version": 2, "request": hex::encode([5u8; 32]),
            "onchain": hex::encode(fail.encode()),
        });
        let v = verdict_from(&report, &case(), 7).unwrap();
        let expected = evidence_from_case(&case()).unwrap().commitment().unwrap();
        assert_eq!(v.evidence, Some(expected));
        assert_eq!((v.round, v.thresholds_version, v.outcome), (7, 2, fail));
        assert_eq!(v.provider, AccountId32::new([3; 32]));
        assert_eq!(v.receipt, receipt());
    }

    #[test]
    fn passes_carry_no_evidence_and_bad_reports_are_refused() {
        let report =
            json!({"thresholds_version": 2, "onchain": hex::encode(VerdictOutcome::Pass.encode())});
        assert_eq!(verdict_from(&report, &case(), 1).unwrap().evidence, None);
        let mismatch = json!({"outcome": "mismatch"});
        assert!(verdict_from(&mismatch, &case(), 1).is_err());
        let other = json!({"thresholds_version": 2, "request": "00", "onchain": hex::encode(VerdictOutcome::Pass.encode())});
        assert!(verdict_from(&other, &case(), 1).is_err());
    }
}
