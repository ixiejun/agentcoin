//! What a re-check takes and what it gives (spec `market/auditor-agent` "复核一次推理").

use ac_market_proto::ReceiptTokens;
use ac_market_proto::toploc::{Judgement, Metric, ToplocProofs};
use ac_primitives::market::SignedReceipt;
use ac_primitives::market::audit::{AuditStats, CURRENT_STATS, ChunkMantissa, VerdictStats};
use ac_toploc::Comparison;
use anyhow::{Context, Result};
use parity_scale_codec::Decode;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Token counts the provider returned with the answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaseUsage {
    /// Prompt tokens.
    pub prompt_tokens: u32,
    /// Generated tokens.
    pub completion_tokens: u32,
}

/// One finished inference to re-check: the auditor's own request and what came back. Its
/// `Debug` output leaves out the messages and the output.
#[derive(Clone, Serialize, Deserialize)]
pub struct RecheckCase {
    /// On-chain model ID (`0x` + 64 hex digits).
    pub model: String,
    /// The model's name on the re-check engine.
    pub engine_model: String,
    /// The request's messages (Chat Completions format).
    pub messages: Value,
    /// The answer's text.
    pub output: String,
    /// The answer's finish reason (`stop` or `length`).
    pub finish_reason: String,
    /// The answer's token counts.
    pub usage: CaseUsage,
    /// The double-signed receipt, SCALE-encoded, hex.
    pub receipt: String,
    /// The TOPLOC proofs that came with it, SCALE-encoded, hex; none for an all-zero commitment.
    pub toploc: Option<String>,
    /// The salt and token IDs that came with it (`ReceiptTokens::to_json`), which open the
    /// receipt's content commitment; none for an all-zero commitment (m6-token-binding).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tokens: Option<Value>,
}

impl core::fmt::Debug for RecheckCase {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("RecheckCase")
            .field("model", &self.model)
            .field("usage", &self.usage)
            .finish_non_exhaustive()
    }
}

impl RecheckCase {
    /// The decoded receipt.
    ///
    /// # Errors
    ///
    /// Bad hex or SCALE.
    pub fn signed_receipt(&self) -> Result<SignedReceipt> {
        let raw = hex::decode(self.receipt.trim_start_matches("0x")).context("receipt hex")?;
        SignedReceipt::decode(&mut &raw[..]).context("receipt encoding")
    }

    /// The decoded proofs, if any.
    ///
    /// # Errors
    ///
    /// Bad hex or SCALE.
    pub fn proofs(&self) -> Result<Option<ToplocProofs>> {
        self.toploc
            .as_deref()
            .map(|h| {
                let raw = hex::decode(h.trim_start_matches("0x")).context("proofs hex")?;
                ToplocProofs::decode(&mut &raw[..]).context("proofs encoding")
            })
            .transpose()
    }

    /// The decoded salt and token IDs, if any.
    ///
    /// # Errors
    ///
    /// A malformed `tokens` field.
    pub fn receipt_tokens(&self) -> Result<Option<ReceiptTokens>> {
        self.tokens
            .as_ref()
            .filter(|t| !t.is_null())
            .map(|t| ReceiptTokens::from_json(t).map_err(|e| anyhow::anyhow!("tokens: {e}")))
            .transpose()
    }

    /// The messages as a list (empty if they are not an array).
    #[must_use]
    pub fn message_list(&self) -> &[Value] {
        self.messages.as_array().map_or(&[], Vec::as_slice)
    }
}

/// Why an inference fails its re-check.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FailReason {
    /// The receipt commits to no proofs (spec "缺少证明即不通过").
    NoProof,
    /// The proofs do not match the receipt's commitment.
    CommitmentMismatch,
    /// The proofs' parameters or their number are not the market's.
    ParamsOrChunks,
    /// The committed prompt tokens are not the registered model's chat template applied to the
    /// committed messages (m6-token-binding).
    PromptMismatch,
    /// A chunk is out of the thresholds.
    Threshold {
        /// The first chunk out of bounds (0 is the prefill).
        chunk: usize,
        /// The first bound it exceeds.
        metric: Metric,
    },
}

/// Why a re-check cannot decide.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Inconclusive {
    /// The receipt does not match the case (model, token counts) or does not decode.
    InputMismatch,
    /// The model's registered precision is not bfloat16.
    UnsupportedPrecision,
    /// The committed output tokens do not decode to the answer's text, while the
    /// re-computation passed (m6-token-binding).
    OutputMismatch,
    /// The re-check engine failed or is unavailable.
    Engine,
}

/// The verdict of a re-check.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Every chunk is within the thresholds.
    Pass,
    /// The inference fails.
    Fail(FailReason),
    /// No verdict.
    Inconclusive(Inconclusive),
    /// The case's messages, salt and token IDs do not open the receipt's content commitment:
    /// not a finding about the provider, and never a verdict (m6-token-binding).
    ContentMismatch,
}

impl Outcome {
    pub(crate) fn from_judgement(j: Judgement) -> Self {
        match j {
            Judgement::Fail { chunk, metric } => {
                Self::Fail(FailReason::Threshold { chunk, metric })
            }
            _ => Self::Pass,
        }
    }

