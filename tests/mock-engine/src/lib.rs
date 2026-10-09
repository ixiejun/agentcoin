//! A deterministic OpenAI-compatible inference engine for tests (m5-gateway-provider design D9).
//!
//! The output echoes the words of the last message, then continues with a fixed word list, one
//! word per token, up to the request's output limit (default 16). Prompt tokens are the
//! whitespace-separated words of every message. Time to first token and the interval between
//! tokens are configurable, and a failure switch drops the stream after a number of tokens.
//! With [`Config::toploc`] the engine also plays the vLLM TOPLOC plugin ([`plugin`]).
//!
//! For auditors' re-checks (m6-toploc-verify design D6) it also has a toy tokenizer, one token
//! per word ([`token_id`]), behind vLLM's `/tokenize` and `/detokenize`, and `/v1/completions`
//! for a prompt of token IDs: the chat template is the messages' words in order, and an output
//! token renders as its word and a space.
//!
//! For public job workers (m6-public-jobs design D12) `/v1/completions` also answers
//! `prompt_logprobs` (vLLM's format, values from [`prompt_logprob`]) and `/v1/embeddings` returns
//! [`EMBEDDING_DIMS`]-dimensional vectors from [`embedding`]; both depend on the model seed, so
//! another seed plays another model.

pub mod plugin;

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ac_market_proto::engine::EngineMode;
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
/// Dimension of the pseudo-model's embeddings.
pub const EMBEDDING_DIMS: usize = 64;

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
    /// Send pseudo-activations to a TOPLOC socket, as the vLLM plugin does.
    pub toploc: Option<PluginConfig>,
    /// Seed of the pseudo-model's activations (another seed plays another model).
    pub model_seed: u64,
    /// From the moment this file exists, play the model of this seed instead: a provider that
    /// starts cheating while running (m6-auditor-agent 7.1).
    pub switch: Option<(u64, std::path::PathBuf)>,
    /// From the moment this file exists, prove slightly deviating activations: a provider that
    /// starts serving 8-bit weights (m6-audit-sprt 7.1).
    pub deviate: Option<std::path::PathBuf>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            models: vec!["mock-model".into()],
            ttft: Duration::from_millis(20),
            token_interval: Duration::from_millis(5),
            fail_after: None,
            toploc: None,
            model_seed: 0,
            switch: None,
            deviate: None,
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

/// The token ID of a word in the toy tokenizer: in `[1, 50,000)`.
#[must_use]
pub fn token_id(word: &str) -> u32 {
    let h = word.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    });
    u32::try_from(h % 49_999).unwrap_or(0).saturating_add(1)
}

