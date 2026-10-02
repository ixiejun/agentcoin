//! The worker's inference engine interface: vLLM's `/tokenize`, `/v1/completions` with
//! `prompt_logprobs` and `/v1/embeddings` (probed on vLLM 0.30, m6-public-jobs task 6.1).

use ac_wallet::http::{Client, join, read_body};
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

/// Largest response body read from the engine.
const LIMIT: usize = 64 << 20;

/// An OpenAI-compatible engine with vLLM's tokenizer endpoint.
pub struct EngineClient {
    http: Client,
    base: String,
}

impl EngineClient {
    /// A client of the engine at `base` (e.g. `http://127.0.0.1:8000`).
    ///
    /// # Errors
    ///
    /// If the HTTP client cannot be built.
    pub fn new(base: &str) -> Result<Self> {
        Ok(Self {
            http: Client::new()?,
            base: base.trim_end_matches('/').to_string(),
        })
    }

    async fn post(&self, path: &str, body: &Value) -> Result<Value> {
        let resp = self
            .http
            .post(&join(&self.base, path), "application/json", body.to_string())
            .await?;
        let status = resp.status().as_u16();
        let bytes = read_body(resp.into_body(), LIMIT).await?;
        // The body may echo the request: report only the status.
        if status != 200 {
            bail!("the engine answered {path} with status {status}");
        }
        serde_json::from_slice(&bytes).with_context(|| format!("the engine's {path} answer"))
    }

    /// The tokens of `text` as a completion prompt.
    ///
    /// # Errors
    ///
    /// Transport errors and unexpected answers.
    pub async fn tokenize(&self, model: &str, text: &str) -> Result<Vec<u32>> {
        let v = self
            .post("/tokenize", &json!({ "model": model, "prompt": text }))
            .await?;
        v.get("tokens")
            .and_then(Value::as_array)
            .context("no token list")?
            .iter()
            .map(|t| {
                t.as_u64()
                    .and_then(|t| u32::try_from(t).ok())
                    .context("bad token")
            })
            .collect()
    }

    /// The log probability of each prompt token given those before it (`None` for the first).
    ///
    /// # Errors
    ///
    /// Transport errors and answers without one entry per prompt token.
    pub async fn prompt_logprobs(&self, model: &str, ids: &[u32]) -> Result<Vec<Option<f64>>> {
        let v = self
            .post(
                "/v1/completions",
                &json!({
                    "model": model, "prompt": ids, "max_tokens": 1, "temperature": 0,
                    "prompt_logprobs": 0,
                }),
            )
            .await?;
        let entries = v
            .pointer("/choices/0/prompt_logprobs")
            .and_then(Value::as_array)
            .context("no prompt_logprobs")?;
        if entries.len() != ids.len() {
            bail!("prompt_logprobs has {} entries for {} tokens", entries.len(), ids.len());
        }
        entries
            .iter()
            .zip(ids)
            .map(|(e, id)| {
                if e.is_null() {
                    return Ok(None);
                }
                e.get(id.to_string())
                    .and_then(|x| x.get("logprob"))
                    .and_then(Value::as_f64)
                    .map(Some)
                    .context("a prompt_logprobs entry without the prompt token")
            })
            .collect()
    }

    /// The embeddings of `texts`, in order.
    ///
    /// # Errors
    ///
    /// Transport errors, a count mismatch or vectors of different dimensions.
    pub async fn embeddings(&self, model: &str, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        let v = self
            .post("/v1/embeddings", &json!({ "model": model, "input": texts }))
            .await?;
        let data = v
            .get("data")
            .and_then(Value::as_array)
            .context("no embeddings")?;
        if data.len() != texts.len() {
            bail!("{} embeddings for {} texts", data.len(), texts.len());
        }
        let vecs: Vec<Vec<f32>> = data
            .iter()
            .map(|d| {
                d.get("embedding")
                    .and_then(Value::as_array)
                    .context("no embedding")?
                    .iter()
                    .map(|x| {
                        x.as_f64()
                            // Embeddings are f32 on the wire; narrowing back is exact.
                            .map(|x| x as f32)
                            .context("bad embedding value")
                    })
                    .collect()
            })
            .collect::<Result<_>>()?;
        let dims = vecs.first().map_or(0, Vec::len);
        if dims == 0 || vecs.iter().any(|v| v.len() != dims) {
            bail!("embeddings of different dimensions");
        }
        Ok(vecs)
    }
}
