//! Audits (m6-audit-chain; spec `clients/wallet-cli` "审计子命令"): reads through `AuditApi`, the
//! verdict an auditor submits from an `ac-auditor` re-check report and its case, and the local
//! checks made before it is signed (m6-audit-hardening).

use ac_market_proto::audit::evidence_from_case;
use ac_primitives::market::audit::{
    AdjustableParams, AuditParams, AuditProfile, AuditorRecord, AuditorStats, DisputeKind,
    DisputeRecord, ProfileSource, ProviderAuditStats, ReviewOutcome, ReviewRecord, ReviewStats,
    ReviewVote, RoundIndex, SprtState, StatsParams, VerdictOutcome, VerdictRecord, VerdictStats,
    spot_checked,
};
use ac_primitives::market::{MicroUsd, ModelId, PriceError};
use anyhow::{Context, Result, bail};
use pallet_audit::VerdictSubmission;
use parity_scale_codec::{Decode, Encode};
use serde_json::Value;
use sp_core::H256;
use sp_runtime::AccountId32;

use crate::client::NodeClient;

/// A dispute as the chain stores it.
pub type Dispute = DisputeRecord<AccountId32, u32>;
/// A spot check as the chain stores it.
pub type Review = ReviewRecord<AccountId32, u32>;

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

    /// Whether the statistical judgment opens disputes (`AuditApi` version 4).
    ///
    /// # Errors
    ///
    /// RPC failures.
    pub async fn audit_stats_enabled(&self) -> Result<bool> {
        self.audit_api("stats_enabled", &()).await
    }

    /// `provider`'s statistical states by model, under each model's profile (`AuditApi`
    /// version 4).
    ///
    /// # Errors
    ///
    /// RPC failures.
    pub async fn audit_sprt_states(
        &self,
        provider: &AccountId32,
    ) -> Result<Vec<(ModelId, SprtState<AccountId32>)>> {
        self.audit_api("sprt_states", provider).await
    }

    /// `model`'s audit profile and where it comes from; `None` when it is uncalibrated
    /// (`AuditApi` version 4).
    ///
    /// # Errors
    ///
    /// RPC failures.
    pub async fn audit_profile(
        &self,
        model: &ModelId,
    ) -> Result<Option<(AuditProfile, ProfileSource)>> {
        self.audit_api("profile", model).await
    }

    /// Open spot checks after `after`, at most `limit` (`AuditApi` version 4).
    ///
    /// # Errors
    ///
    /// RPC failures.
    pub async fn audit_open_reviews(&self, after: Option<u64>, limit: u32) -> Result<Vec<u64>> {
        self.audit_api("open_reviews", &(after, limit)).await
    }

    /// A spot check (`AuditApi` version 4).
    ///
    /// # Errors
    ///
    /// RPC failures.
    pub async fn audit_review(&self, id: u64) -> Result<Option<Review>> {
        self.audit_api("review", &id).await
    }

    /// Open disputes and spot checks citing `who`'s verdicts (`AuditApi` version 4).
    ///
    /// # Errors
    ///
    /// RPC failures.
    pub async fn audit_locks(&self, who: &AccountId32) -> Result<u32> {
        self.audit_api("locks", who).await
    }

    /// An auditor's spot-check counts (`AuditApi` version 4).
    ///
    /// # Errors
    ///
    /// RPC failures.
    pub async fn audit_review_stats(&self, who: &AccountId32) -> Result<ReviewStats> {
        self.audit_api("review_stats", who).await
    }

    /// A provider's deterrent, required stake and recent revenue per epoch, in dollars
    /// (`AuditApi` version 4).
    ///
    /// # Errors
    ///
    /// RPC failures.
    pub async fn audit_required_stake(
        &self,
        provider: &AccountId32,
    ) -> Result<Option<(MicroUsd, MicroUsd, MicroUsd)>> {
        self.audit_api("required_stake", provider).await
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
        DisputeKind::ModelStatistical {
            stats_version,
            model,
        } => format!(
            "statistical on model {} (parameters v{stats_version})",
            hex::encode(model.0)
        ),
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

/// Checks a verdict before it is signed (spec "审计子命令" / "提交前发现未被分配", "未校准模型在本地
/// 被拦下"): `me` is among the auditors `assigned` to its provider, the receipt's model has an
/// audit `profile`, and the verdict's versions are the profile's.
///
/// # Errors
///
/// The first check that fails, with its reason.
pub fn check_verdict(
    verdict: &VerdictSubmission,
    me: &AccountId32,
    assigned: &[AccountId32],
    profile: Option<&AuditProfile>,
) -> Result<()> {
    if !assigned.contains(me) {
        bail!(
            "{} is not assigned to provider {} in round {}",
            ac_primitives::encode_address(me.as_ref()),
            ac_primitives::encode_address(verdict.provider.as_ref()),
            verdict.round
        );
    }
    let model = hex::encode(verdict.receipt.body.model.0);
    let Some(p) = profile else {
        bail!(
            "model 0x{model} has no audit profile (it is uncalibrated): the chain refuses verdicts on it"
        );
    };
    if verdict.thresholds_version != p.thresholds_version {
        bail!(
            "the report uses thresholds version {}, model 0x{model}'s profile version {}",
            verdict.thresholds_version,
            p.thresholds_version
        );
    }
    if let Some(s) = &verdict.stats
        && s.version != p.stats_version
    {
        bail!(
            "the report uses statistical parameter version {}, model 0x{model}'s profile version {}",
            s.version,
            p.stats_version
        );
    }
    Ok(())
}

/// Whether a verdict is drawn for a spot check, recomputed from public data (spec "裁决抽查"):
/// only verdicts with statistics are drawn, with the seed of the round after the verdict's
/// (`next_seed`, `None` until it exists) and the receipt's request ID, at `rate_bps`.
#[must_use]
pub fn spot_drawn(
    verdict: &VerdictRecord<AccountId32>,
    next_seed: Option<&H256>,
    rate_bps: u16,
) -> Option<bool> {
    if verdict.stats.is_none() {
        return Some(false);
    }
    next_seed.map(|seed| spot_checked(seed, &verdict.request_id, rate_bps))
}

/// Parses `uphold` or `overturn`.
///
/// # Errors
///
/// Anything else.
pub fn parse_review_vote(text: &str) -> Result<ReviewVote> {
    match text {
        "uphold" => Ok(ReviewVote::Uphold),
        "overturn" => Ok(ReviewVote::Overturn),
        other => bail!("a spot-check vote is `uphold` or `overturn`, not `{other}`"),
    }
}

/// A model's audit profile, for showing.
#[must_use]
pub fn format_profile(profile: Option<&(AuditProfile, ProfileSource)>) -> String {
    match profile {
        None => "audit profile: none (uncalibrated: not audited, no work counted)".into(),
        Some((p, source)) => format!(
            "audit profile: thresholds v{}, statistics v{} ({})",
            p.thresholds_version,
            p.stats_version,
            match source {
                ProfileSource::Own => "its own",
                ProfileSource::Default => "the chain's default",
            }
        ),
    }
}

/// A spot check, for showing: the checked verdict, the ballots and the outcome.
#[must_use]
pub fn format_review(id: u64, r: &Review) -> Vec<String> {
    let mut lines = vec![format!(
        "spot check {id}: verdict of {} on {} in round {}, deadline block {}",
        ac_primitives::encode_address(r.auditor.as_ref()),
        ac_primitives::encode_address(r.provider.as_ref()),
        r.round,
        r.deadline
    )];
    for (who, ballot) in &r.reviewers {
        let ballot = match ballot {
            None => "none".to_owned(),
            Some((vote, true)) => format!("{vote:?}"),
            Some((vote, false)) => format!("{vote:?}, evidence not obtained"),
        };
        lines.push(format!(
            "  reviewer: {} vote {ballot}",
            ac_primitives::encode_address(who.as_ref())
        ));
    }
    lines.push(match r.outcome {
        None => "  outcome: open".to_owned(),
        Some(ReviewOutcome::Upheld) => "  outcome: upheld".to_owned(),
        Some(ReviewOutcome::Overturned) => "  outcome: overturned".to_owned(),
        Some(ReviewOutcome::Undecided) => "  outcome: undecided".to_owned(),
    });
    lines
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
            content_commit: [0; 32],
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

    fn pass_verdict(thresholds_version: u16) -> VerdictSubmission {
        let pass = hex::encode(VerdictOutcome::Pass.encode());
        let report =
            json!({"thresholds_version": thresholds_version, "onchain": pass, "stats": stats()});
        verdict_from(&report, &case(), 3).unwrap()
    }

    const PROFILE: AuditProfile = AuditProfile {
        thresholds_version: 4,
        stats_version: 1,
    };

    // Spec "审计子命令" / "提交前发现未被分配".
    #[test]
    fn an_auditor_not_assigned_is_stopped_before_signing() {
        let me = AccountId32::new([9; 32]);
        let v = pass_verdict(4);
        let others = [AccountId32::new([8; 32])];
        let e = check_verdict(&v, &me, &others, Some(&PROFILE)).unwrap_err();
        assert!(e.to_string().contains("is not assigned"), "{e}");
        check_verdict(&v, &me, &[others[0].clone(), me.clone()], Some(&PROFILE)).unwrap();
    }

    // Spec "审计子命令" / "未校准模型在本地被拦下"; and versions other than the profile's.
    #[test]
    fn an_uncalibrated_model_is_stopped_before_signing() {
        let me = AccountId32::new([9; 32]);
        let assigned = [me.clone()];
        let e = check_verdict(&pass_verdict(4), &me, &assigned, None).unwrap_err();
        assert!(e.to_string().contains("no audit profile"), "{e}");
        let e = check_verdict(&pass_verdict(3), &me, &assigned, Some(&PROFILE)).unwrap_err();
        assert!(e.to_string().contains("thresholds version 3"), "{e}");
        let newer_stats = AuditProfile {
            stats_version: 2,
            ..PROFILE
        };
        let e = check_verdict(&pass_verdict(4), &me, &assigned, Some(&newer_stats)).unwrap_err();
        assert!(
            e.to_string().contains("statistical parameter version 1"),
            "{e}"
        );
    }

    #[test]
    fn spot_check_draws_are_recomputed() {
        let record = |stats: Option<VerdictStats>| VerdictRecord {
            auditor: AccountId32::new([1; 32]),
            outcome: VerdictOutcome::Pass,
            thresholds_version: 4,
            evidence: Some([2; 32]),
            receipt_hash: H256([3; 32]),
            request_id: [5; 32],
            gateway: AccountId32::new([2; 32]),
            stats,
        };
        let judged = record(pass_verdict(4).stats);
        let seed = H256([7; 32]);
        assert_eq!(spot_drawn(&judged, None, 500), None);
        assert_eq!(spot_drawn(&judged, Some(&seed), 10_000), Some(true));
        assert_eq!(spot_drawn(&judged, Some(&seed), 0), Some(false));
        assert_eq!(
            spot_drawn(&judged, Some(&seed), 500),
            Some(spot_checked(&seed, &[5; 32], 500))
        );
        // Verdicts without statistics are never drawn.
        assert_eq!(spot_drawn(&record(None), Some(&seed), 10_000), Some(false));
    }

    #[test]
    fn spot_check_votes_and_profiles_are_shown() {
        assert_eq!(parse_review_vote("uphold").unwrap(), ReviewVote::Uphold);
        assert_eq!(parse_review_vote("overturn").unwrap(), ReviewVote::Overturn);
        assert!(parse_review_vote("confirm").is_err());
        assert_eq!(
            format_profile(Some(&(PROFILE, ProfileSource::Default))),
            "audit profile: thresholds v4, statistics v1 (the chain's default)"
        );
        assert!(format_profile(None).contains("uncalibrated"));
        let r = Review {
            provider: AccountId32::new([3; 32]),
            round: 4,
            auditor: AccountId32::new([1; 32]),
            reviewers: frame_support::BoundedVec::truncate_from(vec![
                (
                    AccountId32::new([5; 32]),
                    Some((ReviewVote::Overturn, false)),
                ),
                (AccountId32::new([6; 32]), None),
            ]),
            deadline: 120,
            outcome: Some(ReviewOutcome::Overturned),
            closed_at: Some(110),
        };
        let lines = format_review(2, &r);
        assert!(lines[0].starts_with("spot check 2: verdict of atc1"));
        assert!(lines[1].ends_with("vote Overturn, evidence not obtained"));
        assert!(lines[2].ends_with("vote none"));
        assert_eq!(lines[3], "  outcome: overturned");
    }
}
