//! The subset of the OpenAI Chat Completions API the market carries.
//!
//! Requests stay JSON objects: only `model`, `stream`, `stream_options.include_usage` and the
//! output-token limit are ever rewritten, everything else passes through untouched. Parse
//! errors never quote the input, so request content cannot leak into logs or error messages.

use std::collections::BTreeMap;

use serde_json::{Map, Value, json};

use crate::Error;
use crate::msg::{ErrorCode, Usage};

/// Template allowance per message when bounding prompt tokens (role markers, separators).
pub const TOKENS_PER_MESSAGE: u64 = 16;

/// A Chat Completions request.
#[derive(Clone, PartialEq, Eq)]
pub struct ChatRequest(Map<String, Value>);

// Request content must never reach logs: Debug shows only the shape.
impl core::fmt::Debug for ChatRequest {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("ChatRequest")
            .field("model", &self.model())
            .field("stream", &self.stream())
            .finish_non_exhaustive()
    }
}

impl ChatRequest {
    /// Parses and checks the minimal shape: an object with a string `model` and a non-empty
    /// `messages` array of objects.
    ///
    /// # Errors
    ///
    /// [`Error::BadJson`] or [`Error::MissingField`]; neither quotes the input.
    pub fn parse(bytes: &[u8]) -> Result<Self, Error> {
        let value: Value = serde_json::from_slice(bytes).map_err(|_| Error::BadJson)?;
        let Value::Object(map) = value else {
            return Err(Error::BadJson);
        };
        if !map.get("model").is_some_and(Value::is_string) {
            return Err(Error::MissingField("model"));
        }
        let ok_messages = map
            .get("messages")
            .and_then(Value::as_array)
            .is_some_and(|m| !m.is_empty() && m.iter().all(Value::is_object));
        if !ok_messages {
            return Err(Error::MissingField("messages"));
        }
        for key in ["max_tokens", "max_completion_tokens"] {
            if map
                .get(key)
                .is_some_and(|v| !v.is_null() && as_u32(v).is_none())
            {
                return Err(Error::MissingField(key));
            }
        }
        Ok(Self(map))
    }

    /// The requested model name or ID.
    #[must_use]
    pub fn model(&self) -> &str {
        self.0
            .get("model")
            .and_then(Value::as_str)
            .unwrap_or_default()
    }

    /// Whether a streamed response was asked for.
    #[must_use]
    pub fn stream(&self) -> bool {
        self.0
            .get("stream")
            .and_then(Value::as_bool)
            .unwrap_or(false)
    }

    /// The output-token limit: `max_completion_tokens` takes precedence over `max_tokens`.
    #[must_use]
    pub fn max_output_tokens(&self) -> Option<u32> {
        ["max_completion_tokens", "max_tokens"]
            .iter()
            .find_map(|k| self.0.get(*k).and_then(as_u32))
    }

    /// Number of choices asked for (`n`, default 1).
    #[must_use]
    pub fn choices(&self) -> u32 {
        self.0.get("n").and_then(as_u32).unwrap_or(1)
    }

    /// Upper bound on prompt tokens: the UTF-8 bytes of every message's text content plus
    /// [`TOKENS_PER_MESSAGE`] per message (a token never encodes less than one byte of text).
    #[must_use]
    pub fn input_bound(&self) -> u64 {
        let messages = self.0.get("messages").and_then(Value::as_array);
        messages.map_or(0, |ms| {
            ms.iter().fold(0u64, |acc, m| {
                let text = m.get("content").map_or(0, text_bytes);
                let name = m.get("name").and_then(Value::as_str).map_or(0, str::len);
                acc.saturating_add(u64::try_from(text.saturating_add(name)).unwrap_or(u64::MAX))
                    .saturating_add(TOKENS_PER_MESSAGE)
            })
        })
    }

    /// Replaces the model.
    pub fn set_model(&mut self, model: &str) {
        self.0.insert("model".into(), Value::String(model.into()));
    }

    /// Sets `stream` and, when streaming, asks for usage in the last chunk.
    pub fn set_stream(&mut self, stream: bool) {
        self.0.insert("stream".into(), Value::Bool(stream));
        if stream {
            let options = self
                .0
                .entry("stream_options")
                .or_insert_with(|| Value::Object(Map::new()));
            if let Value::Object(o) = options {
                o.insert("include_usage".into(), Value::Bool(true));
            } else {
                *options = json!({ "include_usage": true });
            }
        } else {
            self.0.remove("stream_options");
        }
    }

