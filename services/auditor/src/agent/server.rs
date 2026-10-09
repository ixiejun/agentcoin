//! The evidence service (spec `market/auditor-agent` "证据保存与交付"; design D6): reviewers
//! ask over a sealed channel for the evidence of a dispute's verdict; the agent hands it over
//! only when the chain shows the requester is a reviewer of that open dispute, this auditor is
//! one of its accusers (for a statistical dispute, the auditor of one of its verdicts), and the
//! commitment is that of this auditor's verdict. Refusals never say why. Also the janitor that
//! deletes evidence no dispute can use any more.

use std::collections::BTreeMap;

use ac_market_proto::audit::{EvidenceRequest, EvidenceResponse};
use ac_primitives::market::audit::{RoundIndex, SprtState};
use sp_runtime::AccountId32;

use super::ports::AuditCall;
use super::{Agent, short};
use crate::logging::TARGET;

impl Agent {
    /// The answer to `requester`'s evidence request. Logs the dispute, the requester and the
    /// result only.
    pub async fn answer(&self, requester: &AccountId32, req: EvidenceRequest) -> EvidenceResponse {
        let granted = self.entitled(requester, &req).await;
        let response = match granted.then(|| self.store.get(&req.commitment)).flatten() {
            Some(bytes) => EvidenceResponse::Evidence(bytes),
            None => EvidenceResponse::Refused,
        };
        log::info!(
            target: TARGET,
            "evidence request for dispute {} from {}: {}",
            req.dispute,
            short(requester),
            if matches!(response, EvidenceResponse::Evidence(_)) { "served" } else { "refused" }
        );
        response
    }

    async fn entitled(&self, requester: &AccountId32, req: &EvidenceRequest) -> bool {
        let Ok(Some(d)) = self.chain.dispute(req.dispute).await else {
            return false;
        };
        if d.outcome.is_some() || !d.reviewers.iter().any(|(r, _)| r == requester) {
            return false;
        }
        for mine in d.accusers.iter().filter(|a| a.auditor == self.me) {
            let Ok(verdicts) = self.chain.verdicts(mine.round, &d.provider).await else {
                return false;
            };
            if verdicts
                .iter()
                .any(|v| v.auditor == self.me && v.evidence == Some(req.commitment))
            {
                return true;
            }
        }
        false
    }

    /// Deletes evidence no dispute can use any more (design D6 of m6-auditor-agent, D11 of
    /// m6-audit-sprt): a verdict of round `r` can open a failure dispute until round `r + 1`
    /// ends; after that its evidence goes once no open dispute names it and the provider's
    /// statistical state no longer holds it. Disputes past their deadline that name this
    /// auditor are closed by it (anyone may).
    pub async fn tidy(&self) {
        let Ok(Some((current, _, _))) = self.chain.round().await else {
            return;
        };
        let Ok(now) = self.chain.best_block().await else {
            return;
        };
        // (provider, round) of this auditor's verdicts that open disputes name.
        let mut mine_open: Vec<(AccountId32, RoundIndex)> = Vec::new();
        let Ok(open) = self.chain.open_disputes().await else {
            return;
        };
        for (provider, id) in open {
            let Ok(Some(d)) = self.chain.dispute(id).await else {
                // Unknown for now: keep everything.
                return;
            };
            let rounds: Vec<RoundIndex> = d
                .accusers
                .iter()
                .filter(|a| a.auditor == self.me)
                .map(|a| a.round)
                .collect();
            if rounds.is_empty() {
                continue;
            }
            if now > d.deadline {
                let call = AuditCall::Close {
                    provider: provider.clone(),
                    id,
                };
                if matches!(self.chain.submit(call).await, Ok(true)) {
                    log::info!(target: TARGET, "closed undecided dispute {id}");
                    continue;
                }
            }
            mine_open.extend(rounds.into_iter().map(|r| (provider.clone(), r)));
        }
        let mut states: BTreeMap<AccountId32, SprtState<AccountId32>> = BTreeMap::new();
        for s in self.store.list() {
            if current < s.round.saturating_add(2) {
                continue;
            }
            let held = match &s.provider {
                Some(p) => {
                    if mine_open.contains(&(p.clone(), s.round)) {
                        true
                    } else {
                        if !states.contains_key(p) {
                            let Ok(state) = self.chain.sprt_state(p).await else {
                                continue;
                            };
                            states.insert(p.clone(), state);
                        }
                        states.get(p).is_some_and(|st| {
                            st.entries
                                .iter()
                                .any(|e| e.auditor == self.me && e.round == s.round)
                        })
                    }
                }
                // Files of older agents only served failure disputes.
                None => mine_open.iter().any(|(_, r)| *r == s.round),
            };
            if !held {
                self.store.remove(&s);
                log::debug!(target: TARGET, "deleted the evidence of a round-{} verdict", s.round);
            }
        }
    }
}
