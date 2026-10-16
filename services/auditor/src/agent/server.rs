//! The evidence service (spec `market/auditor-agent` "证据保存与交付"; design D6): reviewers
//! ask over a sealed channel for the evidence of a verdict cited by a dispute or a spot check;
//! the agent hands it over only when the chain shows the requester is a reviewer of that open
//! dispute or spot check, it cites a verdict of this auditor (a failure dispute's accuser, a
//! statistical dispute's verdict, or the spot-checked verdict), and the commitment is that
//! verdict's. Refusals never say why. Also the janitor that deletes evidence nothing can cite any
//! more.
//!
//! The request names its dispute or spot check by number alone, and the two are numbered apart,
//! so the agent checks both readings (m6-audit-hardening 7.4): each is a full entitlement check
//! on its own, and the wire format stays as published.

use std::collections::BTreeMap;

use ac_market_proto::audit::{EvidenceRequest, EvidenceResponse};
use ac_primitives::market::audit::{RoundIndex, SprtState, citation_window};
use sp_runtime::AccountId32;

use super::ports::AuditCall;
use super::{Agent, short};
use crate::logging::TARGET;

impl Agent {
    /// The answer to `requester`'s evidence request. Logs the dispute or spot check, the
    /// requester and the result only.
    pub async fn answer(&self, requester: &AccountId32, req: EvidenceRequest) -> EvidenceResponse {
        let granted = self.entitled(requester, &req).await
            || self.entitled_by_spot_check(requester, &req).await;
        let response = match granted.then(|| self.store.get(&req.commitment)).flatten() {
            Some(bytes) => EvidenceResponse::Evidence(bytes),
            None => EvidenceResponse::Refused,
        };
        log::info!(
            target: TARGET,
            "evidence request for dispute or spot check {} from {}: {}",
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

    /// Whether `req` names an open spot check of a verdict of this auditor with that
    /// commitment, which `requester` reviews.
    async fn entitled_by_spot_check(&self, requester: &AccountId32, req: &EvidenceRequest) -> bool {
        let Ok(Some(r)) = self.chain.spot_check(req.dispute).await else {
            return false;
        };
        if r.outcome.is_some()
            || r.auditor != self.me
            || !r.reviewers.iter().any(|(who, _)| who == requester)
        {
            return false;
        }
        let Ok(verdicts) = self.chain.verdicts(r.round, &r.provider).await else {
            return false;
        };
        verdicts
            .iter()
            .any(|v| v.auditor == self.me && v.evidence == Some(req.commitment))
    }

    /// Deletes evidence nothing can cite any more (spec "证据保存与交付"; design D6 of
    /// m6-auditor-agent, D11 of m6-audit-sprt, D9 of m6-audit-hardening): a verdict can be cited
    /// by a dispute or a spot check for `2R + V` blocks after its submission, which comes before
    /// the next round starts; after that its evidence goes once no open dispute or spot check
    /// cites it and the provider's statistical state no longer holds it. Disputes and spot
    /// checks past their deadline that cite this auditor are closed by it (anyone may).
    pub async fn tidy(&self) {
        let Ok(Some((current, start, next))) = self.chain.round().await else {
            return;
        };
        let Ok(now) = self.chain.best_block().await else {
            return;
        };
        let Ok(Some((params, _))) = self.chain.params().await else {
            return;
        };
        let round_blocks = next.saturating_sub(start);
        let window = citation_window(u64::from(round_blocks), u64::from(params.vote_blocks));
        // Whether the verdicts of `round` are past the window: submitted before round + 1
        // started, which was `current - round - 1` rounds before this one.
        let past_window = |round: RoundIndex| {
            let Some(back) = current.checked_sub(round.saturating_add(1)) else {
                return false;
            };
            let submitted_by = u64::from(start)
                .checked_sub(u64::from(back).saturating_mul(u64::from(round_blocks)));
            submitted_by.is_some_and(|b| u64::from(now) >= b.saturating_add(window))
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
        let Ok(spot_checks) = self.chain.open_spot_checks().await else {
            return;
        };
        for id in spot_checks {
            let Ok(Some(r)) = self.chain.spot_check(id).await else {
                // Unknown for now: keep everything.
                return;
            };
            if r.auditor != self.me {
                continue;
            }
            if now > r.deadline
                && matches!(
                    self.chain.submit(AuditCall::CloseSpotCheck { id }).await,
                    Ok(true)
                )
            {
                log::info!(target: TARGET, "closed undecided spot check {id}");
                continue;
            }
            mine_open.push((r.provider, r.round));
        }
        let mut states: BTreeMap<AccountId32, Vec<SprtState<AccountId32>>> = BTreeMap::new();
        for s in self.store.list() {
            if !past_window(s.round) {
                continue;
            }
            let held = match &s.provider {
                Some(p) => {
                    if mine_open.contains(&(p.clone(), s.round)) {
                        true
                    } else {
                        if !states.contains_key(p) {
                            let Ok(by_model) = self.chain.sprt_states(p).await else {
                                continue;
                            };
                            states
                                .insert(p.clone(), by_model.into_iter().map(|(_, s)| s).collect());
                        }
                        // In the participation list of the provider's state on any model.
                        states.get(p).is_some_and(|all| {
                            all.iter().any(|st| {
                                st.entries
                                    .iter()
                                    .any(|e| e.auditor == self.me && e.round == s.round)
                            })
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