    /// Sets the output-token limit (both spellings, so every engine honours it).
    pub fn set_max_output_tokens(&mut self, limit: u32) {
        self.0.insert("max_tokens".into(), Value::from(limit));
        if self.0.contains_key("max_completion_tokens") {
            self.0
                .insert("max_completion_tokens".into(), Value::from(limit));
        }
    }

    /// Serializes back to JSON.
    #[must_use]
    pub fn to_bytes(&self) -> Vec<u8> {
        serde_json::to_vec(&self.0).unwrap_or_default()
    }
}

fn as_u32(v: &Value) -> Option<u32> {
    v.as_u64().and_then(|n| u32::try_from(n).ok())
}

/// Bytes of text in a message `content`: a string, or an array of parts whose `text` fields
/// count (other parts, e.g. images, count their serialized size).
fn text_bytes(content: &Value) -> usize {
    match content {
        Value::String(s) => s.len(),
        Value::Array(parts) => parts.iter().fold(0usize, |acc, p| {
            let n = p
                .get("text")
                .and_then(Value::as_str)
                .map_or_else(|| serde_json::to_vec(p).map_or(0, |v| v.len()), str::len);
            acc.saturating_add(n)
        }),
        Value::Null => 0,
        other => serde_json::to_vec(other).map_or(0, |v| v.len()),
    }
}

/// One server-sent event of a streamed response.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SseEvent {
    /// A `data:` payload (a JSON chunk).
    Data(Vec<u8>),
    /// The `data: [DONE]` terminator.
    Done,
}

/// Incremental server-sent-events parser for `data:` lines.
#[derive(Debug, Default)]
pub struct SseDecoder {
    buf: Vec<u8>,
    data: Vec<u8>,
}

impl SseDecoder {
    /// Largest event accepted.
    pub const MAX_EVENT: usize = 4 * 1024 * 1024;

    /// An empty parser.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            buf: Vec::new(),
            data: Vec::new(),
        }
    }

    /// Feeds bytes and returns the events they complete.
    ///
    /// # Errors
    ///
    /// [`Error::FrameTooLarge`] for an event larger than [`Self::MAX_EVENT`].
    pub fn push(&mut self, bytes: &[u8]) -> Result<Vec<SseEvent>, Error> {
        self.buf.extend_from_slice(bytes);
        let mut events = Vec::new();
        while let Some(pos) = self.buf.iter().position(|b| *b == b'\n') {
            let mut line: Vec<u8> = self.buf.drain(..=pos).collect();
            line.pop();
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            if line.is_empty() {
                if !self.data.is_empty() {
                    let data = core::mem::take(&mut self.data);
                    events.push(if data == b"[DONE]" {
                        SseEvent::Done
                    } else {
                        SseEvent::Data(data)
                    });
                }
            } else if let Some(rest) = line.strip_prefix(b"data:") {
                let rest = rest.strip_prefix(b" ").unwrap_or(rest);
                if !self.data.is_empty() {
                    self.data.push(b'\n');
                }
                self.data.extend_from_slice(rest);
            }
            // Comments (`:`) and other fields (`event:`, `id:`) are ignored.
        }
        if self.buf.len().saturating_add(self.data.len()) > Self::MAX_EVENT {
            return Err(Error::FrameTooLarge);
        }
        Ok(events)
    }
}

/// Formats one SSE `data:` event.
#[must_use]
pub fn sse_data(json: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(json.len().saturating_add(8));
    out.extend_from_slice(b"data: ");
    out.extend_from_slice(json);
    out.extend_from_slice(b"\n\n");
    out
}

/// The SSE terminator.
pub const SSE_DONE: &[u8] = b"data: [DONE]\n\n";

/// Usage reported in a chunk or response, if present.
#[must_use]
pub fn usage_of(json: &[u8]) -> Option<Usage> {
    let v: Value = serde_json::from_slice(json).ok()?;
    let u = v.get("usage")?;
    Some(Usage {
        prompt_tokens: u.get("prompt_tokens").and_then(as_u32)?,
        completion_tokens: u.get("completion_tokens").and_then(as_u32)?,
    })
}

/// Whether a streamed chunk carries generated content (the first such chunk ends TTFT).
#[must_use]
pub fn has_content(json: &[u8]) -> bool {
    let Ok(v) = serde_json::from_slice::<Value>(json) else {
        return false;
    };
    v.get("choices")
        .and_then(Value::as_array)
        .is_some_and(|cs| {
            cs.iter().any(|c| {
                let d = c.get("delta");
                d.and_then(|d| d.get("content"))
                    .and_then(Value::as_str)
                    .is_some_and(|s| !s.is_empty())
                    || d.and_then(|d| d.get("tool_calls"))
                        .is_some_and(|t| !t.is_null())
            })
        })
}

