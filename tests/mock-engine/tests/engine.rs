//! m5-gateway-provider 3.1: the mock engine is deterministic, streams with usage and fails on
//! demand.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::time::Duration;

use ac_market_proto::openai::{SseDecoder, SseEvent, usage_of};
use ac_mock_engine::{Config, spawn};
use ac_wallet::http::{Client, join, next_chunk, read_body};
use serde_json::{Value, json};

async fn post(client: &Client, base: &str, body: &Value) -> (u16, Vec<u8>) {
    let resp = client
        .post(
            &join(base, "/v1/chat/completions"),
            "application/json",
            body.to_string(),
        )
        .await
        .unwrap();
    let status = resp.status().as_u16();
    let mut body = resp.into_body();
    let mut out = Vec::new();
    while let Ok(Some(c)) = next_chunk(&mut body).await {
        out.extend_from_slice(&c);
    }
    (status, out)
}

fn request(stream: bool) -> Value {
    json!({
        "model": "mock-model",
        "messages": [{"role": "system", "content": "be brief"}, {"role": "user", "content": "hello there world"}],
        "max_tokens": 5,
        "stream": stream,
        "stream_options": {"include_usage": true},
    })
}

fn events(bytes: &[u8]) -> Vec<SseEvent> {
    SseDecoder::new().push(bytes).unwrap()
}

#[tokio::test]
async fn deterministic_output_and_usage() {
    let engine = spawn(
        "127.0.0.1:0",
        Config {
            ttft: Duration::ZERO,
            token_interval: Duration::ZERO,
            ..Config::default()
        },
    )
    .await
    .unwrap();
    let client = Client::new().unwrap();
    let (s1, a) = post(&client, &engine.url(), &request(false)).await;
    let (_, b) = post(&client, &engine.url(), &request(false)).await;
    assert_eq!((s1, &a), (200, &b));
    let v: Value = serde_json::from_slice(&a).unwrap();
    assert_eq!(
        v["choices"][0]["message"]["content"],
        "hello there world lorem ipsum "
    );
    assert_eq!(v["usage"]["prompt_tokens"], 5);
    assert_eq!(v["usage"]["completion_tokens"], 5);

    let (_, s) = post(&client, &engine.url(), &request(true)).await;
    let ev = events(&s);
    assert_eq!(ev.last(), Some(&SseEvent::Done));
    let usage = ev.iter().find_map(|e| match e {
        SseEvent::Data(d) => usage_of(d),
        SseEvent::Done => None,
    });
    assert_eq!(
        usage.map(|u| (u.prompt_tokens, u.completion_tokens)),
        Some((5, 5))
    );
    let text: String = ev
        .iter()
        .filter_map(|e| match e {
            SseEvent::Data(d) => serde_json::from_slice::<Value>(d).ok(),
            SseEvent::Done => None,
        })
        .filter_map(|v| {
            v.pointer("/choices/0/delta/content")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .collect();
    assert_eq!(text, "hello there world lorem ipsum ");
    assert_eq!(engine.requests(), 3);

    let (status, _) = post(
        &client,
        &engine.url(),
        &json!({"model": "other", "messages": []}),
    )
    .await;
    assert_eq!(status, 404);
    let models = client
        .get_bytes(&join(&engine.url(), "/v1/models"), 4096)
        .await
        .unwrap();
    assert!(String::from_utf8_lossy(&models).contains("mock-model"));
    let count = read_body(
        client
            .get(&join(&engine.url(), "/mock/requests"))
            .await
            .unwrap()
            .into_body(),
        64,
    )
    .await
    .unwrap();
    assert_eq!(&count[..], b"4");
}

#[tokio::test]
async fn the_failure_switch_drops_the_stream() {
    let engine = spawn(
        "127.0.0.1:0",
        Config {
            ttft: Duration::ZERO,
            token_interval: Duration::ZERO,
            fail_after: Some(2),
            ..Config::default()
        },
    )
    .await
    .unwrap();
    let (_, s) = post(&Client::new().unwrap(), &engine.url(), &request(true)).await;
    let ev = events(&s);
    assert!(!ev.contains(&SseEvent::Done));
    let content = ev
        .iter()
        .filter(|e| matches!(e, SseEvent::Data(d) if String::from_utf8_lossy(d).contains("\"content\":\"") && !String::from_utf8_lossy(d).contains("\"content\":\"\"")))
        .count();
    assert_eq!(content, 2);
    assert!(
        ev.iter()
            .all(|e| !matches!(e, SseEvent::Data(d) if usage_of(d).is_some()))
    );
}
