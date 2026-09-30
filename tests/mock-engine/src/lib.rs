//! A deterministic OpenAI-compatible inference engine for tests (m5-gateway-provider design D9).
//!
//! The output echoes the words of the last message, then continues with a fixed word list, one
//! word per token, up to the request's output limit (default 16). Prompt tokens are the
//! whitespace-separated words of every message. Time to first token and the interval between
//! tokens are configurable, and a failure switch drops the stream after a number of tokens.
//! With [`Config::toploc`] the engine also plays the vLLM TOPLOC plugin ([`plugin`]).

pub mod plugin;

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use ac_wallet::http::{self, Request, Response};
pub use plugin::PluginConfig;
use serde_json::{Value, json};
use tokio::net::TcpListener;

const WORDS: [&str; 8] = [
    "lorem",
    "ipsum",
    "dolor",
    "sit",
    "amet",
    "consectetur",
    "adipiscing",
    "elit",
];
const DEFAULT_MAX_TOKENS: u64 = 16;

/// Engine behaviour.
#[derive(Clone, Debug)]
pub struct Config {
    /// Model names served (`/v1/models`); requests may name any of them.
    pub models: Vec<String>,
    /// Delay before the first token.
    pub ttft: Duration,
    /// Delay between tokens.
    pub token_interval: Duration,
    /// Drop the stream after this many tokens (no usage, no `[DONE]`).
    pub fail_after: Option<u64>,
    /// Send pseudo-activations to a provider's TOPLOC socket, as the vLLM plugin does.
    pub toploc: Option<PluginConfig>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            models: vec!["mock-model".into()],
            ttft: Duration::from_millis(20),
            token_interval: Duration::from_millis(5),
            fail_after: None,
            toploc: None,
        }
    }
}

/// A running engine.
#[derive(Clone, Debug)]
pub struct Engine {
    /// Where it listens.
    pub addr: SocketAddr,
    served: Arc<AtomicU64>,
}

impl Engine {
    /// Base URL, e.g. `http://127.0.0.1:1234`.
    #[must_use]
    pub fn url(&self) -> String {
        format!("http://{}", self.addr)
    }

    /// Number of chat completion requests received so far.
    #[must_use]
    pub fn requests(&self) -> u64 {
        self.served.load(Ordering::SeqCst)
    }
}

/// Binds `listen` and serves in the background.
///
/// # Errors
///
/// If the address cannot be bound.
pub async fn spawn(listen: &str, config: Config) -> anyhow::Result<Engine> {
    let listener = TcpListener::bind(listen).await?;
    let addr = listener.local_addr()?;
    let served = Arc::new(AtomicU64::new(0));
    let plugin = config
        .toploc
        .clone()
        .map(|c| Arc::new(plugin::Plugin::new(c)));
    let (config, counter) = (Arc::new(config), Arc::clone(&served));
    tokio::spawn(http::serve(listener, move |req| {
        let (config, counter, plugin) = (Arc::clone(&config), Arc::clone(&counter), plugin.clone());
        async move { handle(req, &config, &counter, plugin).await }
    }));
    Ok(Engine { addr, served })
}

async fn handle(
    req: Request,
    config: &Config,
    served: &AtomicU64,
    plugin: Option<Arc<plugin::Plugin>>,
) -> Response {
    match (req.method().as_str(), req.uri().path()) {
        ("GET", "/v1/models") => {
            let data: Vec<Value> = config
                .models
                .iter()
                .map(|m| json!({ "id": m, "object": "model", "owned_by": "mock" }))
                .collect();
            http::json(200, json!({ "object": "list", "data": data }).to_string())
        }
        ("GET", "/mock/requests") => http::json(200, served.load(Ordering::SeqCst).to_string()),
        ("POST", "/v1/chat/completions") => {
            served.fetch_add(1, Ordering::SeqCst);
            // vLLM names a request `chatcmpl-<X-Request-Id>-<suffix>`.
            let engine_id = req
                .headers()
                .get("x-request-id")
                .and_then(|v| v.to_str().ok())
                .map_or_else(
                    || "chatcmpl-mock".to_string(),
                    |x| format!("chatcmpl-{x}-6d6f636b"),
                );
            let Ok(body) = http::read_body(req.into_body(), 4 << 20).await else {
                return error(400, "unreadable body");
            };
            let Ok(request) = serde_json::from_slice::<Value>(&body) else {
                return error(400, "invalid JSON");
            };
            let model = request
                .get("model")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            if !config.models.contains(&model) {
                return error(404, "unknown model");
            }
            complete(&request, model, config, Answer { engine_id, plugin })
        }
        _ => error(404, "not found"),
    }
}

