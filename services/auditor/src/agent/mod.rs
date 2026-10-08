//! The auditor agent's service mode, `ac-auditor run` (m6-auditor-agent; spec
//! `market/auditor-agent` "审计员代理服务" and the requirements after it): mystery-shopper
//! requests to the assigned providers, re-checks and verdicts, the evidence service, reviews of
//! disputes and votes. The decisions live here, behind the traits of [`ports`], so tests drive
//! them with stand-ins; [`live`] wires them to the node, the gateways and the engines.

pub mod config;
pub mod live;
pub mod ports;
pub mod prompts;
pub mod rand;
pub mod review;
pub mod server;
pub mod store;

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use ac_market_proto::toploc::AUDIT_THRESHOLDS;
use ac_primitives::market::ModelId;
use ac_primitives::market::audit::{RoundIndex, VerdictOutcome};
use pallet_audit::VerdictSubmission;
use serde_json::{Value, json};
use sp_runtime::AccountId32;
use tokio::sync::Mutex;

use crate::case::{CaseUsage, RecheckCase};
use crate::evidence::{evidence_of, onchain_outcome};
use crate::logging::TARGET;
use ports::{AuditCall, AuditChain, FetchEvidence, Recheck, Shop};
use prompts::Prompts;
use rand::Rand;
use store::EvidenceStore;

/// Attempts of one audit request (the first and up to two retries through other routes).
pub const ATTEMPTS: u32 = 3;

/// Prompt token counts per generated audit prompt before the audit gives up (design D9 of
/// m6-toploc-gpu-calibration).
pub const PROMPT_ATTEMPTS: u32 = 20;

/// Counters the agent reports (numbers only).
#[derive(Debug, Default)]
pub struct Counters {
    /// Audit requests that were answered and re-checked.
    pub audited: AtomicU64,
    /// Assigned providers skipped for want of a re-checkable model.
    pub skipped_no_model: AtomicU64,
    /// Assigned providers left without a verdict (requests or submissions failed).
    pub missed: AtomicU64,
    /// Verdicts accepted on chain.
    pub verdicts: AtomicU64,
    /// Votes cast.
    pub votes: AtomicU64,
    /// Audits given up for want of a prompt in the audit length band.
    pub skipped_no_prompt: AtomicU64,
}

fn bump(c: &AtomicU64) {
    c.fetch_add(1, Ordering::Relaxed);
}

/// The agent's state and collaborators.
pub struct Agent {
    /// The auditor account.
    pub me: AccountId32,
    /// The chain.
    pub chain: Arc<dyn AuditChain>,
    /// Buying requests.
    pub shop: Arc<dyn Shop>,
    /// Re-check engines.
    pub engines: Arc<dyn Recheck>,
    /// Fetching evidence from accusers.
    pub fetch: Arc<dyn FetchEvidence>,
    /// This agent's evidence.
    pub store: EvidenceStore,
    /// Audit prompts.
    pub prompts: Prompts,
    /// Choices.
    pub rng: Mutex<Box<dyn Rand>>,
    /// Share of a round kept free at its end for re-checking and submitting, in percent.
    pub margin_percent: u8,
    /// Counters.
    pub counters: Counters,
    /// Per-dispute review progress.
    pub reviews: Mutex<BTreeMap<u64, review::Progress>>,
}

/// One planned audit: a provider and the block to send its request at.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Planned {
    /// The provider.
    pub provider: AccountId32,
    /// When to send.
    pub at: u32,
}

/// Spreads the audits of a round `[start, next)` uniformly over its first part, keeping
/// `margin_percent` of the round free at the end (design D5). Providers the auditor already
/// submitted on are left out.
pub fn plan(
    (start, next): (u32, u32),
    assigned: &[(AccountId32, bool)],
    margin_percent: u8,
    rng: &mut dyn Rand,
) -> Vec<Planned> {
    let length = next.saturating_sub(start);
    let margin = length.saturating_mul(u32::from(margin_percent.min(100))) / 100;
    let usable = length.saturating_sub(margin.max(1));
    assigned
        .iter()
        .filter(|(_, done)| !done)
        .map(|(p, _)| Planned {
            provider: p.clone(),
            at: start
                .saturating_add(u32::try_from(rng.below(u64::from(usable))).unwrap_or_default()),
        })
        .collect()
}

