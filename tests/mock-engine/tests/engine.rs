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
    use ac_market_proto::engine::{ENGINE_PROTOCOL_VERSION, EngineMsg, EngineReader};
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
                        version: ENGINE_PROTOCOL_VERSION,
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
                    preempt_after: None,
                    mode: ac_market_proto::engine::EngineMode::Prove,
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

// m6-public-jobs 5.5: with `preempt_after`, the request is preempted after two decode steps and
// recomputed as the vLLM plugin splits a recomputation: the prompt as one prefill segment, then
// one decode segment per generated row; the decode rows end as without a preemption.
#[tokio::test]
async fn the_engine_plays_a_preemption() {
    use ac_toploc::Phase;
    let dir = std::env::temp_dir().join(format!("ac-mock-preempt-{}", std::process::id()));
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
                half_decode: false,
                preempt_after: Some(2),
                mode: ac_market_proto::engine::EngineMode::Prove,
            }),
            ..Config::default()
        },
    )
    .await
    .unwrap();
    let header = ac_market_proto::engine::request_id_header(&[0x3d; 32]);
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
    let segs = segments_of(&mut rx).await;
    let shape: Vec<(Phase, u32)> = segs.iter().map(|s| (s.phase, s.len)).collect();
    let mut expected = vec![
        (Phase::Prefill, 2 * 256),
        (Phase::Prefill, 3 * 256),
        (Phase::Decode, 256),
        (Phase::Decode, 256),
        (Phase::Prefill, 5 * 256),
    ];
    expected.extend([(Phase::Decode, 256); 4]);
    assert_eq!(shape, expected);
    std::fs::remove_dir_all(&dir).unwrap();
}

/// The segments of one request, up to its end marker.
async fn segments_of(
    rx: &mut tokio::sync::mpsc::UnboundedReceiver<ac_market_proto::engine::EngineMsg>,
) -> Vec<ac_toploc::Segment> {
    use ac_market_proto::engine::EngineMsg;
    let mut out = Vec::new();
    loop {
        match rx.recv().await.unwrap() {
            EngineMsg::Segment {
                phase,
                len,
                candidates,
                ..
            } => out.push(ac_toploc::Segment {
                phase,
                len,
                candidates,
            }),
            EngineMsg::Finish { .. } => return out,
            other => panic!("unexpected {other:?}"),
        }
    }
}

async fn engine_with(
    socket: std::path::PathBuf,
    mode: ac_market_proto::engine::EngineMode,
    seed: u64,
) -> ac_mock_engine::Engine {
    spawn(
        "127.0.0.1:0",
        Config {
            ttft: Duration::ZERO,
            token_interval: Duration::ZERO,
            toploc: Some(ac_mock_engine::PluginConfig {
                socket,
                half_decode: false,
                preempt_after: None,
                mode,
            }),
            model_seed: seed,
            ..Config::default()
        },
    )
    .await
    .unwrap()
}

async fn post_json(base: &str, path: &str, body: &Value, id: Option<&str>) -> Value {
    let headers: Vec<(&str, &str)> = id.map(|i| ("x-request-id", i)).into_iter().collect();
    let resp = Client::new()
        .unwrap()
        .post_with(
            &join(base, path),
            "application/json",
            &headers,
            body.to_string(),
        )
        .await
        .unwrap();
    serde_json::from_slice(&read_body(resp.into_body(), 1 << 20).await.unwrap()).unwrap()
}

// m6-toploc-verify 5.2: an answer's proofs (prove mode) match the rows a verify-mode engine of
// the same seed sends for "prompt + output but the last token", re-tokenized through
// /tokenize and /detokenize; another seed (another model) does not.
#[tokio::test]
async fn prove_and_verify_modes_agree_on_a_sequence() {
    use ac_market_proto::MARKET_PARAMS;
    use ac_market_proto::engine::EngineMode;
    use ac_toploc::{Phase, build_proofs_from_candidates, compare_from_candidates};
    let dir = std::env::temp_dir().join(format!("ac-mock-verify-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    let mut prove_rx = toploc_receiver(dir.join("p.sock")).await;
    let prover = engine_with(dir.join("p.sock"), EngineMode::Prove, 7).await;
    let chat = json!({
        "model": "mock-model",
        "messages": [{"role": "user", "content": "one two three four five six"}],
        "max_tokens": 40,
    });
    let reply = post_json(
        &prover.url(),
        "/v1/chat/completions",
        &chat,
        Some(&"ab".repeat(32)),
    )
    .await;
    let proofs =
        build_proofs_from_candidates(&segments_of(&mut prove_rx).await, &MARKET_PARAMS).unwrap();
    assert_eq!(proofs.len(), 1 + 39usize.div_ceil(32));
    let output = reply["choices"][0]["message"]["content"].as_str().unwrap();
    let (prompt_n, out_n) = (
        reply["usage"]["prompt_tokens"].as_u64().unwrap(),
        reply["usage"]["completion_tokens"].as_u64().unwrap(),
    );

    for (seed, same) in [(7, true), (8, false)] {
        let socket = dir.join(format!("v{seed}.sock"));
        let mut rx = toploc_receiver(socket.clone()).await;
        let verifier = engine_with(socket, EngineMode::Verify, seed).await;
        let base = verifier.url();
        let prompt = post_json(
            &base,
            "/tokenize",
            &json!({"model": "mock-model", "messages": chat["messages"], "add_generation_prompt": true}),
            None,
        )
        .await;
        let out = post_json(
            &base,
            "/tokenize",
            &json!({"model": "mock-model", "prompt": output, "add_special_tokens": false}),
            None,
        )
        .await;
        let back = post_json(
            &base,
            "/detokenize",
            &json!({"model": "mock-model", "tokens": out["tokens"]}),
            None,
        )
        .await;
        assert_eq!(back["prompt"], output);
        let prompt_ids = prompt["tokens"].as_array().unwrap();
        let out_ids = out["tokens"].as_array().unwrap();
        assert_eq!(
            (prompt_ids.len() as u64, out_ids.len() as u64),
            (prompt_n, out_n)
        );
        let mut ids: Vec<Value> = prompt_ids.clone();
        ids.extend(out_ids[..out_ids.len() - 1].iter().cloned());
        let done = post_json(
            &base,
            "/v1/completions",
            &json!({"model": "mock-model", "prompt": ids, "max_tokens": 1}),
            Some(&"cd".repeat(32)),
        )
        .await;
        assert_eq!(done["usage"]["prompt_tokens"], ids.len());
        let mut rows = segments_of(&mut rx).await;
        assert_eq!(rows.len(), ids.len());
        assert!(
            rows.iter()
                .all(|r| r.phase == Phase::Prefill && r.len == 256)
        );
        for r in rows.iter_mut().skip(prompt_ids.len()) {
            r.phase = Phase::Decode;
        }
        let cmp = compare_from_candidates(&rows, &proofs, &MARKET_PARAMS).unwrap();
        let exact = cmp
            .iter()
            .all(|c| c.exp_mismatches == 0 && c.mant_err_sum == 0);
        assert_eq!(exact, same, "seed {seed}");
    }
    std::fs::remove_dir_all(&dir).unwrap();
}
