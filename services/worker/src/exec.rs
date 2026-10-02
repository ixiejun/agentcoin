//! Executing a unit (spec `market/public-worker`): its summary, its full result and the result's
//! BLAKE3 hash, as committed and revealed on chain.

use anyhow::{Result, bail, ensure};
use serde::{Deserialize, Serialize};

use ac_primitives::market::public::JobId;

use crate::clean;
use crate::engine::EngineClient;
use crate::fingerprint;
use crate::manifest::{EvalItem, blake3};

/// A unit's output.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Output {
    /// The summary committed to.
    pub summary: Vec<u8>,
    /// The full result, uploaded to the publisher.
    pub result: Vec<u8>,
    /// BLAKE3 of the full result.
    pub result_hash: [u8; 32],
}

impl Output {
    fn new(summary: Vec<u8>, result: Vec<u8>) -> Self {
        let result_hash = blake3(&result);
        Self {
            summary,
            result,
            result_hash,
        }
    }
}

/// One evaluation item's full result.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvalResult {
    /// Each choice's log likelihood, in thousandths of a nat.
    pub loglik_millinats: Vec<i64>,
    /// The chosen index.
    pub choice: u8,
}

/// A log likelihood in thousandths of a nat, rounded to the nearest.
///
/// # Errors
///
/// A value that is not finite or does not fit.
pub fn millinats(nats: f64) -> Result<i64> {
    let m = (nats * 1000.0).round();
    ensure!(
        m.is_finite() && m.abs() < 9.0e15,
        "log likelihood out of range"
    );
    // Rounded and range-checked above, so the conversion is exact.
    Ok(m as i64)
}

/// The chosen index: the largest log likelihood, the lower index among equals.
#[must_use]
pub fn choose(logliks: &[i64]) -> u8 {
    let mut best = 0usize;
    for (i, l) in logliks.iter().enumerate() {
        if logliks.get(best).is_some_and(|b| l > b) {
            best = i;
        }
    }
    u8::try_from(best).unwrap_or(0)
}

/// Runs an evaluation unit (spec "评测单元的执行"): each choice's continuation is scored by its
/// log likelihood after the context.
///
/// # Errors
///
/// Engine errors, or a context whose tokens are not a prefix of context + continuation (the
/// item cannot be scored, so the unit is not committed).
pub async fn eval_unit(engine: &EngineClient, model: &str, items: &[EvalItem]) -> Result<Output> {
    let mut results = Vec::with_capacity(items.len());
    for item in items {
        let ctx = engine.tokenize(model, &item.context).await?;
        let mut logliks = Vec::with_capacity(item.choices.len());
        for cont in &item.choices {
            let full = engine
                .tokenize(model, &format!("{}{cont}", item.context))
                .await?;
            if full.len() <= ctx.len() || full.get(..ctx.len()) != Some(&ctx[..]) {
                bail!("a continuation does not extend the context's tokens");
            }
            let lps = engine.prompt_logprobs(model, &full).await?;
            let mut sum = 0.0f64;
            for lp in lps.get(ctx.len()..).unwrap_or_default() {
                match lp {
                    Some(x) => sum += x,
                    None => bail!("a continuation token without a log probability"),
                }
            }
            logliks.push(millinats(sum)?);
        }
        results.push(EvalResult {
            choice: choose(&logliks),
            loglik_millinats: logliks,
        });
    }
    let summary = results.iter().map(|r| r.choice).collect();
    Ok(Output::new(summary, serde_json::to_vec(&results)?))
}

/// The full result of an embedding unit.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EmbedResult {
    /// One normalized vector per text.
    pub embeddings: Vec<Vec<f32>>,
}

/// Runs an embedding unit (spec "嵌入单元的执行").
///
/// # Errors
///
/// Engine errors.
pub async fn embed_unit(
    engine: &EngineClient,
    model: &str,
    job: JobId,
    texts: &[String],
) -> Result<Output> {
    let raw = engine.embeddings(model, texts).await?;
    let dims = raw.first().map_or(0, Vec::len);
    let dirs = fingerprint::directions(job, dims);
    let summary = fingerprint::summary(&dirs, &raw);
    let embeddings = raw.iter().map(|v| normalized(v)).collect();
    Ok(Output::new(
        summary,
        serde_json::to_vec(&EmbedResult { embeddings })?,
    ))
}

fn normalized(v: &[f32]) -> Vec<f32> {
    let norm = v
        .iter()
        .map(|x| f64::from(*x) * f64::from(*x))
        .sum::<f64>()
        .sqrt();
    if norm == 0.0 {
        return v.to_vec();
    }
    // Back to the wire's f32 after dividing in f64.
    v.iter().map(|x| (f64::from(*x) / norm) as f32).collect()
}

/// Runs a data cleaning unit (spec "数据清洗单元的执行"): the summary is the result's hash.
///
/// # Errors
///
/// Serialization failure (none in practice).
pub fn clean_unit(docs: &[String]) -> Result<Output> {
    let result = clean::result_bytes(&clean::clean(docs))?;
    let hash = blake3(&result);
    Ok(Output::new(hash.to_vec(), result))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)] // Test code.

    use super::*;

    #[test]
    fn choices_prefer_the_largest_and_the_lower_index_among_equals() {
        assert_eq!(choose(&[-5, -3, -3, -9]), 1);
        assert_eq!(choose(&[-1]), 0);
        assert_eq!(choose(&[-2, -2]), 0);
    }

    #[test]
    fn millinats_round_to_the_nearest() {
        assert_eq!(millinats(-1.23456).unwrap(), -1235);
        assert_eq!(millinats(0.0004).unwrap(), 0);
        assert!(millinats(f64::NAN).is_err());
        assert!(millinats(f64::NEG_INFINITY).is_err());
    }

    #[test]
    fn clean_units_summarize_their_result() {
        let docs = vec!["A document long enough to be kept by the cleaning rules.".to_string()];
        let out = clean_unit(&docs).unwrap();
        assert_eq!(out.summary, out.result_hash.to_vec());
        assert_eq!(out, clean_unit(&docs).unwrap());
    }
}
