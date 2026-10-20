//! Spot-check reviews (spec `market/auditor-agent` "抽查复核"; m6-audit-hardening design D5): for
//! a verdict drawn for a spot check, fetch its evidence from its auditor, open it against the
//! commitment on chain, check that its receipt is the verdict's, re-check it by the verdict's
//! thresholds version and compare the outcome with the verdict's. The statistics are never
//! compared: they differ too much between hardware to be grounds for a slash. Evidence that is
//! not obtained overturns the verdict and says so; this agent's engine failing, or a version it
//! does not know, is handled as in disputes.

use ac_market_proto::audit::{AuditEvidence, EvidenceResponse};
use ac_market_proto::toploc::AUDIT_THRESHOLDS;
use ac_primitives::market::audit::{ReviewVote, VerdictOutcome};
use parity_scale_codec::Encode;

use super::ports::{AuditCall, SpotCheck};
use super::{Agent, bump, short};
use crate::case::{Inconclusive, Outcome};
use crate::evidence::{case_of, onchain_outcome};
use crate::logging::TARGET;

/// What the checked verdict's evidence showed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpotFinding {
    /// It re-checks with the verdict's outcome.
    Matches,
    /// It was obtained but re-checks otherwise (or cannot be re-checked at all).
    Differs,
    /// It was not obtained: no commitment, not opening against it, or not the verdict's receipt.
    Unobtained,
    /// Not reachable yet (no endpoint, refused, transport error): retried until the deadline.
    Unreachable,
    /// This agent's engine failed: retried; never counted against the auditor.
    EngineDown,
    /// This agent cannot judge it at all (unknown thresholds version or model).
    CannotJudge,
}

/// The decision about a spot check for now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpotDecision {
    /// Cast this vote, saying whether the evidence was obtained.
    Vote(ReviewVote, bool),
    /// Try again later.
    Wait,
    /// Do not vote (and say why).
    Abstain(&'static str),
}

/// The vote a finding calls for (`final_try`: the deadline is near).
#[must_use]
pub fn decide_spot(finding: SpotFinding, final_try: bool) -> SpotDecision {
    match finding {
        SpotFinding::Matches => SpotDecision::Vote(ReviewVote::Uphold, true),
        SpotFinding::Differs => SpotDecision::Vote(ReviewVote::Overturn, true),
        SpotFinding::Unobtained => SpotDecision::Vote(ReviewVote::Overturn, false),
        SpotFinding::CannotJudge => {
            SpotDecision::Abstain("cannot judge the evidence (thresholds version or model)")
        }
        SpotFinding::Unreachable | SpotFinding::EngineDown if !final_try => SpotDecision::Wait,
        // Evidence still unreachable at the deadline was not obtained.
        SpotFinding::Unreachable => SpotDecision::Vote(ReviewVote::Overturn, false),
        SpotFinding::EngineDown => SpotDecision::Abstain("the re-check engine is unavailable"),
    }
}

/// Whether a re-check gives the verdict's outcome: both pass, or both fail. A spot-checked
/// verdict is judged by the thresholds, so its failure is a chunk over a bound; which chunk and
/// which bound may differ between hardware and are not compared.
#[must_use]
pub fn same_outcome(verdict: &VerdictOutcome, recheck: &VerdictOutcome) -> bool {
    matches!(
        (verdict, recheck),
        (VerdictOutcome::Pass, VerdictOutcome::Pass)
            | (VerdictOutcome::Fail(_), VerdictOutcome::Fail(_))
    )
}

impl Agent {
    /// Reviews every open spot check this auditor was drawn for and has not voted in.
    pub async fn review_spot_checks(&self) {
        let Ok(open) = self.chain.open_spot_checks().await else {
            return;
        };
        let Ok(now) = self.chain.best_block().await else {
            return;
        };
        for id in open {
            let Ok(Some(r)) = self.chain.spot_check(id).await else {
                continue;
            };
            let pending = r
                .reviewers
                .iter()
                .any(|(who, b)| *who == self.me && b.is_none());
            if !pending || r.outcome.is_some() || now > r.deadline {
                continue;
            }
            let final_try = now.saturating_add(2) >= r.deadline;
            match decide_spot(self.examine_spot(id, &r).await, final_try) {
                SpotDecision::Vote(vote, obtained) => {
                    let call = AuditCall::SpotVote { id, vote, obtained };
                    match self.chain.submit(call).await {
                        Ok(true) => {
                            bump(&self.counters.votes);
                            log::info!(
                                target: TARGET,
                                "spot check {id} of {}'s verdict on {}: voted {vote:?} (evidence obtained: {obtained})",
                                short(&r.auditor),
                                short(&r.provider)
                            );
                        }
                        Ok(false) => {
                            log::warn!(target: TARGET, "spot check {id}: the vote was refused")
                        }
                        Err(e) => {
                            log::warn!(target: TARGET, "spot check {id}: vote not sent: {e:#}")
                        }
                    }
                }
                SpotDecision::Wait => {}
                SpotDecision::Abstain(why) => {
                    log::warn!(target: TARGET, "spot check {id}: not voting: {why}");
                }
            }
        }
    }