/// Sets or replaces `usage` in a JSON object (the proxy reports the receipt's usage).
#[must_use]
pub fn with_usage(json: &[u8], usage: Usage) -> Vec<u8> {
    let Ok(Value::Object(mut map)) = serde_json::from_slice::<Value>(json) else {
        return json.to_vec();
    };
    map.insert("usage".into(), usage_json(usage));
    serde_json::to_vec(&map).unwrap_or_else(|_| json.to_vec())
}

/// `usage` object of a response.
#[must_use]
pub fn usage_json(usage: Usage) -> Value {
    json!({
        "prompt_tokens": usage.prompt_tokens,
        "completion_tokens": usage.completion_tokens,
        "total_tokens": u64::from(usage.prompt_tokens).saturating_add(u64::from(usage.completion_tokens)),
    })
}

/// An OpenAI-format error body.
#[must_use]
pub fn error_json(code: ErrorCode, message: &str) -> Vec<u8> {
    serde_json::to_vec(&json!({
        "error": { "message": message, "type": "agentcoin_error", "code": code.as_str() }
    }))
    .unwrap_or_default()
}

/// Folds streamed chunks into one non-streamed `chat.completion` response.
#[derive(Debug, Default)]
pub struct Assembler {
    id: Option<Value>,
    model: Option<Value>,
    created: Option<Value>,
    choices: BTreeMap<u64, (String, Value)>,
}

