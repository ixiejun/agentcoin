//! Spec `market/provider-agent` against an in-process mock engine and a simulated gateway
//! (m5-gateway-provider 4.3-4.5).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ac_crypto::kem::KemSecretKey;
use ac_crypto::sig::{SecretSeed, SigningKey, verify};
use ac_crypto::{KemAlg, KemPublicKey, OsRng, SigAlg, account_id};
use ac_market_proto::{ErrorCode, ProviderMsg, ProviderReq, msg};
use ac_mock_engine::Engine;
use ac_primitives::market::receipt::RECEIPT_CONTEXT;
use ac_primitives::market::work::JobKind;
use ac_primitives::market::{MicroUsd, ModelId, PricePerMTok, SignedReceipt};
use ac_provider::chain::{GatewayView, StaticDirectory};
use ac_provider::logging::{self, Sink};
use ac_provider::{Config, ModelEntry, Service};
use ac_wallet::http::Client;
use ac_wallet::sealed_http;
use sp_core::H256;
use sp_runtime::AccountId32;

const MODEL: ModelId = ModelId([1; 32]);
const PRICE: PricePerMTok = PricePerMTok {
    input: MicroUsd(100_000),
    output: MicroUsd(200_000),
};
const MARKER: &str = "zebra-marker-5f2c";

static LOGS: Mutex<Vec<String>> = Mutex::new(Vec::new());

struct Party {
    account: AccountId32,
    key: SigningKey,
}

fn party(seed: u8) -> Party {
    let key = SigningKey::from_seed(SigAlg::MlDsa44, &SecretSeed::new([seed; 32])).unwrap();
    let account = AccountId32::new(*account_id(&key.public_key().unwrap()).as_bytes());
    Party { account, key }
}

struct Setup {
    base: String,
    kem: KemPublicKey,
    engine: Engine,
    gateway: Party,
    provider: Party,
    service: Arc<Service>,
    data: PathBuf,
}

async fn setup(label: &str, engine: ac_mock_engine::Config) -> Setup {
    setup_with(label, engine, None).await
}

/// With `toploc = Some(half_decode)` the provider listens for the plugin and the mock engine
/// plays it.
async fn setup_with(
    label: &str,
    mut engine: ac_mock_engine::Config,
    toploc: Option<bool>,
) -> Setup {
    logging::init(log::LevelFilter::Debug, Sink::Buffer(&LOGS));
    let data =
        std::env::temp_dir().join(format!("ac-provider-test-{label}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&data);
    std::fs::create_dir_all(&data).unwrap();
    let collector = toploc.map(|half_decode| {
        let socket = data.join("toploc.sock");
        engine.toploc = Some(ac_mock_engine::PluginConfig {
            socket: socket.clone(),
            half_decode,
        });
        ac_provider::toploc::Collector::listen(&socket).unwrap()
    });
    let engine = ac_mock_engine::spawn("127.0.0.1:0", engine).await.unwrap();
    let (gateway, inactive, provider) = (party(10), party(11), party(12));
    let kem_secret = KemSecretKey::from_seed(KemAlg::XWing, &SecretSeed::new([13; 32])).unwrap();
    let kem = kem_secret.public_key().unwrap();
    let mut dir = HashMap::new();
    dir.insert(
        gateway.account.clone(),
        GatewayView {
            key: gateway.key.public_key().unwrap(),
            active: true,
        },
    );
    dir.insert(
        inactive.account.clone(),
        GatewayView {
            key: inactive.key.public_key().unwrap(),
            active: false,
        },
    );
    let service = Service::new(
        Config {
            account: provider.account.clone(),
            key: SigningKey::from_seed(SigAlg::MlDsa44, &SecretSeed::new([12; 32])).unwrap(),
            kem: kem_secret,
            kem_public: kem.clone(),
            genesis: H256([7; 32]),
            models: BTreeMap::from([(
                MODEL,
                ModelEntry {
                    engine_name: "mock-model".into(),
                    price: PRICE,
                },
            )]),
            engine: engine.url(),
            store_dir: data.join("receipts"),
            toploc: collector,
        },
        Box::new(StaticDirectory(dir)),
    )
    .unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let svc = Arc::clone(&service);
    tokio::spawn(ac_wallet::http::serve(listener, move |req| {
        Arc::clone(&svc).handle(req)
    }));
    Setup {
        base,
        kem,
        engine,
        gateway,
        provider,
        service,
        data,
    }
}

fn infer(model: ModelId, prompt: &str, max_tokens: u32) -> Vec<u8> {
    let request = serde_json::json!({
        "model": "anything",
        "messages": [{ "role": "user", "content": prompt }],
        "max_tokens": max_tokens,
    });
    msg::encode(&ProviderReq::Infer {
        request_id: [9; 32],
        kind: JobKind::Inference,
        model,
        request: request.to_string().into_bytes(),
    })
}

/// Sends `message` as `who` and collects the reply messages with their arrival times.
async fn call(
    s: &Setup,
    who: &Party,
    message: &[u8],
) -> anyhow::Result<Vec<(Duration, ProviderMsg)>> {
    let start = Instant::now();
    let mut resp = sealed_http::post(
        &Client::new()?,
        &s.base,
        &s.kem,
        (&who.account, &who.key),
        message,
    )
    .await?;
    let mut out = Vec::new();
    while let Some(m) = resp.next().await? {
        out.push((start.elapsed(), msg::decode::<ProviderMsg>(&m)?));
    }
    Ok(out)
}

fn fast() -> ac_mock_engine::Config {
    ac_mock_engine::Config {
        ttft: Duration::ZERO,
        token_interval: Duration::ZERO,
        ..Default::default()
    }
}

// Scenario "非网关账户": refused before the engine is contacted; an inactive gateway too.
#[tokio::test]
async fn only_active_gateways_are_served() {
    let s = setup("gateways", fast()).await;
    let stranger = party(20);
    let err = call(&s, &stranger, &infer(MODEL, "hi", 4))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("403"), "{err}");
    let inactive = party(11);
    let err = call(&s, &inactive, &infer(MODEL, "hi", 4))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("403"), "{err}");
    // A registered gateway's account with someone else's key: authentication fails.
    let impostor = Party {
        account: s.gateway.account.clone(),
        key: party(21).key,
    };
    let err = call(&s, &impostor, &infer(MODEL, "hi", 4))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("401"), "{err}");
    assert_eq!(s.engine.requests(), 0);
}