/// How an audit of one provider ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Audited {
    /// A verdict was accepted on chain.
    Submitted(VerdictOutcome),
    /// The provider has no model this agent can re-check.
    NoModel,
    /// The chain accepts another thresholds version than this agent's.
    ThresholdsVersion {
        /// The chain's.
        chain: u16,
        /// This agent's.
        ours: u16,
    },
    /// No request could be bought, or the verdict was not accepted (reason, no content).
    Missed(String),
}

/// The re-check case of a bought answer.
fn case_of_answer(
    model: ModelId,
    engine_model: &str,
    messages: &Value,
    done: &ac_wallet::proxy::Completed,
) -> Option<RecheckCase> {
    use parity_scale_codec::Encode;
    let answer: Value = serde_json::from_slice(&done.response).ok()?;
    let choice = answer.get("choices")?.get(0)?;
    Some(RecheckCase {
        model: format!("0x{}", hex::encode(model.0)),
        engine_model: engine_model.to_owned(),
        messages: messages.clone(),
        output: choice
            .get("message")?
            .get("content")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        finish_reason: choice.get("finish_reason")?.as_str()?.to_owned(),
        usage: CaseUsage {
            prompt_tokens: done.receipt.body.in_tokens,
            completion_tokens: done.receipt.body.out_tokens,
        },
        receipt: hex::encode(done.receipt.encode()),
        toploc: done.proofs.as_ref().map(|p| hex::encode(p.encode())),
    })
}

impl Agent {
    async fn below(&self, n: u64) -> u64 {
        self.rng.lock().await.below(n)
    }

    /// Audits `provider` in `round` (spec "神秘顾客请求", "自动复核与提交裁决"): picks a model it
    /// can re-check, buys one pinned request through a random usable route, re-checks the
    /// answer and submits the verdict; a failure's evidence is stored before the verdict is
    /// sent. Logs only IDs, outcomes and reasons.
    pub async fn audit(&self, round: RoundIndex, provider: &AccountId32) -> Audited {
        let result = self.audit_inner(round, provider).await;
        match &result {
            Audited::Submitted(_) => bump(&self.counters.verdicts),
            Audited::NoModel => bump(&self.counters.skipped_no_model),
            Audited::ThresholdsVersion { .. } | Audited::Missed(_) => bump(&self.counters.missed),
        }
        log::info!(target: TARGET, "round {round}: audit of {}: {result:?}", short(provider));
        result
    }