/// SplitMix64's finalizer: a fixed, well-spread hash step (not a security primitive).
fn mix(mut z: u64) -> u64 {
    z = z.wrapping_add(0x9e37_79b9_7f4a_7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

/// The log probability the pseudo-model of `seed` gives `token` after `prefix`: in `(-8, 0]`,
/// from a rolling hash of the seed and the prefix tokens.
#[must_use]
pub fn prompt_logprob(seed: u64, prefix: &[u32], token: u32) -> f64 {
    let state = prefix.iter().fold(mix(seed), |h, t| mix(h ^ u64::from(*t)));
    let [a, b, c, d, ..] = mix(state ^ u64::from(token).rotate_left(32)).to_le_bytes();
    -f64::from(u32::from_le_bytes([a, b, c, d])) / f64::from(u32::MAX) * 8.0
}

/// The pseudo-model's embedding of `text` under `seed`: [`EMBEDDING_DIMS`] values in `[-1, 1)`.
#[must_use]
pub fn embedding(seed: u64, text: &str) -> Vec<f32> {
    let h = text.bytes().fold(mix(!seed), |h, b| mix(h ^ u64::from(b)));
    (0..EMBEDDING_DIMS)
        .map(|i| {
            let [a, b, ..] = mix(h ^ u64::try_from(i).unwrap_or(0)).to_le_bytes();
            f32::from(i16::from_le_bytes([a, b])) / 32_768.0
        })
        .collect()
}

struct Shared {
    config: Config,
    served: Arc<AtomicU64>,
    plugin: Option<plugin::Plugin>,
    /// Words seen, for `/detokenize`.
    vocab: Mutex<BTreeMap<u32, String>>,
}

impl Shared {
    fn ids(&self, words: &[String]) -> Vec<u32> {
        let mut vocab = self
            .vocab
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        words
            .iter()
            .map(|w| {
                let id = token_id(w);
                vocab.entry(id).or_insert_with(|| w.clone());
                id
            })
            .collect()
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
    let shared = Arc::new(Shared {
        plugin: config.toploc.clone().map(|c| {
            plugin::Plugin::new(c, config.model_seed)
                .switching(config.switch.clone())
                .deviating(config.deviate.clone())
        }),
        config,
        served: Arc::clone(&served),
        vocab: Mutex::new(BTreeMap::new()),
    });
    if shared
        .plugin
        .as_ref()
        .is_some_and(|p| p.mode() == ac_market_proto::engine::EngineMode::Verify)
    {
        let eager = Arc::clone(&shared);
        tokio::spawn(async move {
            // Up to a minute for the receiver to appear.
            for _ in 0..300 {
                if let Some(p) = &eager.plugin
                    && p.connect_now().await
                {
                    return;
                }
                tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            }
        });
    }
    tokio::spawn(http::serve(listener, move |req| {
        let shared = Arc::clone(&shared);
        async move { handle(req, shared).await }
    }));
    Ok(Engine { addr, served })
}

/// The engine's ID for a request: vLLM's `<prefix>-<X-Request-Id>-<suffix>`.
fn engine_id(req: &Request, prefix: &str) -> String {
    req.headers()
        .get("x-request-id")
        .and_then(|v| v.to_str().ok())
        .map_or_else(|| format!("{prefix}-mock"), |x| format!("{prefix}-{x}-0"))
}

async fn body_json(req: Request, shared: &Shared) -> Result<Value, Response> {
    let Ok(body) = http::read_body(req.into_body(), 4 << 20).await else {
        return Err(error(400, "unreadable body"));
    };
    let Ok(request) = serde_json::from_slice::<Value>(&body) else {
        return Err(error(400, "invalid JSON"));
    };
    let model = request
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if !shared.config.models.iter().any(|m| m == model) {
        return Err(error(404, "unknown model"));
    }
    Ok(request)
}

async fn handle(req: Request, shared: Arc<Shared>) -> Response {
    match (req.method().as_str(), req.uri().path()) {
        ("GET", "/v1/models") => {
            let data: Vec<Value> = shared
                .config
                .models
                .iter()
                .map(|m| json!({ "id": m, "object": "model", "owned_by": "mock" }))
                .collect();
            http::json(200, json!({ "object": "list", "data": data }).to_string())
        }
        ("GET", "/mock/requests") => {
            http::json(200, shared.served.load(Ordering::SeqCst).to_string())
        }
        ("POST", "/v1/chat/completions") => {
            shared.served.fetch_add(1, Ordering::SeqCst);
            let engine_id = engine_id(&req, "chatcmpl");
            let request = match body_json(req, &shared).await {
                Ok(r) => r,
                Err(e) => return e,
            };
            let model = request
                .get("model")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            complete(&request, model, Arc::clone(&shared), engine_id)
        }
        ("POST", "/tokenize") => {
            let request = match body_json(req, &shared).await {
                Ok(r) => r,
                Err(e) => return e,
            };
            let words = match request.get("messages").and_then(Value::as_array) {
                Some(messages) => messages.iter().flat_map(words).collect(),
                None => request
                    .get("prompt")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .split_whitespace()
                    .map(str::to_string)
                    .collect::<Vec<_>>(),
            };
            let ids = shared.ids(&words);
            http::json(
                200,
                json!({ "count": ids.len(), "max_model_len": 4096, "tokens": ids }).to_string(),
            )
        }
        ("POST", "/detokenize") => {
            let request = match body_json(req, &shared).await {
                Ok(r) => r,
                Err(e) => return e,
            };
            let vocab = shared
                .vocab
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let mut text = String::new();
            for id in request
                .get("tokens")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let Some(word) = id
                    .as_u64()
                    .and_then(|i| u32::try_from(i).ok())
                    .and_then(|i| vocab.get(&i))
                else {
                    return error(400, "unknown token");
                };
                text.push_str(word);
                text.push(' ');
            }
            http::json(200, json!({ "prompt": text }).to_string())
        }
        ("POST", "/v1/completions") => {
            shared.served.fetch_add(1, Ordering::SeqCst);
            let engine_id = engine_id(&req, "cmpl");
            let request = match body_json(req, &shared).await {
                Ok(r) => r,
                Err(e) => return e,
            };
            let ids: Vec<u32> = match request.get("prompt") {
                Some(Value::Array(a)) => a
                    .iter()
                    .filter_map(|v| v.as_u64().and_then(|i| u32::try_from(i).ok()))
                    .collect(),
                Some(Value::String(s)) => {
                    shared.ids(&s.split_whitespace().map(str::to_string).collect::<Vec<_>>())
                }
                _ => return error(400, "no prompt"),
            };
            if let Some(p) = shared
                .plugin
                .as_ref()
                .filter(|p| p.mode() == EngineMode::Verify)
            {
                p.send_rows(&engine_id, &ids).await;
            }
            let model = request
                .get("model")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let mut choice =
                json!({ "index": 0, "text": format!("{} ", WORDS[0]), "finish_reason": "length" });
            if request.get("prompt_logprobs").is_some_and(|v| !v.is_null()) {
                choice["prompt_logprobs"] = prompt_logprobs(&shared, &ids);
            }
            let body = json!({
                "id": engine_id, "object": "text_completion", "created": 0, "model": model,
                "choices": [choice],
                "usage": { "prompt_tokens": ids.len(), "completion_tokens": 1, "total_tokens": ids.len().saturating_add(1) },
            });
            http::json(200, body.to_string())
        }
        ("POST", "/v1/embeddings") => {
            let request = match body_json(req, &shared).await {
                Ok(r) => r,
                Err(e) => return e,
            };
            let texts: Vec<String> = match request.get("input") {
                Some(Value::String(s)) => vec![s.clone()],
                Some(Value::Array(a)) => a
                    .iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect(),
                _ => return error(400, "no input"),
            };
            let data: Vec<Value> = texts
                .iter()
                .enumerate()
                .map(|(i, t)| {
                    json!({ "object": "embedding", "index": i,
                            "embedding": embedding(shared.config.model_seed, t) })
                })
                .collect();
            let tokens: usize = texts.iter().map(|t| t.split_whitespace().count()).sum();
            let model = request
                .get("model")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let body = json!({
                "object": "list", "model": model, "data": data,
                "usage": { "prompt_tokens": tokens, "total_tokens": tokens },
            });
            http::json(200, body.to_string())
        }
        _ => error(404, "not found"),
    }
}

/// vLLM's `prompt_logprobs`: `null` for the first token, then the prompt token's entry.
fn prompt_logprobs(shared: &Shared, ids: &[u32]) -> Value {
    let vocab = shared
        .vocab
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut out = vec![Value::Null];
    for (i, id) in ids.iter().enumerate().skip(1) {
        let lp = prompt_logprob(
            shared.config.model_seed,
            ids.get(..i).unwrap_or_default(),
            *id,
        );
        let word = vocab.get(id).cloned().unwrap_or_default();
        out.push(json!({ id.to_string(): { "logprob": lp, "rank": 1, "decoded_token": word } }));
    }
    Value::Array(out)
}

fn words(m: &Value) -> Vec<String> {
    m.get("content")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .split_whitespace()
        .map(str::to_string)
        .collect()
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
    shared: Arc<Shared>,
    prompt: Vec<u32>,
    output: Vec<u32>,
}

impl Answer {
    async fn finished(&self) {
        if let Some(p) = self
            .shared
            .plugin
            .as_ref()
            .filter(|p| p.mode() == EngineMode::Prove)
        {
            p.send(&self.engine_id, &self.prompt, &self.output).await;
        }
    }
}

fn complete(request: &Value, model: String, shared: Arc<Shared>, engine_id: String) -> Response {
    let (tokens, prompt) = answer(request);
    let messages = request
        .get("messages")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let prompt_words: Vec<String> = messages.iter().flat_map(words).collect();
    let answer_to = Answer {
        engine_id,
        prompt: shared.ids(&prompt_words),
        output: shared.ids(&tokens),
        shared: Arc::clone(&shared),
    };
    let config = &shared.config;
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
            answer_to.finished().await;
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
        answer_to.finished().await;
        if include_usage {
            send(json!({ "id": id, "object": "chat.completion.chunk", "created": 0, "model": model, "choices": [], "usage": usage })).await;
        }
        tx.send("data: [DONE]\n\n").await;
    });
    resp
}