    /// `pass`, `fail`, `inconclusive` or `mismatch` (the content does not open the receipt).
    #[must_use]
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::Pass => "pass",
            Self::Fail(_) => "fail",
            Self::Inconclusive(_) => "inconclusive",
            Self::ContentMismatch => "mismatch",
        }
    }

    /// Why (empty for a pass).
    #[must_use]
    pub fn reason(&self) -> String {
        match self {
            Self::Pass => String::new(),
            Self::Fail(FailReason::NoProof) => "no proof".into(),
            Self::Fail(FailReason::CommitmentMismatch) => {
                "proofs do not match the commitment".into()
            }
            Self::Fail(FailReason::ParamsOrChunks) => "proof parameters or chunk count".into(),
            Self::Fail(FailReason::PromptMismatch) => "prompt does not match the request".into(),
            Self::Fail(FailReason::Threshold { chunk, metric }) => {
                format!("chunk {chunk}: {metric}")
            }
            Self::Inconclusive(Inconclusive::InputMismatch) => "input mismatch".into(),
            Self::Inconclusive(Inconclusive::UnsupportedPrecision) => {
                "precision not supported".into()
            }
            Self::Inconclusive(Inconclusive::OutputMismatch) => {
                "answer does not match the tokens".into()
            }
            Self::ContentMismatch => "content does not match the receipt".into(),
            Self::Inconclusive(Inconclusive::Engine) => "re-check engine error".into(),
        }
    }
}

/// The metrics of one chunk, as reported.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct ChunkMetrics {
    /// Exponent mismatches.
    pub exp_mismatches: u32,
    /// Sum of mantissa errors where the exponent matches.
    pub mant_err_sum: u32,
    /// Positions whose exponent matches.
    pub mant_count: u32,
    /// Median mantissa error (`None` when no exponent matches).
    pub median: Option<u8>,
}

impl ChunkMetrics {
    /// The chunk's mantissa errors, as the statistics read them.
    #[must_use]
    pub const fn mantissa(&self) -> ChunkMantissa {
        ChunkMantissa {
            err_sum: self.mant_err_sum,
            count: self.mant_count,
        }
    }
}

impl From<&Comparison> for ChunkMetrics {
    fn from(c: &Comparison) -> Self {
        Self {
            exp_mismatches: c.exp_mismatches,
            mant_err_sum: c.mant_err_sum,
            mant_count: c.mant_count,
            median: c.median_upper,
        }
    }
}

/// The statistics of a re-check judged by the thresholds (OpenSpec change m6-audit-sprt design
/// D1), with the parameter version they are submitted under.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatsReport {
    /// The statistical judgment's parameter version.
    pub version: u16,
    /// Prompt tokens the re-check rebuilt.
    pub prompt_tokens: u32,
    /// The prefill chunk's mean mantissa error, in hundredths.
    pub prefill_mean_centi: u16,
    /// The decode chunks' means averaged, in hundredths.
    pub decode_mean_centi: u16,
    /// Decode chunks.
    pub decode_chunks: u16,
}

impl StatsReport {
    /// The statistics of a re-check with `outcome`, `prompt_tokens` and `chunks`, under the
    /// current parameter version: `None` unless the outcome was judged by the thresholds (a
    /// pass, or a chunk out of bounds) on at least one chunk.
    #[must_use]
    pub fn of(
        outcome: &Outcome,
        prompt_tokens: Option<u32>,
        chunks: &[ChunkMetrics],
    ) -> Option<Self> {
        let judged = matches!(
            outcome,
            Outcome::Pass | Outcome::Fail(FailReason::Threshold { .. })
        );
        if !judged {
            return None;
        }
        let mantissas: Vec<ChunkMantissa> = chunks.iter().map(ChunkMetrics::mantissa).collect();
        let s = AuditStats::from_chunks(prompt_tokens?, &mantissas)?;
        Some(Self {
            version: CURRENT_STATS.version,
            prompt_tokens: s.prompt_tokens,
            prefill_mean_centi: s.prefill_mean_centi,
            decode_mean_centi: s.decode_mean_centi,
            decode_chunks: s.decode_chunks,
        })
    }

    /// The statistics as a verdict carries them.
    #[must_use]
    pub const fn onchain(&self) -> VerdictStats {
        VerdictStats {
            version: self.version,
            stats: AuditStats {
                prompt_tokens: self.prompt_tokens,
                prefill_mean_centi: self.prefill_mean_centi,
                decode_mean_centi: self.decode_mean_centi,
                decode_chunks: self.decode_chunks,
            },
        }
    }
}

/// A re-check's result: the outcome, the thresholds' version, every chunk's metrics (empty
/// when no comparison ran) and, when judged by the thresholds, the statistics. Numbers only,
/// never content.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Report {
    /// `pass`, `fail` or `inconclusive`.
    pub outcome: &'static str,
    /// Why; empty for a pass.
    pub reason: String,
    /// The receipt's request ID (hex), when the receipt decodes.
    pub request: Option<String>,
    /// Version of the thresholds judged by.
    pub thresholds_version: u16,
    /// The prompt tokens the inference was judged with (they pick the prefill bounds); none when
    /// no comparison ran.
    pub prompt_tokens: Option<u32>,
    /// Per-chunk metrics.
    pub chunks: Vec<ChunkMetrics>,
    /// The statistics a verdict on it carries; none unless judged by the thresholds.
    pub stats: Option<StatsReport>,
    #[serde(skip)]
    /// The outcome itself.
    pub verdict: Outcome,
}