    /// What the evidence of spot check `id`'s verdict shows.
    pub async fn examine_spot(&self, id: u64, r: &SpotCheck) -> SpotFinding {
        let Ok(verdicts) = self.chain.verdicts(r.round, &r.provider).await else {
            return SpotFinding::Unreachable;
        };
        let Some(v) = verdicts.into_iter().find(|v| v.auditor == r.auditor) else {
            return SpotFinding::Unobtained;
        };
        let Some(commitment) = v.evidence else {
            return SpotFinding::Unobtained;
        };
        if v.thresholds_version != AUDIT_THRESHOLDS.version {
            return SpotFinding::CannotJudge;
        }
        let Ok(Some(endpoint)) = self.chain.endpoint(&r.auditor).await else {
            return SpotFinding::Unreachable;
        };
        let bytes = match self.fetch.fetch(&endpoint, id, commitment).await {
            Ok(EvidenceResponse::Evidence(b)) => b,
            Ok(EvidenceResponse::Refused) | Err(_) => return SpotFinding::Unreachable,
        };
        let Ok(evidence) = AuditEvidence::open(&bytes, &commitment) else {
            return SpotFinding::Unobtained;
        };
        let hash = ac_crypto::hash::blake3_256(&evidence.receipt.encode());
        if hash != v.receipt_hash.0 || evidence.receipt.body.provider != r.provider {
            return SpotFinding::Unobtained;
        }
        let model = evidence.receipt.body.model;
        let Some(engine_model) = self.engines.engine_model(model) else {
            return SpotFinding::CannotJudge;
        };
        // Evidence that opens but cannot be re-checked does not back the verdict.
        let Ok(case) = case_of(&evidence, &engine_model) else {
            return SpotFinding::Differs;
        };
        let quant = match self.chain.model_quant(model).await {
            Ok(Some(q)) => q,
            Ok(None) => return SpotFinding::Differs,
            Err(_) => return SpotFinding::Unreachable,
        };
        let report = self.engines.recheck(&case, quant).await;
        if report.verdict == Outcome::Inconclusive(Inconclusive::Engine) {
            return SpotFinding::EngineDown;
        }
        // Evidence whose content does not open its receipt does not back the verdict
        // (m6-token-binding design D6).
        if onchain_outcome(&report.verdict).is_some_and(|o| same_outcome(&v.outcome, &o)) {
            SpotFinding::Matches
        } else {
            SpotFinding::Differs
        }
    }
}

#[cfg(test)]
mod tests {
    use ac_primitives::market::audit::{AuditMetric, FailReason, InconclusiveReason};

    use super::*;

    #[test]
    fn the_finding_decides_the_vote() {
        use SpotFinding::{CannotJudge, Differs, EngineDown, Matches, Unobtained, Unreachable};
        let vote = |v, obtained| SpotDecision::Vote(v, obtained);
        assert_eq!(decide_spot(Matches, false), vote(ReviewVote::Uphold, true));
        assert_eq!(
            decide_spot(Differs, false),
            vote(ReviewVote::Overturn, true)
        );
        assert_eq!(
            decide_spot(Unobtained, false),
            vote(ReviewVote::Overturn, false)
        );
        // Spec "拿不到证据" (spot check): retried, then overturned at the deadline.
        assert_eq!(decide_spot(Unreachable, false), SpotDecision::Wait);
        assert_eq!(
            decide_spot(Unreachable, true),
            vote(ReviewVote::Overturn, false)
        );
        assert_eq!(decide_spot(EngineDown, false), SpotDecision::Wait);
        assert!(matches!(
            decide_spot(EngineDown, true),
            SpotDecision::Abstain(_)
        ));
        assert!(matches!(
            decide_spot(CannotJudge, false),
            SpotDecision::Abstain(_)
        ));
    }

    #[test]
    fn outcomes_compare_pass_and_fail_only() {
        let fail = |chunk, metric| VerdictOutcome::Fail(FailReason::Threshold { chunk, metric });
        let pass = VerdictOutcome::Pass;
        assert!(same_outcome(&pass, &pass));
        assert!(same_outcome(
            &fail(1, AuditMetric::ExpMismatches),
            &fail(3, AuditMetric::MantissaMean)
        ));
        assert!(!same_outcome(&pass, &fail(1, AuditMetric::ExpMismatches)));
        assert!(!same_outcome(&fail(1, AuditMetric::ExpMismatches), &pass));
        let tokens = VerdictOutcome::Inconclusive(InconclusiveReason::InputMismatch);
        assert!(!same_outcome(&pass, &tokens));
    }
}
