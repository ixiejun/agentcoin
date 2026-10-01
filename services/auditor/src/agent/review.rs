//! Reviews of disputes (spec `market/auditor-agent` "升级复核与投票"; design D7): for each
//! accuser, fetch its evidence, open it against the commitment on chain, check that its receipt
//! is the verdict's, and re-check it by the verdict's thresholds version. Two distinct accusers
//! whose evidence fails confirm; anything else rejects. An unavailable engine of this agent is
//! not the accusers' fault: the agent retries and, if it never recovers, does not vote.

use std::collections::BTreeMap;

use ac_market_proto::audit::{AuditEvidence, EvidenceResponse};
use ac_market_proto::toploc::AUDIT_THRESHOLDS;
use ac_primitives::market::audit::Vote;
use parity_scale_codec::Encode;
use sp_runtime::AccountId32;

use super::ports::{AuditCall, Dispute};
use super::{Agent, bump, short};
use crate::case::{Inconclusive, Outcome};
use crate::evidence::case_of;
use crate::logging::TARGET;

/// What one accuser's evidence showed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Finding {
    /// It re-checks as a failure.
    Fails,
    /// It does not hold up: missing, not matching, not the verdict's receipt, or not failing.
    DoesNotFail,
    /// Not reachable yet (no endpoint, refused, transport error): retried until the deadline.
    Unreachable,
    /// This agent's engine failed: retried; never counted against the accuser.
    EngineDown,
    /// This agent cannot judge it at all (unknown thresholds version or model).
    CannotJudge,
}

/// Review state of one dispute: the definitive findings so far, by accuser.
#[derive(Clone, Debug, Default)]
pub struct Progress {
    findings: BTreeMap<AccountId32, Finding>,
}

/// The decision about a dispute for now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    /// Cast this vote.
    Vote(Vote),
    /// Try again later.
    Wait,
    /// Do not vote (and say why).
    Abstain(&'static str),
}

/// Combines the accusers' findings (`final_try`: the deadline is near).
#[must_use]
pub fn decide(findings: &[Finding], final_try: bool) -> Decision {
    let fails = findings.iter().filter(|f| **f == Finding::Fails).count();
    if fails >= 2 {
        return Decision::Vote(Vote::Confirm);
    }
    if findings.contains(&Finding::CannotJudge) {
        return Decision::Abstain("cannot judge the evidence (thresholds version or model)");
    }
    let engine_down = findings.contains(&Finding::EngineDown);
    let unreachable = findings.contains(&Finding::Unreachable);
    if !final_try && (engine_down || unreachable) {
        return Decision::Wait;
    }
    if engine_down {
        return Decision::Abstain("the re-check engine is unavailable");
    }
    // Evidence still unreachable at the deadline counts as missing.
    Decision::Vote(Vote::Reject)
}

impl Agent {
    /// Reviews every open dispute this auditor was drawn for and has not voted in.
    pub async fn review_all(&self) {
        let Ok(open) = self.chain.open_disputes().await else {
            return;
        };
        let Ok(now) = self.chain.best_block().await else {
            return;
        };
        for (provider, id) in open {
            let Ok(Some(d)) = self.chain.dispute(id).await else {
                continue;
            };
            let pending = d
                .reviewers
                .iter()
                .any(|(r, v)| *r == self.me && v.is_none());
            if !pending || d.outcome.is_some() || now > d.deadline {
                continue;
            }
            let final_try = now.saturating_add(2) >= d.deadline;
            match self.review(id, &d, final_try).await {
                Decision::Vote(vote) => {
                    let call = AuditCall::Vote {
                        provider: provider.clone(),
                        id,
                        vote,
                    };
                    match self.chain.submit(call).await {
                        Ok(true) => {
                            bump(&self.counters.votes);
                            log::info!(target: TARGET, "dispute {id} on {}: voted {vote:?}", short(&provider));
                            self.reviews.lock().await.remove(&id);
                        }
                        Ok(false) => {
                            log::warn!(target: TARGET, "dispute {id}: the vote was refused")
                        }
                        Err(e) => log::warn!(target: TARGET, "dispute {id}: vote not sent: {e:#}"),
                    }
                }
                Decision::Wait => {}
                Decision::Abstain(why) => {
                    log::warn!(target: TARGET, "dispute {id}: not voting: {why}");
                }
            }
        }
    }