fn error(status: u16, message: &str) -> Response {
    http::json(
        status,
        json!({ "error": { "message": message, "type": "mock_error" } }).to_string(),
    )
}

/// The tokens of a request's answer and its prompt token count.
#[must_use]
pub fn answer(request: &Value) -> (Vec<String>, u64) {
    let messages = request
        .get("messages")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let words = |m: &Value| -> Vec<String> {
        m.get("content")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .split_whitespace()
            .map(str::to_string)
            .collect()
    };
    let prompt = messages
        .iter()
        .map(|m| u64::try_from(words(m).len()).unwrap_or(u64::MAX))
        .fold(0u64, u64::saturating_add);
    let limit = ["max_completion_tokens", "max_tokens"]
        .iter()
        .find_map(|k| request.get(*k).and_then(Value::as_u64))
        .unwrap_or(DEFAULT_MAX_TOKENS);
    let echo = messages.last().map(words).unwrap_or_default();
    let tokens = echo
        .into_iter()
        .chain(WORDS.iter().cycle().map(|w| (*w).to_string()))
        .take(usize::try_from(limit).unwrap_or(usize::MAX))
        .collect();
    (tokens, prompt)
}

/// Who hears about an answer once it is complete.
struct Answer {
    engine_id: String,
    plugin: Option<Arc<plugin::Plugin>>,
}

impl Answer {
    async fn finished(&self, prompt: u64, completion: u64) {
        if let Some(p) = &self.plugin {
            p.send(&self.engine_id, prompt, completion).await;
        }
    }
}

fn complete(request: &Value, model: String, config: &Config, answer_to: Answer) -> Response {
    let (tokens, prompt) = answer(request);
    let completion = u64::try_from(tokens.len()).unwrap_or(u64::MAX);
    let usage = json!({ "prompt_tokens": prompt, "completion_tokens": completion, "total_tokens": prompt.saturating_add(completion) });
    let stream = request
        .get("stream")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let include_usage = request
        .pointer("/stream_options/include_usage")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let id = "chatcmpl-mock";
    if !stream {
        let text: Vec<String> = tokens.iter().map(|t| format!("{t} ")).collect();
        let (config, model) = (config.clone(), model);
        let (tx, resp) = http::streaming(200, "application/json");
        tokio::spawn(async move {
            tokio::time::sleep(config.ttft).await;
            answer_to.finished(prompt, completion).await;
            let body = json!({
                "id": id, "object": "chat.completion", "created": 0, "model": model,
                "choices": [{ "index": 0, "message": { "role": "assistant", "content": text.concat() }, "finish_reason": "length" }],
                "usage": usage,
            });
            tx.send(body.to_string()).await;
        });
        return resp;
    }
    let (tx, resp) = http::streaming(200, "text/event-stream");
    let config = config.clone();
    tokio::spawn(async move {
        let chunk = |delta: Value, finish: Value| {
            json!({ "id": id, "object": "chat.completion.chunk", "created": 0, "model": model,
                    "choices": [{ "index": 0, "delta": delta, "finish_reason": finish }] })
        };
        let send = |v: Value| {
            let tx = tx.clone();
            async move { tx.send(format!("data: {v}\n\n")).await }
        };
        tokio::time::sleep(config.ttft).await;
        if !send(chunk(
            json!({ "role": "assistant", "content": "" }),
            Value::Null,
        ))
        .await
        {
            return;
        }
        for (i, t) in tokens.iter().enumerate() {
            if config
                .fail_after
                .is_some_and(|n| u64::try_from(i).unwrap_or(u64::MAX) >= n)
            {
                return; // Dropping the sender ends the body without usage or [DONE].
            }
            if i > 0 {
                tokio::time::sleep(config.token_interval).await;
            }
            if !send(chunk(json!({ "content": format!("{t} ") }), Value::Null)).await {
                return;
            }
        }
        send(chunk(json!({}), json!("length"))).await;
        answer_to.finished(prompt, completion).await;
        if include_usage {
            send(json!({ "id": id, "object": "chat.completion.chunk", "created": 0, "model": model, "choices": [], "usage": usage })).await;
        }
        tx.send("data: [DONE]\n\n").await;
    });
    resp
}
