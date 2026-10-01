//! The re-check engine's HTTP interface (vLLM's `/tokenize`, `/detokenize` and
//! `/v1/completions`; verified against vLLM 0.30, m6-toploc-verify task 1.1).

use ac_wallet::http::{Client, join, read_body};
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

/// Largest response body read from the engine.
const LIMIT: usize = 16 << 20;

/// An OpenAI-compatible engine with vLLM's tokenizer endpoints.
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

    async fn post(&self, path: &str, body: &Value, headers: &[(&str, &str)]) -> Result<Value> {
        let resp = self
            .http
            .post_with(
                &join(&self.base, path),
                "application/json",
                headers,
                body.to_string(),
            )
            .await?;
        let status = resp.status().as_u16();
        let bytes = read_body(resp.into_body(), LIMIT).await?;
        // The body may echo the request: report only the status.
        if status != 200 {
            bail!("the engine answered {path} with status {status}");
        }
        serde_json::from_slice(&bytes).with_context(|| format!("the engine's {path} answer"))
    }

    fn ids(v: &Value, key: &str) -> Result<Vec<u32>> {
        v.get(key)
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

    /// The prompt tokens of `messages` under the model's chat template, with the generation
    /// prompt (as the chat endpoint builds them).
    ///
    /// # Errors
    ///
    /// Transport errors and unexpected answers.
    pub async fn chat_tokens(&self, model: &str, messages: &Value) -> Result<Vec<u32>> {
        let v = self
            .post(
                "/tokenize",
                &json!({ "model": model, "messages": messages, "add_generation_prompt": true }),
                &[],
            )
            .await?;
        Self::ids(&v, "tokens")
    }

    /// The tokens of a generated text (no special tokens added).
    ///
    /// # Errors
    ///
    /// Transport errors and unexpected answers.
    pub async fn text_tokens(&self, model: &str, text: &str) -> Result<Vec<u32>> {
        let v = self
            .post(
                "/tokenize",
                &json!({ "model": model, "prompt": text, "add_special_tokens": false }),
                &[],
            )
            .await?;
        Self::ids(&v, "tokens")
    }

    /// The text of `tokens`.
    ///
    /// # Errors
    ///
    /// Transport errors and unexpected answers.
    pub async fn detokenize(&self, model: &str, tokens: &[u32]) -> Result<String> {
        let v = self
            .post(
                "/detokenize",
                &json!({ "model": model, "tokens": tokens }),
                &[],
            )
            .await?;
        v.get("prompt")
            .and_then(Value::as_str)
            .map(str::to_string)
            .context("no text")
    }

    /// Prefills `tokens` (generating one token) under the request ID `request_id` (64 hex
    /// digits, sent as `X-Request-Id`); returns the prompt token count the engine reports.
    ///
    /// # Errors
    ///
    /// Transport errors and unexpected answers.
    pub async fn prefill(&self, model: &str, tokens: &[u32], request_id: &str) -> Result<u64> {
        let v = self
            .post(
                "/v1/completions",
                &json!({ "model": model, "prompt": tokens, "max_tokens": 1, "temperature": 0 }),
                &[("x-request-id", request_id)],
            )
            .await?;
        v.pointer("/usage/prompt_tokens")
            .and_then(Value::as_u64)
            .context("no usage")
    }
}
