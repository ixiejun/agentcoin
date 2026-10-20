//! Re-checking one inference (spec `market/auditor-agent`; design D5 of m6-toploc-verify).
//!
//! The re-check takes the tokens the receipt's content commitment binds (m6-token-binding design
//! D5): it never re-tokenizes the answer, so an honest answer whose text re-tokenizes to other
//! IDs passes (I-023), and a provider that computed another prompt fails.

use std::time::Duration;

use ac_market_proto::engine::request_id_header;
use ac_market_proto::toploc::{
    AUDIT_THRESHOLDS, MARKET_PARAMS, Thresholds, ToplocError, ToplocProofs, check, judge,
};
use ac_market_proto::{ReceiptTokens, check_content};
use ac_primitives::market::model::QuantType;
use ac_primitives::market::{ModelId, SignedReceipt};
use ac_toploc::{Phase, ProofPoly, compare_from_candidates};
use rand_core::TryRng;

use crate::case::{
    ChunkMetrics, FailReason, Inconclusive, Outcome, RecheckCase, Report, StatsReport,
};
use crate::engine::EngineClient;
use crate::logging::TARGET;
use crate::socket::Rows;

/// How long a re-check waits for the engine's rows after its answer.
pub const ROWS_WAIT: Duration = Duration::from_secs(30);

/// The re-check engine and its plugin socket.
pub struct Verifier {
    /// The engine (verify-mode plugin loaded).
    pub engine: EngineClient,
    /// Where its plugin sends the rows.
    pub rows: Rows,
    /// The thresholds judged by.
    pub thresholds: Thresholds,
    /// How long to wait for the rows after the engine's answer.
    pub rows_wait: Duration,
}

impl Verifier {
    /// A verifier judging by [`AUDIT_THRESHOLDS`].
    #[must_use]
    pub const fn new(engine: EngineClient, rows: Rows) -> Self {
        Self {
            engine,
            rows,
            thresholds: AUDIT_THRESHOLDS,
            rows_wait: ROWS_WAIT,
        }
    }

    /// Re-checks `case` for a model registered with the precision `quant`. Never fails: every
    /// problem is an outcome. Logs only IDs, counts, metrics and the verdict.
    pub async fn recheck(&self, case: &RecheckCase, quant: QuantType) -> Report {
        let receipt = case.signed_receipt().ok();
        let (outcome, chunks, prompt_tokens) = self.run(case, receipt.as_ref(), quant).await;
        let request = receipt.as_ref().map(|r| hex::encode(r.body.request_id));
        log::info!(
            target: TARGET,
            "re-check {}: {} {} ({} chunks, thresholds v{})",
            request.as_deref().unwrap_or("?"),
            outcome.kind(),
            outcome.reason(),
            chunks.len(),
            self.thresholds.version
        );
        Report {
            outcome: outcome.kind(),
            reason: outcome.reason(),
            request,
            thresholds_version: self.thresholds.version,
            stats: StatsReport::of(&outcome, prompt_tokens, &chunks),
            prompt_tokens,
            chunks,
            verdict: outcome,
        }
    }

    async fn run(
        &self,
        case: &RecheckCase,
        receipt: Option<&SignedReceipt>,
        quant: QuantType,
    ) -> (Outcome, Vec<ChunkMetrics>, Option<u32>) {
        let fail = |why| (Outcome::Fail(why), Vec::new(), None);
        let inconclusive = |why| (Outcome::Inconclusive(why), Vec::new(), None);
        // 1. The receipt is the one of this case.
        let Some(receipt) = receipt else {
            return inconclusive(Inconclusive::InputMismatch);
        };
        let body = &receipt.body;
        let model = ac_wallet::market::parse_model_id(&case.model).ok();
        if model != Some(body.model)
            || body.in_tokens != case.usage.prompt_tokens
            || body.out_tokens != case.usage.completion_tokens
        {
            return inconclusive(Inconclusive::InputMismatch);
        }
        // 1b. The messages, salt and token IDs open the receipt's content commitment (spec
        // "复核一次推理"); otherwise the case says nothing about the provider.
        let Ok(committed) = case.receipt_tokens() else {
            return (Outcome::ContentMismatch, Vec::new(), None);
        };
        if check_content(body, committed.as_ref(), case.message_list()).is_err() {
            return (Outcome::ContentMismatch, Vec::new(), None);
        }
        // 2. Only bfloat16 models are re-checked (spec "只复核 bfloat16 模型").
        if quant != QuantType::Bf16 {
            return inconclusive(Inconclusive::UnsupportedPrecision);
        }
        // 3. The proofs (spec "缺少证明即不通过").
        let Ok(proofs) = case.proofs() else {
            return fail(FailReason::CommitmentMismatch);
        };
        let proofs = match (body.toploc_commit == [0; 32], proofs) {
            (true, None) => return fail(FailReason::NoProof),
            (_, p) => p,
        };
        if let Err(e) = check(body, proofs.as_ref()) {
            let reason = match e {
                ToplocError::Missing => FailReason::NoProof,
                ToplocError::WrongParams | ToplocError::ChunkCount { .. } => {
                    FailReason::ParamsOrChunks
                }
                _ => FailReason::CommitmentMismatch,
            };
            return fail(reason);
        }
        let Some(proofs) = proofs.as_ref().and_then(decode_proofs) else {
            return fail(FailReason::CommitmentMismatch);
        };
        // 4. The committed tokens (spec "按承诺的 token 复算"); proofs come with tokens, as the
        // content check above made sure.
        let Some(committed) = committed else {
            return fail(FailReason::NoProof);
        };
        let tokens = match self.tokens(case, &committed).await {
            Ok(Ok(t)) => t,
            Ok(Err(outcome)) => return (outcome, Vec::new(), None),
            Err(_) => return inconclusive(Inconclusive::Engine),
        };
        // 5. Prefill and collect one row per token.
        let Some(mut rows) = self.prefill(&case.engine_model, &tokens.sequence).await else {
            return inconclusive(Inconclusive::Engine);
        };
        for r in rows.iter_mut().skip(tokens.prompt) {
            r.phase = Phase::Decode;
        }
        // 6. Compare and judge: the prefill bounds depend on the prompt's length (thresholds
        // version 3), the prompt the auditor re-created and checked against the receipt.
        let Ok(prompt_tokens) = u32::try_from(tokens.prompt) else {
            return inconclusive(Inconclusive::InputMismatch);
        };
        match compare_from_candidates(&rows, &proofs, &MARKET_PARAMS) {
            Ok(cmp) => {
                let mut outcome =
                    Outcome::from_judgement(judge(&cmp, &self.thresholds, prompt_tokens));
                // Only the answer's text is not committed: changing it can turn a pass into
                // "inconclusive", never into a failure.
                if outcome == Outcome::Pass && !tokens.answer_matches {
                    outcome = Outcome::Inconclusive(Inconclusive::OutputMismatch);
                }
                (
                    outcome,
                    cmp.iter().map(ChunkMetrics::from).collect(),
                    Some(prompt_tokens),
                )
            }
            Err(_) => inconclusive(Inconclusive::Engine),
        }
    }