impl Assembler {
    /// An empty assembler.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds one streamed chunk; unparsable chunks are ignored.
    pub fn push(&mut self, json: &[u8]) {
        let Ok(v) = serde_json::from_slice::<Value>(json) else {
            return;
        };
        for (slot, key) in [
            (&mut self.id, "id"),
            (&mut self.model, "model"),
            (&mut self.created, "created"),
        ] {
            if slot.is_none() {
                *slot = v.get(key).cloned();
            }
        }
        for c in v
            .get("choices")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let index = c.get("index").and_then(Value::as_u64).unwrap_or(0);
            let entry = self
                .choices
                .entry(index)
                .or_insert_with(|| (String::new(), Value::Null));
            if let Some(s) = c
                .get("delta")
                .and_then(|d| d.get("content"))
                .and_then(Value::as_str)
            {
                entry.0.push_str(s);
            }
            if let Some(r) = c.get("finish_reason").filter(|r| !r.is_null()) {
                entry.1 = r.clone();
            }
        }
    }

    /// The assembled response with `usage`.
    #[must_use]
    pub fn finish(self, usage: Usage) -> Vec<u8> {
        let choices: Vec<Value> = self
            .choices
            .into_iter()
            .map(|(index, (content, finish))| {
                json!({
                    "index": index,
                    "message": { "role": "assistant", "content": content },
                    "finish_reason": finish,
                })
            })
            .collect();
        serde_json::to_vec(&json!({
            "id": self.id.unwrap_or(Value::Null),
            "object": "chat.completion",
            "created": self.created.unwrap_or(Value::Null),
            "model": self.model.unwrap_or(Value::Null),
            "choices": choices,
            "usage": usage_json(usage),
        }))
        .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // The request example of the OpenAI API reference ("Create chat completion").
    const EXAMPLE: &str = r#"{
      "model": "gpt-4o",
      "messages": [
        {"role": "developer", "content": "You are a helpful assistant."},
        {"role": "user", "content": "Hello!"}
      ],
      "temperature": 0.7,
      "logit_bias": {"50256": -100},
      "user": "abc"
    }"#;

    #[test]
    fn unknown_fields_pass_through_and_rewrites_are_limited() {
        let mut r = ChatRequest::parse(EXAMPLE.as_bytes()).unwrap();
        assert_eq!(r.model(), "gpt-4o");
        assert!(!r.stream());
        assert_eq!(r.max_output_tokens(), None);
        assert_eq!(r.input_bound(), 28 + 6 + 2 * TOKENS_PER_MESSAGE);
        r.set_model("0xabc");
        r.set_stream(true);
        r.set_max_output_tokens(64);
        let v: Value = serde_json::from_slice(&r.to_bytes()).unwrap();
        assert_eq!(v["model"], "0xabc");
        assert_eq!(v["stream"], true);
        assert_eq!(v["stream_options"]["include_usage"], true);
        assert_eq!(v["max_tokens"], 64);
        assert_eq!(v["temperature"], 0.7);
        assert_eq!(v["logit_bias"]["50256"], -100);
        assert_eq!(v["user"], "abc");
        assert_eq!(v["messages"][1]["content"], "Hello!");
        let r2 = ChatRequest::parse(&r.to_bytes()).unwrap();
        assert_eq!(r2, r);
    }

    #[test]
    fn output_limit_precedence_and_content_parts() {
        let r = ChatRequest::parse(
            br#"{"model":"m","max_tokens":10,"max_completion_tokens":20,
                "messages":[{"role":"user","content":[{"type":"text","text":"abc"},{"type":"text","text":"de"}]}]}"#,
        )
        .unwrap();
        assert_eq!(r.max_output_tokens(), Some(20));
        assert_eq!(r.input_bound(), 5 + TOKENS_PER_MESSAGE);
    }

    #[test]
    fn malformed_requests_are_rejected_without_quoting_them() {
        for (bad, err) in [
            (
                &br#"{"model":"m","messages":"secret-prompt"}"#[..],
                Error::MissingField("messages"),
            ),
            (
                br#"{"messages":[{"content":"secret-prompt"}]}"#,
                Error::MissingField("model"),
            ),
            (br#"["secret-prompt"]"#, Error::BadJson),
            (
                br#"{"model":"m","messages":[{"content":"secret-prompt"}],"max_tokens":-1}"#,
                Error::MissingField("max_tokens"),
            ),
            (br#"{"model": secret-prompt"#, Error::BadJson),
        ] {
            let e = ChatRequest::parse(bad).unwrap_err();
            assert_eq!(e, err);
            assert!(!e.to_string().contains("secret"));
        }
        let r = ChatRequest::parse(br#"{"model":"m","messages":[{"content":"secret-prompt"}]}"#)
            .unwrap();
        assert!(!format!("{r:?}").contains("secret"));
    }

    #[test]
    fn sse_parsing_and_usage() {
        let mut d = SseDecoder::new();
        let stream = b"data: {\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\"}}]}\n\n\
            : keep-alive\r\n\r\n\
            data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"Hi\"}}]}\r\n\r\n\
            data: {\"choices\":[],\"usage\":{\"prompt_tokens\":9,\"completion_tokens\":2}}\n\ndata: [DONE]\n\n";
        let mut events = Vec::new();
        for part in stream.chunks(7) {
            events.extend(d.push(part).unwrap());
        }
        assert_eq!(events.len(), 4);
        assert_eq!(events[3], SseEvent::Done);
        let SseEvent::Data(first) = &events[0] else {
            panic!()
        };
        let SseEvent::Data(second) = &events[1] else {
            panic!()
        };
        let SseEvent::Data(third) = &events[2] else {
            panic!()
        };
        assert!(!has_content(first));
        assert!(has_content(second));
        assert_eq!(
            usage_of(third),
            Some(Usage {
                prompt_tokens: 9,
                completion_tokens: 2
            })
        );
        assert_eq!(usage_of(second), None);
        assert_eq!(sse_data(b"{}"), b"data: {}\n\n");
    }

    #[test]
    fn assembling_a_completion() {
        let mut a = Assembler::new();
        a.push(br#"{"id":"c1","model":"m","created":5,"choices":[{"index":0,"delta":{"role":"assistant","content":"Hel"}}]}"#);
        a.push(br#"{"id":"c1","choices":[{"index":0,"delta":{"content":"lo"},"finish_reason":"stop"}]}"#);
        let v: Value = serde_json::from_slice(&a.finish(Usage {
            prompt_tokens: 3,
            completion_tokens: 2,
        }))
        .unwrap();
        assert_eq!(v["object"], "chat.completion");
        assert_eq!(v["id"], "c1");
        assert_eq!(v["choices"][0]["message"]["content"], "Hello");
        assert_eq!(v["choices"][0]["finish_reason"], "stop");
        assert_eq!(v["usage"]["total_tokens"], 5);
    }

    #[test]
    fn error_bodies() {
        let v: Value =
            serde_json::from_slice(&error_json(ErrorCode::PaymentRequired, "short by $0.01"))
                .unwrap();
        assert_eq!(v["error"]["code"], "payment_required");
        assert_eq!(v["error"]["message"], "short by $0.01");
    }
}