    /// Reviews one dispute.
    pub async fn review(&self, id: u64, d: &Dispute, final_try: bool) -> Decision {
        let mut findings = Vec::new();
        for accuser in &d.accusers {
            let known = self
                .reviews
                .lock()
                .await
                .get(&id)
                .and_then(|p| p.findings.get(&accuser.auditor).copied());
            let f = match known {
                Some(f) => f,
                None => {
                    let f = self.examine(id, d, &accuser.auditor, accuser.round).await;
                    // Only definitive findings are kept; the others are tried again.
                    if matches!(
                        f,
                        Finding::Fails | Finding::DoesNotFail | Finding::CannotJudge
                    ) {
                        self.reviews
                            .lock()
                            .await
                            .entry(id)
                            .or_default()
                            .findings
                            .insert(accuser.auditor.clone(), f);
                    }
                    f
                }
            };
            findings.push(f);
        }
        decide(&findings, final_try)
    }

    async fn examine(
        &self,
        id: u64,
        d: &Dispute,
        accuser: &AccountId32,
        round: ac_primitives::market::audit::RoundIndex,
    ) -> Finding {
        let Ok(verdicts) = self.chain.verdicts(round, &d.provider).await else {
            return Finding::Unreachable;
        };
        let Some(v) = verdicts.into_iter().find(|v| v.auditor == *accuser) else {
            return Finding::DoesNotFail;
        };
        let Some(commitment) = v.evidence else {
            return Finding::DoesNotFail;
        };
        if v.thresholds_version != AUDIT_THRESHOLDS.version {
            return Finding::CannotJudge;
        }
        let Ok(Some(endpoint)) = self.chain.endpoint(accuser).await else {
            return Finding::Unreachable;
        };
        let bytes = match self.fetch.fetch(&endpoint, id, commitment).await {
            Ok(EvidenceResponse::Evidence(b)) => b,
            Ok(EvidenceResponse::Refused) | Err(_) => return Finding::Unreachable,
        };
        let Ok(evidence) = AuditEvidence::open(&bytes, &commitment) else {
            return Finding::DoesNotFail;
        };
        let hash = ac_crypto::hash::blake3_256(&evidence.receipt.encode());
        if hash != v.receipt_hash.0 || evidence.receipt.body.provider != d.provider {
            return Finding::DoesNotFail;
        }
        let model = evidence.receipt.body.model;
        let Some(engine_model) = self.engines.engine_model(model) else {
            return Finding::CannotJudge;
        };
        let Ok(case) = case_of(&evidence, &engine_model) else {
            return Finding::DoesNotFail;
        };
        let quant = match self.chain.model_quant(model).await {
            Ok(Some(q)) => q,
            Ok(None) => return Finding::DoesNotFail,
            Err(_) => return Finding::Unreachable,
        };
        match self.engines.recheck(&case, quant).await.verdict {
            Outcome::Fail(_) => Finding::Fails,
            Outcome::Inconclusive(Inconclusive::Engine) => Finding::EngineDown,
            _ => Finding::DoesNotFail,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_failing_accusers_confirm_anything_else_rejects() {
        use Finding::{CannotJudge, DoesNotFail, EngineDown, Fails, Unreachable};
        assert_eq!(
            decide(&[Fails, Fails], false),
            Decision::Vote(Vote::Confirm)
        );
        assert_eq!(
            decide(&[Fails, Fails, DoesNotFail], false),
            Decision::Vote(Vote::Confirm)
        );
        assert_eq!(
            decide(&[Fails, DoesNotFail], false),
            Decision::Vote(Vote::Reject)
        );
        // Spec "拿不到证据": retried, then rejected at the deadline.
        assert_eq!(decide(&[Fails, Unreachable], false), Decision::Wait);
        assert_eq!(
            decide(&[Fails, Unreachable], true),
            Decision::Vote(Vote::Reject)
        );
        // Spec "本机引擎不可用": retried, then no vote at all.
        assert_eq!(decide(&[Fails, EngineDown], false), Decision::Wait);
        assert!(matches!(
            decide(&[Fails, EngineDown], true),
            Decision::Abstain(_)
        ));
        assert!(matches!(
            decide(&[CannotJudge, Fails], false),
            Decision::Abstain(_)
        ));
    }
}