// Scenario "未登记的模型".
#[tokio::test]
async fn unregistered_models_are_refused() {
    let s = setup("models", fast()).await;
    let reply = call(&s, &s.gateway, &infer(ModelId([2; 32]), "hi", 4))
        .await
        .unwrap();
    assert!(matches!(
        &reply[..],
        [(
            _,
            ProviderMsg::Error {
                code: ErrorCode::ModelNotFound,
                ..
            }
        )]
    ));
    let other_job = msg::encode(&ProviderReq::Infer {
        request_id: [1; 32],
        kind: JobKind::Embed,
        model: MODEL,
        request: b"{}".to_vec(),
    });
    let reply = call(&s, &s.gateway, &other_job).await.unwrap();
    assert!(matches!(
        &reply[..],
        [(
            _,
            ProviderMsg::Error {
                code: ErrorCode::UnsupportedJob,
                ..
            }
        )]
    ));
    assert_eq!(s.engine.requests(), 0);
}

// Scenario "边生成边回传": the first token arrives long before the engine finishes.
#[tokio::test]
async fn tokens_stream_while_generating() {
    let s = setup(
        "stream",
        ac_mock_engine::Config {
            ttft: Duration::ZERO,
            token_interval: Duration::from_millis(40),
            ..Default::default()
        },
    )
    .await;
    let reply = call(&s, &s.gateway, &infer(MODEL, "one two", 20))
        .await
        .unwrap();
    let first = reply
        .iter()
        .find(
            |(_, m)| matches!(m, ProviderMsg::Delta(d) if ac_market_proto::openai::has_content(d)),
        )
        .map(|(t, _)| *t)
        .unwrap();
    let (end, last) = reply.last().unwrap();
    assert!(matches!(last, ProviderMsg::Receipt { .. }));
    // 20 tokens at 40 ms: the engine needs about 760 ms; the first token must not wait for it.
    assert!(first * 3 < *end, "first token at {first:?}, end at {end:?}");
}

// Scenario "引擎中途失败".
#[tokio::test]
async fn engine_failure_ends_without_a_receipt() {
    let s = setup(
        "failure",
        ac_mock_engine::Config {
            fail_after: Some(2),
            ..fast()
        },
    )
    .await;
    let reply = call(&s, &s.gateway, &infer(MODEL, "one two three", 10))
        .await
        .unwrap();
    assert!(matches!(
        reply.last(),
        Some((
            _,
            ProviderMsg::Error {
                code: ErrorCode::EngineFailed,
                ..
            }
        ))
    ));
    assert!(
        !reply
            .iter()
            .any(|(_, m)| matches!(m, ProviderMsg::Receipt { .. }))
    );
}

