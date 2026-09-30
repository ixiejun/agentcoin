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

/// Receives the stand-in plugin's messages on a Unix socket (as a provider would).
async fn toploc_receiver(
    path: std::path::PathBuf,
) -> tokio::sync::mpsc::UnboundedReceiver<ac_market_proto::engine::EngineMsg> {
    use ac_market_proto::engine::{EngineMsg, EngineReader};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::UnixListener::bind(&path).unwrap();
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    tokio::spawn(async move {
        let (mut s, _) = listener.accept().await.unwrap();
        let mut reader = EngineReader::new();
        let mut buf = vec![0u8; 65536];
        let mut greeted = false;
        loop {
            let n = s.read(&mut buf).await.unwrap();
            if n == 0 {
                return;
            }
            reader.push(&buf[..n]);
            while let Some(m) = reader.next_msg().unwrap() {
                if !greeted {
                    assert!(matches!(
                        m,
                        EngineMsg::Hello {
                            hidden_size: 256,
                            ..
                        }
                    ));
                    let w = EngineMsg::Welcome {
                        version: 1,
                        topk: 128,
                    };
                    s.write_all(&w.to_frame().unwrap()).await.unwrap();
                    greeted = true;
                } else {
                    tx.send(m).unwrap();
                }
            }
        }
    });
    rx
}

// m5-engine-toploc 6.1: the engine plays the TOPLOC plugin: a prefill of prompt × 256 values,
// one decode segment per output token but the last, the end marker, all under vLLM's request
// ID derived from X-Request-Id.
#[tokio::test]
async fn the_engine_plays_the_toploc_plugin() {
    use ac_market_proto::engine::{EngineMsg, market_request_id};
    use ac_toploc::Phase;
    for (half, decode) in [(false, 4), (true, 2)] {
        let dir =
            std::env::temp_dir().join(format!("ac-mock-toploc-{}-{half}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let socket = dir.join("t.sock");
        let mut rx = toploc_receiver(socket.clone()).await;
        let engine = spawn(
            "127.0.0.1:0",
            Config {
                ttft: Duration::ZERO,
                token_interval: Duration::ZERO,
                toploc: Some(ac_mock_engine::PluginConfig {
                    socket,
                    half_decode: half,
                }),
                ..Config::default()
            },
        )
        .await
        .unwrap();
        let id = [0x3cu8; 32];
        let header = ac_market_proto::engine::request_id_header(&id);
        let resp = Client::new()
            .unwrap()
            .post_with(
                &join(&engine.url(), "/v1/chat/completions"),
                "application/json",
                &[("x-request-id", &header)],
                request(true).to_string(),
            )
            .await
            .unwrap();
        read_body(resp.into_body(), 1 << 20).await.unwrap();
        // 5 prompt words (2 + 3 in two prefill steps), 5 output tokens.
        let mut prefill = Vec::new();
        let mut decodes = 0;
        loop {
            match rx.recv().await.unwrap() {
                EngineMsg::Segment {
                    request,
                    phase,
                    len,
                    candidates,
                } => {
                    assert_eq!(market_request_id(&request), Some(id));
                    assert_eq!(candidates.len(), 128);
                    match phase {
                        Phase::Prefill => prefill.push(len),
                        _ => {
                            assert_eq!(len, 256);
                            decodes += 1;
                        }
                    }
                }
                EngineMsg::Finish { request } => {
                    assert_eq!(market_request_id(&request), Some(id));
                    break;
                }
                other => panic!("unexpected {other:?}"),
            }
        }
        assert_eq!(prefill, [2 * 256, 3 * 256]);
        assert_eq!(decodes, decode);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