    /// The re-computed sequence from the committed tokens, or the outcome that stops the
    /// re-check: the prompt is not the chat template of the messages (a failure), or there is
    /// no output token.
    async fn tokens(
        &self,
        case: &RecheckCase,
        committed: &ReceiptTokens,
    ) -> anyhow::Result<Result<Tokens, Outcome>> {
        let model = &case.engine_model;
        let prompt = self.engine.chat_tokens(model, &case.messages).await?;
        if prompt != committed.prompt_ids {
            return Ok(Err(Outcome::Fail(FailReason::PromptMismatch)));
        }
        let output = &committed.output_ids;
        let Some(fed) = output.len().checked_sub(1) else {
            return Ok(Err(Outcome::Inconclusive(Inconclusive::InputMismatch)));
        };
        // The answer shows every token but a final end token (`stop`), which is special.
        let shown = if case.finish_reason == "stop" {
            output.get(..fed).unwrap_or_default()
        } else {
            output.as_slice()
        };
        let answer_matches = self.engine.detokenize(model, shown).await? == case.output;
        // The last generated token is never fed back, whatever the finish reason.
        let mut sequence = prompt;
        sequence.extend(output.iter().take(fed));
        Ok(Ok(Tokens {
            prompt: committed.prompt_ids.len(),
            sequence,
            answer_matches,
        }))
    }

    /// The rows of `sequence` from the engine, one per token.
    async fn prefill(&self, model: &str, sequence: &[u32]) -> Option<Vec<ac_toploc::Segment>> {
        let mut id = [0u8; 32];
        let mut rng = ac_crypto::OsRng::new().ok()?;
        rng.try_fill_bytes(&mut id).ok()?;
        self.rows.expect(id);
        let counted = self
            .engine
            .prefill(model, sequence, &request_id_header(&id))
            .await;
        // A failed request sends no end marker: stop waiting at once.
        let wait = if counted.is_ok() {
            self.rows_wait
        } else {
            Duration::ZERO
        };
        let rows = self.rows.take(id, wait).await?;
        let counted = usize::try_from(counted.ok()?).ok()?;
        if counted != sequence.len() || rows.len() != sequence.len() {
            log::warn!(
                target: TARGET,
                "re-check engine sent {} rows for {} tokens",
                rows.len(),
                sequence.len()
            );
            return None;
        }
        Some(rows)
    }
}

struct Tokens {
    /// Prompt tokens at the start of `sequence`.
    prompt: usize,
    /// The prompt and every output token but the last.
    sequence: Vec<u32>,
    /// Whether the output tokens decode to the answer's text.
    answer_matches: bool,
}

fn decode_proofs(p: &ToplocProofs) -> Option<Vec<ProofPoly>> {
    p.proofs
        .iter()
        .map(|b| ProofPoly::from_bytes(b).ok())
        .collect()
}

/// The registered precision of `model`, from the chain.
///
/// # Errors
///
/// RPC failures and unknown models.
pub async fn model_quant(
    node: &ac_wallet::NodeClient,
    model: ModelId,
) -> anyhow::Result<QuantType> {
    let m = node
        .market_model(model)
        .await?
        .ok_or_else(|| anyhow::anyhow!("model {model:?} is not registered"))?;
    Ok(m.manifest.quant)
}