// Scenario "费用按链上价格计算" (1,000 + 500 tokens at $0.1 / $0.2 per million = 200
// micro-dollars), then the co-signed receipt is kept.
#[tokio::test]
async fn receipts_charge_the_listed_price_and_are_kept() {
    let s = setup("fees", fast()).await;
    let prompt = vec!["w"; 1_000].join(" ");
    let reply = call(&s, &s.gateway, &infer(MODEL, &prompt, 500))
        .await
        .unwrap();
    let Some((
        _,
        ProviderMsg::Receipt {
            body,
            key,
            signature,
            usage,
            ..
        },
    )) = reply.last().cloned()
    else {
        panic!("no receipt: {:?}", reply.last());
    };
    assert_eq!((usage.prompt_tokens, usage.completion_tokens), (1_000, 500));
    assert_eq!(body.fee, MicroUsd(200));
    assert_eq!((body.in_tokens, body.out_tokens), (1_000, 500));
    assert_eq!(body.toploc_commit, [0; 32]);
    assert_eq!(body.request_id, [9; 32]);
    assert_eq!(
        (&body.gateway, &body.provider, body.model),
        (&s.gateway.account, &s.provider.account, MODEL)
    );
    assert!(body.ttft_ms <= body.total_ms);
    verify(&key, &body.payload().unwrap(), RECEIPT_CONTEXT, &signature).unwrap();

    // The gateway co-signs and hands it back; the provider keeps it.
    let payload = body.payload().unwrap();
    let cosigned = SignedReceipt {
        gateway_sig: s
            .gateway
            .key
            .sign(&payload, RECEIPT_CONTEXT, &mut OsRng::new().unwrap())
            .unwrap(),
        gateway_key: s.gateway.key.public_key().unwrap(),
        provider_key: key,
        provider_sig: signature,
        body,
    };
    let ack = call(
        &s,
        &s.gateway,
        &msg::encode(&ProviderReq::Cosigned(cosigned.clone())),
    )
    .await
    .unwrap();
    assert!(matches!(&ack[..], [(_, ProviderMsg::Ack)]));
    let stored = s.service.store().list().unwrap();
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0].1, cosigned);
    // A tampered receipt is refused.
    let mut bad = cosigned;
    bad.body.fee = MicroUsd(201);
    let reply = call(&s, &s.gateway, &msg::encode(&ProviderReq::Cosigned(bad)))
        .await
        .unwrap();
    assert!(matches!(
        &reply[..],
        [(
            _,
            ProviderMsg::Error {
                code: ErrorCode::BadRequest,
                ..
            }
        )]
    ));
    assert_eq!(s.service.store().list().unwrap().len(), 1);
}

// Scenario "日志中没有输出内容": at debug level neither the prompt nor the output (which echoes
// it) appears in the logs or the data directory.
#[tokio::test]
async fn no_request_content_is_logged_or_stored() {
    let s = setup("privacy", fast()).await;
    let reply = call(
        &s,
        &s.gateway,
        &infer(MODEL, &format!("please repeat {MARKER}"), 6),
    )
    .await
    .unwrap();
    let Some((
        _,
        ProviderMsg::Receipt {
            body,
            key,
            signature,
            ..
        },
    )) = reply.last().cloned()
    else {
        panic!("no receipt");
    };
    let echoed = reply.iter().any(
        |(_, m)| matches!(m, ProviderMsg::Delta(d) if String::from_utf8_lossy(d).contains(MARKER)),
    );
    assert!(echoed, "the engine output carries the marker");
    let payload = body.payload().unwrap();
    let cosigned = SignedReceipt {
        gateway_sig: s
            .gateway
            .key
            .sign(&payload, RECEIPT_CONTEXT, &mut OsRng::new().unwrap())
            .unwrap(),
        gateway_key: s.gateway.key.public_key().unwrap(),
        provider_key: key,
        provider_sig: signature,
        body,
    };
    call(
        &s,
        &s.gateway,
        &msg::encode(&ProviderReq::Cosigned(cosigned)),
    )
    .await
    .unwrap();
    let logs = LOGS.lock().unwrap().join("\n");
    assert!(logs.contains("served"), "the request was logged: {logs}");
    assert!(!logs.contains(MARKER));
    for entry in walk(&s.data) {
        let bytes = std::fs::read(&entry).unwrap();
        assert!(
            !String::from_utf8_lossy(&bytes).contains(MARKER),
            "{}",
            entry.display()
        );
    }
}