    async fn audit_inner(&self, round: RoundIndex, provider: &AccountId32) -> Audited {
        let models: Vec<(ModelId, String)> = match self.chain.provider_models(provider).await {
            Ok(m) => m
                .into_iter()
                .filter_map(|m| self.engines.engine_model(m).map(|e| (m, e)))
                .collect(),
            Err(e) => return Audited::Missed(format!("provider lookup: {e:#}")),
        };
        if models.is_empty() {
            return Audited::NoModel;
        }
        let pick = usize::try_from(self.below(models.len() as u64).await).unwrap_or(0);
        let Some((model, engine_model)) = models.get(pick).cloned() else {
            return Audited::NoModel;
        };
        // Spec "自动复核与提交裁决" / "阈值版本不符": checked before paying for a request.
        let chain_version = match self.chain.params().await {
            Ok(Some((_, a))) => a.thresholds_version,
            Ok(None) => return Audited::Missed("audits are not configured".into()),
            Err(e) => return Audited::Missed(format!("parameters: {e:#}")),
        };
        if chain_version != AUDIT_THRESHOLDS.version {
            log::warn!(
                target: TARGET,
                "the chain accepts thresholds version {chain_version}, this agent judges by {}; not auditing",
                AUDIT_THRESHOLDS.version
            );
            return Audited::ThresholdsVersion {
                chain: chain_version,
                ours: AUDIT_THRESHOLDS.version,
            };
        }
        let quant = match self.chain.model_quant(model).await {
            Ok(Some(q)) => q,
            Ok(None) => return Audited::Missed("the model is not registered".into()),
            Err(e) => return Audited::Missed(format!("model lookup: {e:#}")),
        };
        let (messages, max_tokens, tokens) = match self.prompt(model).await {
            Ok(p) => p,
            Err(why) => return Audited::Missed(why),
        };
        log::info!(target: TARGET, "round {round}: audit prompt of {tokens} tokens");
        let body = json!({
            "model": format!("0x{}", hex::encode(model.0)),
            "messages": messages,
            "max_tokens": max_tokens,
        })
        .to_string();
        let Some(done) = self.buy(body.as_bytes(), provider).await else {
            return Audited::Missed("no request could be bought".into());
        };
        let Some(case) = case_of_answer(model, &engine_model, &messages, &done) else {
            return Audited::Missed("the answer is not a chat completion".into());
        };
        let report = self.engines.recheck(&case, quant).await;
        bump(&self.counters.audited);
        let outcome = onchain_outcome(&report.verdict);
        let evidence = match outcome {
            VerdictOutcome::Fail(_) => match self.keep_evidence(round, &case) {
                Ok(c) => Some(c),
                Err(e) => return Audited::Missed(format!("evidence: {e:#}")),
            },
            _ => None,
        };
        let verdict = VerdictSubmission {
            provider: provider.clone(),
            round,
            outcome,
            thresholds_version: report.thresholds_version,
            evidence,
            receipt: done.receipt,
        };
        match self
            .chain
            .submit(AuditCall::Verdict(Box::new(verdict)))
            .await
        {
            Ok(true) => Audited::Submitted(outcome),
            Ok(false) => Audited::Missed("the chain refused the verdict".into()),
            Err(e) => Audited::Missed(format!("submission: {e:#}")),
        }
    }

    /// An audit prompt for `model` whose token count is in the audit length band (spec
    /// "审计 prompt"): a screened bank entry, or a generated prompt that is lengthened while
    /// below the band and generated anew when above it, counted at most [`PROMPT_ATTEMPTS`]
    /// times. Returns the messages, the `max_tokens` and the prompt token count.
    async fn prompt(&self, model: ModelId) -> Result<(Value, u32, u32), String> {
        let band = AUDIT_THRESHOLDS.band;
        let draw = {
            let mut rng = self.rng.lock().await;
            self.prompts.next(rng.as_mut(), model)
        };
        if let Some(tokens) = draw.tokens {
            return Ok((draw.messages, draw.max_tokens, tokens));
        }
        let mut messages = draw.messages;
        // Aim at the band's middle; a word is at least about one token in common tokenizers,
        // so three words in four of the missing tokens rarely overshoot.
        let target = band
            .min
            .saturating_add(band.max.saturating_sub(band.min) / 2);
        for _ in 0..PROMPT_ATTEMPTS {
            let tokens = self
                .engines
                .prompt_tokens(model, &messages)
                .await
                .map_err(|e| format!("counting the prompt: {e:#}"))?;
            if band.contains(tokens) {
                return Ok((messages, draw.max_tokens, tokens));
            }
            let mut rng = self.rng.lock().await;
            if tokens > band.max {
                messages = prompts::generate(rng.as_mut());
            } else {
                let missing = usize::try_from(target.saturating_sub(tokens)).unwrap_or(0);
                prompts::lengthen(&mut messages, rng.as_mut(), missing.saturating_mul(3) / 4);
            }
        }
        bump(&self.counters.skipped_no_prompt);
        log::warn!(
            target: TARGET,
            "no audit prompt of {}–{} tokens after {PROMPT_ATTEMPTS} counts",
            band.min,
            band.max
        );
        Err("no prompt in the audit length band".into())
    }

    /// Stores a failing case's evidence and returns its commitment.
    fn keep_evidence(&self, round: RoundIndex, case: &RecheckCase) -> anyhow::Result<[u8; 32]> {
        let e = evidence_of(case)?;
        let c = e.commitment().map_err(|e| anyhow::anyhow!("{e}"))?;
        self.store.put(round, &c, &e.to_bytes())?;
        Ok(c)
    }

