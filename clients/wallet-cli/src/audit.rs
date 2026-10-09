//! Audits (m6-audit-chain; spec `clients/wallet-cli` "审计子命令"): reads through `AuditApi` and
//! the verdict an auditor submits from an `ac-auditor` re-check report and its case.

use ac_market_proto::audit::evidence_from_case;
use ac_primitives::market::PriceError;
use ac_primitives::market::audit::{
    AdjustableParams, AuditParams, AuditorRecord, AuditorStats, DisputeKind, DisputeRecord,
    ProviderAuditStats, RoundIndex, SprtState, StatsConfig, StatsParams, VerdictOutcome,
    VerdictRecord, VerdictStats,
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

    /// An auditor's evidence endpoint and encryption key (`AuditApi` version 2).
    ///
    /// # Errors
    ///
    /// RPC failures.
    pub async fn audit_endpoint(
        &self,
        who: &AccountId32,
    ) -> Result<Option<pallet_audit::AuditorEndpoint>> {
        self.audit_api("endpoint", who).await
    }

    /// Open disputes after `after`, at most `limit` (`AuditApi` version 2).
    ///
    /// # Errors
    ///
    /// RPC failures.
    pub async fn audit_open_disputes(
        &self,
        after: Option<&AccountId32>,
        limit: u32,
    ) -> Result<Vec<(AccountId32, u64)>> {
        self.audit_api("open_disputes", &(after, limit)).await
    }

    /// The audit pot's account and balance.
    ///
    /// # Errors
    ///
    /// RPC failures.
    pub async fn audit_pot(&self) -> Result<(AccountId32, u128)> {
        self.audit_api("pot", &()).await
    }

    /// The statistical judgment's parameter version and switch (`AuditApi` version 3).
    ///
    /// # Errors
    ///
    /// RPC failures.
    pub async fn audit_stats_config(&self) -> Result<Option<StatsConfig>> {
        self.audit_api("stats_config", &()).await
    }

    /// `provider`'s statistical state under the current parameter version (`AuditApi`
    /// version 3).
    ///
    /// # Errors
    ///
    /// RPC failures.
    pub async fn audit_sprt_state(&self, provider: &AccountId32) -> Result<SprtState<AccountId32>> {
        self.audit_api("sprt_state", provider).await
    }
}

/// The verdict of `report` (one `ac-auditor recheck` output line) on the inference of `case`
/// (its case file), for `round`; with the commitment to the evidence for a failure or a verdict
/// judged by the thresholds, and the latter's statistics (m6-audit-sprt).
///
/// # Errors
///
/// A report without an on-chain outcome or a thresholds version (a `mismatch` line, for one), a
/// judged report without statistics, or a case whose receipt or proofs do not decode.
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
    let judged = outcome.judged_by_thresholds();
    let stats = if judged {
        Some(stats_of(report).context("the report has no statistics (re-check it again)")?)
    } else {
        None
    };
    let commitment = if judged || matches!(outcome, VerdictOutcome::Fail(_)) {
        Some(evidence.commitment().map_err(|e| anyhow::anyhow!("{e}"))?)
    } else {
        None
    };
    Ok(VerdictSubmission {
        provider: evidence.receipt.body.provider.clone(),
        round,
        outcome,
        thresholds_version,
        evidence: commitment,
        receipt: evidence.receipt,
        stats,
    })
}

/// The statistics of a re-check report line (its `stats` object).
fn stats_of(report: &Value) -> Option<VerdictStats> {
    let s = report.get("stats")?;
    let int = |k: &str| s.get(k).and_then(Value::as_u64);
    let short = |k: &str| int(k).and_then(|v| u16::try_from(v).ok());
    Some(VerdictStats {
        version: short("version")?,
        stats: ac_primitives::market::audit::AuditStats {
            prompt_tokens: int("prompt_tokens").and_then(|v| u32::try_from(v).ok())?,
            prefill_mean_centi: short("prefill_mean_centi")?,
            decode_mean_centi: short("decode_mean_centi")?,
            decode_chunks: short("decode_chunks")?,
        },
    })
}

/// Thousandths of a nat as nats with three decimals (`-0.500`, `24.700`).
#[must_use]
pub fn format_nats(milli: i64) -> String {
    let sign = if milli < 0 { "-" } else { "" };
    let abs = milli.unsigned_abs();
    format!("{sign}{}.{:03}", abs / 1_000, abs % 1_000)
}

