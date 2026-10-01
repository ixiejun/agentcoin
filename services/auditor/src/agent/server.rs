//! The evidence service (spec `market/auditor-agent` "证据保存与交付"; design D6): reviewers
//! ask over a sealed channel for the evidence of a dispute's verdict; the agent hands it over
//! only when the chain shows the requester is a reviewer of that open dispute, this auditor is
//! one of its accusers, and the commitment is that of this auditor's verdict. Refusals never
//! say why. Also the janitor that deletes evidence no dispute can use any more.

use ac_market_proto::audit::{EvidenceRequest, EvidenceResponse};
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
        let Some(mine) = d.accusers.iter().find(|a| a.auditor == self.me) else {
            return false;
        };
        let Ok(verdicts) = self.chain.verdicts(mine.round, &d.provider).await else {
            return false;
        };
        verdicts
            .iter()
            .any(|v| v.auditor == self.me && v.evidence == Some(req.commitment))
    }

    /// Deletes evidence no dispute can use any more (design D6): a verdict of round `r` can
    /// open a dispute until round `r + 1` ends; after that its evidence goes once every dispute
    /// naming it is closed. Disputes past their deadline that name this auditor are closed by
    /// it (anyone may).
    pub async fn tidy(&self) {
        let Ok(Some((current, _, _))) = self.chain.round().await else {
            return;
        };
        let Ok(now) = self.chain.best_block().await else {
            return;
        };
        let mut mine_open = Vec::new();
        if let Ok(open) = self.chain.open_disputes().await {
            for (provider, id) in open {
                let Ok(Some(d)) = self.chain.dispute(id).await else {
                    continue;
                };
                let Some(a) = d.accusers.iter().find(|a| a.auditor == self.me) else {
                    continue;
                };
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
                mine_open.push(a.round);
            }
        }
        for (round, commitment) in self.store.list() {
            if current >= round.saturating_add(2) && !mine_open.contains(&round) {
                self.store.remove(round, &commitment);
                log::debug!(target: TARGET, "deleted the evidence of a round-{round} verdict");
            }
        }
    }
}