    /// One pinned request through a random usable route; up to [`ATTEMPTS`] routes.
    async fn buy(
        &self,
        body: &[u8],
        provider: &AccountId32,
    ) -> Option<ac_wallet::proxy::Completed> {
        let mut tried = Vec::new();
        for _ in 0..ATTEMPTS {
            let mut usable = Vec::new();
            for r in 0..self.shop.routes() {
                if !tried.contains(&r) && self.shop.usable(r).await {
                    usable.push(r);
                }
            }
            if usable.is_empty() {
                if tried.is_empty() {
                    log::warn!(target: TARGET, "no payment account can pay for audit requests");
                }
                return None;
            }
            let i = usize::try_from(self.below(usable.len() as u64).await).unwrap_or(0);
            let route = usable.get(i).copied()?;
            tried.push(route);
            match self.shop.buy(route, body, provider).await {
                Ok(done) => return Some(done),
                Err((code, _)) => {
                    log::info!(target: TARGET, "audit request to {} failed: {}", short(provider), code.as_str());
                }
            }
        }
        None
    }
}

/// Start-up checks (spec "审计员代理服务"): the auditor is registered and active, no payment
/// account is the auditor account, every re-checkable model is registered.
///
/// # Errors
///
/// The first check that fails, with its reason.
pub async fn preflight(
    chain: &dyn AuditChain,
    me: &AccountId32,
    payers: &[AccountId32],
    models: &[ModelId],
) -> anyhow::Result<()> {
    if !chain.is_active_auditor(me).await? {
        anyhow::bail!(
            "{} is not a registered, active auditor (register with ac-wallet audit register)",
            ac_primitives::encode_address(me.as_ref())
        );
    }
    if payers.contains(me) {
        anyhow::bail!("a payment account must not be the auditor account");
    }
    for m in models {
        if chain.model_quant(*m).await?.is_none() {
            anyhow::bail!("model 0x{} is not registered on chain", hex::encode(m.0));
        }
    }
    Ok(())
}

/// Screens the prompt bank for every re-checkable model (spec "审计 prompt" / "题库条目不在区间
/// 内"): counts each entry with the model's re-check engine and keeps those in the audit length
/// band. Logs the numbers skipped, never the content.
///
/// # Errors
///
/// A count that fails, or a model for which no entry is in the band.
pub async fn screen_bank(
    prompts: &mut Prompts,
    engines: &dyn Recheck,
    models: &[ModelId],
) -> anyhow::Result<()> {
    if prompts.bank().is_empty() {
        return Ok(());
    }
    let band = AUDIT_THRESHOLDS.band;
    for m in models {
        let mut counts = Vec::with_capacity(prompts.bank().len());
        for entry in prompts.bank() {
            counts.push(engines.prompt_tokens(*m, entry).await?);
        }
        let skipped = prompts.screen(*m, &counts, band)?;
        log::info!(
            target: TARGET,
            "model 0x{}: {skipped} of {} bank prompts are outside {}–{} tokens and are skipped",
            hex::encode(m.0),
            counts.len(),
            band.min,
            band.max
        );
    }
    Ok(())
}

/// Registers `wanted` as this auditor's evidence endpoint unless the chain already has it
/// (spec "审计员代理服务" / "自动登记证据地址"). Returns whether a transaction was sent.
///
/// # Errors
///
/// Chain failures, or the chain refusing the endpoint.
pub async fn sync_endpoint(
    chain: &dyn AuditChain,
    me: &AccountId32,
    wanted: pallet_audit::AuditorEndpoint,
) -> anyhow::Result<bool> {
    if chain.endpoint(me).await?.as_ref() == Some(&wanted) {
        return Ok(false);
    }
    if !chain.submit(AuditCall::SetEndpoint(Some(wanted))).await? {
        anyhow::bail!("the chain refused the evidence endpoint");
    }
    Ok(true)
}

/// A short account form for logs.
pub(crate) fn short(a: &AccountId32) -> String {
    let b: &[u8] = a.as_ref();
    hex::encode(b.get(..4).unwrap_or_default())
}

#[cfg(test)]
pub(crate) mod tests;