fn walk(dir: &std::path::Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let p = e.path();
        if p.is_dir() {
            out.extend(walk(&p));
        } else {
            out.push(p);
        }
    }
    out
}

/// Co-signs a provider receipt as the gateway would.
fn cosign(s: &Setup, reply: &[(Duration, ProviderMsg)]) -> SignedReceipt {
    let Some((
        _,
        ProviderMsg::Receipt {
            body,
            key,
            signature,
            ..
        },
    )) = reply.last().cloned()
    else {
        panic!("no receipt: {:?}", reply.last());
    };
    let payload = body.payload().unwrap();
    SignedReceipt {
        gateway_sig: s
            .gateway
            .key
            .sign(&payload, RECEIPT_CONTEXT, &mut OsRng::new().unwrap())
            .unwrap(),
        gateway_key: s.gateway.key.public_key().unwrap(),
        provider_key: key,
        provider_sig: signature,
        body,
    }
}

// m5-engine-toploc 4.2, scenarios "带证明的收据", "证明随收据保存与删除" and "不保存候选": the
// receipt commits to proofs built from the plugin's candidates, the proofs are kept with the
// co-signed receipt and pruned with it, and only receipts and proofs reach the disk.
#[tokio::test]
async fn receipts_commit_to_toploc_proofs_kept_until_pruned() {
    let s = setup_with("toploc", fast(), Some(false)).await;
    let prompt = format!("w w w {MARKER}");
    let reply = call(&s, &s.gateway, &infer(MODEL, &prompt, 40))
        .await
        .unwrap();
    let Some((_, ProviderMsg::Receipt { body, toploc, .. })) = reply.last().cloned() else {
        panic!("no receipt: {:?}", reply.last());
    };
    let proofs = toploc.expect("the receipt comes with proofs");
    assert_ne!(body.toploc_commit, [0; 32]);
    // 40 output tokens: the prefill and 39 decode steps in 2 batches.
    assert_eq!(proofs.proofs.len(), 3);
    ac_market_proto::toploc::check(&body, Some(&proofs)).unwrap();
    assert_eq!(s.service.toploc_missing(), 0);

    let cosigned = cosign(&s, &reply);
    let ack = call(
        &s,
        &s.gateway,
        &msg::encode(&ProviderReq::Cosigned(cosigned)),
    )
    .await
    .unwrap();
    assert!(matches!(&ack[..], [(_, ProviderMsg::Ack)]));
    assert_eq!(s.service.store().toploc_of(&[9; 32]).unwrap(), Some(proofs));
    // Besides the plugin socket, only the receipt file is in the data directory.
    let stored: Vec<PathBuf> = walk(&s.data)
        .into_iter()
        .filter(|p| p.extension().is_none_or(|e| e != "sock"))
        .collect();
    let files: Vec<String> = stored
        .iter()
        .map(|p| p.strip_prefix(&s.data).unwrap().display().to_string())
        .collect();
    assert_eq!(files, [format!("receipts/{}.json", hex::encode([9u8; 32]))]);
    let logs = LOGS.lock().unwrap().join("\n");
    assert!(!logs.contains(MARKER));
    for entry in stored {
        assert!(!String::from_utf8_lossy(&std::fs::read(&entry).unwrap()).contains(MARKER));
    }
    // Pruned after the challenge period: the proofs go with the receipt.
    assert_eq!(s.service.store().prune(u64::MAX).unwrap(), 1);
    assert!(s.service.store().toploc_of(&[9; 32]).is_err());
}

// Scenario "步骤数不符": too few decode segments give an all-zero commitment and no proofs.
#[tokio::test]
async fn incomplete_candidates_give_no_proof() {
    let s = setup_with("toploc-half", fast(), Some(true)).await;
    let reply = call(&s, &s.gateway, &infer(MODEL, "a b c", 10))
        .await
        .unwrap();
    let Some((_, ProviderMsg::Receipt { body, toploc, .. })) = reply.last().cloned() else {
        panic!("no receipt");
    };
    assert_eq!(body.toploc_commit, [0; 32]);
    assert!(toploc.is_none());
    assert_eq!(s.service.toploc_missing(), 1);
    ac_market_proto::toploc::check(&body, None).unwrap();
}