/// A verdict's statistics for showing: version, prompt tokens, the two means and the decode
/// chunks.
#[must_use]
pub fn format_stats(s: &VerdictStats) -> String {
    let centi = |v: u16| format!("{}.{:02}", v / 100, v % 100);
    format!(
        "stats v{}: {} prompt tokens, prefill mean {}, decode mean {} over {} chunks",
        s.version,
        s.stats.prompt_tokens,
        centi(s.stats.prefill_mean_centi),
        centi(s.stats.decode_mean_centi),
        s.stats.decode_chunks
    )
}

/// What opened a dispute, for showing.
#[must_use]
pub fn format_kind(kind: &DisputeKind) -> String {
    match kind {
        DisputeKind::Fail => "failing verdicts".into(),
        DisputeKind::Statistical { stats_version } => {
            format!("statistical (parameters v{stats_version})")
        }
    }
}

/// A provider's statistical state against the bound, for showing.
#[must_use]
pub fn format_state(state: &SprtState<AccountId32>, params: Option<&StatsParams>) -> String {
    let bound = params.map_or_else(|| "?".to_owned(), |p| format_nats(p.bound));
    format!(
        "statistical state: {} of {bound} nats, {} verdicts",
        format_nats(state.cumulative),
        state.entries.len()
    )
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
            "onchain": hex::encode(fail.encode()), "stats": stats(),
        });
        let v = verdict_from(&report, &case(), 7).unwrap();
        let expected = evidence_from_case(&case()).unwrap().commitment().unwrap();
        assert_eq!(v.evidence, Some(expected));
        assert_eq!((v.round, v.thresholds_version, v.outcome), (7, 2, fail));
        assert_eq!(v.provider, AccountId32::new([3; 32]));
        assert_eq!(v.receipt, receipt());
        assert_eq!(v.stats.unwrap().stats.decode_mean_centi, 106);
    }

    fn stats() -> Value {
        json!({"version": 1, "prompt_tokens": 219, "prefill_mean_centi": 51,
               "decode_mean_centi": 106, "decode_chunks": 4})
    }

    // m6-audit-sprt 6.1: statistics, states and dispute kinds as shown.
    #[test]
    fn statistics_are_shown_in_plain_units() {
        assert_eq!(format_nats(24_700), "24.700");
        assert_eq!(format_nats(-500), "-0.500");
        assert_eq!(format_nats(0), "0.000");
        let v = verdict_from(
            &json!({"thresholds_version": 2, "onchain": hex::encode(VerdictOutcome::Pass.encode()), "stats": stats()}),
            &case(),
            1,
        )
        .unwrap();
        assert_eq!(
            format_stats(&v.stats.unwrap()),
            "stats v1: 219 prompt tokens, prefill mean 0.51, decode mean 1.06 over 4 chunks"
        );
        assert_eq!(format_kind(&DisputeKind::Fail), "failing verdicts");
        assert_eq!(
            format_kind(&DisputeKind::Statistical { stats_version: 1 }),
            "statistical (parameters v1)"
        );
        let state = SprtState {
            cumulative: 6_000,
            entries: Default::default(),
        };
        let p = ac_primitives::market::audit::CURRENT_STATS;
        assert_eq!(
            format_state(&state, Some(&p)),
            "statistical state: 6.000 of 24.700 nats, 0 verdicts"
        );
        assert_eq!(
            format_state(&state, None),
            "statistical state: 6.000 of ? nats, 0 verdicts"
        );
    }

    // m6-audit-sprt: a pass carries its statistics and an evidence commitment; an
    // inconclusive verdict neither.
    #[test]
    fn judged_verdicts_carry_statistics_and_bad_reports_are_refused() {
        let pass = hex::encode(VerdictOutcome::Pass.encode());
        let report = json!({"thresholds_version": 2, "onchain": pass, "stats": stats()});
        let v = verdict_from(&report, &case(), 1).unwrap();
        assert!(v.evidence.is_some());
        assert_eq!(v.stats.unwrap().version, 1);
        let no_stats = json!({"thresholds_version": 2, "onchain": pass});
        assert!(verdict_from(&no_stats, &case(), 1).is_err());
        let tokens =
            VerdictOutcome::Inconclusive(ac_primitives::market::audit::InconclusiveReason::Tokens);
        let report = json!({"thresholds_version": 2, "onchain": hex::encode(tokens.encode())});
        let v = verdict_from(&report, &case(), 1).unwrap();
        assert_eq!((v.evidence, v.stats), (None, None));
        let mismatch = json!({"outcome": "mismatch"});
        assert!(verdict_from(&mismatch, &case(), 1).is_err());
        let other =
            json!({"thresholds_version": 2, "request": "00", "onchain": pass, "stats": stats()});
        assert!(verdict_from(&other, &case(), 1).is_err());
    }
}
