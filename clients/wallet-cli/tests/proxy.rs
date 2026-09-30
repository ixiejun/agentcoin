//! Spec `clients/wallet-cli` "本地推理代理" against an in-process simulated gateway
//! (m5-gateway-provider 6.2, 6.3).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use ac_crypto::kem::KemSecretKey;
use ac_crypto::sealed::{Acceptor, ReplayCache, recipient_id};
use ac_crypto::sig::{SecretSeed, SigningKey};
use ac_crypto::{KemAlg, KemPublicKey, OsRng, SigAlg, account_id};
use ac_market_proto::openai::{SseDecoder, SseEvent, usage_of};
use ac_market_proto::{ErrorCode, GatewayMsg, Payment, UserMsg, msg};
use ac_primitives::market::receipt::{RECEIPT_CONTEXT, fee_for};
use ac_primitives::market::voucher::key_fingerprint;
use ac_primitives::market::work::JobKind;
use ac_primitives::market::{MicroUsd, ModelId, PricePerMTok, ReceiptBody, SignedReceipt};
use ac_wallet::http::{self, Client, join, next_chunk};
use ac_wallet::proxy::{Config, Facts, Proxy};
use ac_wallet::sealed_http::{self, now_secs};
use ac_wallet::work::ReceiptFacts;
use anyhow::Result;
use async_trait::async_trait;
use serde_json::{Value, json};
use sp_core::H256;
use sp_runtime::AccountId32;

const GENESIS: H256 = H256([5; 32]);
const MODEL: ModelId = ModelId([8; 32]);
const PRICE: PricePerMTok = PricePerMTok {
    input: MicroUsd(100_000),
    output: MicroUsd(200_000),
};
const USAGE: (u32, u32) = (1_000, 500); // 200 micro-USD per request

fn key(seed: u8) -> SigningKey {
    SigningKey::from_seed(SigAlg::MlDsa44, &SecretSeed::new([seed; 32])).unwrap()
}

fn account(k: &SigningKey) -> AccountId32 {
    AccountId32::new(*account_id(&k.public_key().unwrap()).as_bytes())
}

/// A simulated gateway: strict about vouchers, optionally overcharging.
struct FakeGateway {
    key: SigningKey,
    provider: SigningKey,
    user_key: ac_crypto::PqPublicKey,
    kem: KemSecretKey,
    billed: Mutex<u128>,
    vouchers: Mutex<Vec<u128>>,
    overcharge: bool,
    replay: Mutex<ReplayCache>,
}

impl FakeGateway {
    fn receipt(&self, fee_bump: u128) -> SignedReceipt {
        let fee = fee_for(&PRICE, USAGE.0, USAGE.1).unwrap();
        let body = ReceiptBody {
            genesis: GENESIS,
            gateway: account(&self.key),
            provider: account(&self.provider),
            kind: JobKind::Inference,
            model: MODEL,
            request_id: [1; 32],
            in_tokens: USAGE.0,
            out_tokens: USAGE.1,
            fee: MicroUsd(fee.0 + fee_bump),
            toploc_commit: [0; 32],
            ttft_ms: 1,
            total_ms: 2,
        };
        let p = body.payload().unwrap();
        SignedReceipt {
            provider_sig: self
                .provider
                .sign(&p, RECEIPT_CONTEXT, &mut OsRng::new().unwrap())
                .unwrap(),
            provider_key: self.provider.public_key().unwrap(),
            gateway_sig: self
                .key
                .sign(&p, RECEIPT_CONTEXT, &mut OsRng::new().unwrap())
                .unwrap(),
            gateway_key: self.key.public_key().unwrap(),
            body,
        }
    }

    fn reply(&self, m: UserMsg) -> Vec<GatewayMsg> {
        match m {
            UserMsg::Pay {
                payment: Payment::Transparent(v),
            } => {
                let billed = *self.billed.lock().unwrap();
                self.vouchers.lock().unwrap().push(v.body.cumulative.0);
                if v.body.cumulative.0 == billed {
                    vec![GatewayMsg::Paid {
                        billed_total: MicroUsd(billed),
                    }]
                } else {
                    vec![GatewayMsg::Error {
                        code: ErrorCode::PaymentRequired,
                        message: b"not exact".to_vec(),
                    }]
                }
            }
            UserMsg::Chat {
                request,
                payment: Payment::Transparent(v),
            } => {
                self.vouchers.lock().unwrap().push(v.body.cumulative.0);
                let mut billed = self.billed.lock().unwrap();
                if v.body.cumulative.0 != *billed {
                    return vec![GatewayMsg::Error {
                        code: ErrorCode::PaymentRequired,
                        message: b"unpaid".to_vec(),
                    }];
                }
                let stream = serde_json::from_slice::<Value>(&request).unwrap()["stream"]
                    .as_bool()
                    .unwrap_or(false);
                let receipt = self.receipt(u128::from(self.overcharge));
                let fee = receipt.body.fee;
                *billed += fee.0;
                let usage = json!({ "prompt_tokens": USAGE.0, "completion_tokens": USAGE.1 });
                let billing = GatewayMsg::Billing {
                    receipt,
                    fee,
                    billed_total: MicroUsd(*billed),
                };
                if stream {
                    let chunk = |c: &str| json!({ "id": "x", "object": "chat.completion.chunk", "choices": [{ "index": 0, "delta": { "content": c } }] });
                    vec![
                        GatewayMsg::Delta(chunk("Hel").to_string().into_bytes()),
                        GatewayMsg::Delta(chunk("lo").to_string().into_bytes()),
                        GatewayMsg::Delta(json!({ "id": "x", "object": "chat.completion.chunk", "choices": [], "usage": usage }).to_string().into_bytes()),
                        billing,
                    ]
                } else {
                    let c = json!({ "id": "x", "object": "chat.completion", "choices": [{ "index": 0, "message": { "role": "assistant", "content": "Hello" }, "finish_reason": "stop" }], "usage": usage });
                    vec![GatewayMsg::Completion(c.to_string().into_bytes()), billing]
                }
            }
        }
    }
}

async fn fake_gateway(
    overcharge: bool,
    user_key: &SigningKey,
) -> (String, Arc<FakeGateway>, KemPublicKey) {
    let kem = KemSecretKey::from_seed(KemAlg::XWing, &SecretSeed::new([31; 32])).unwrap();
    let kem_pk = kem.public_key().unwrap();
    let g = Arc::new(FakeGateway {
        key: key(30),
        provider: key(32),
        user_key: user_key.public_key().unwrap(),
        kem,
        billed: Mutex::new(0),
        vouchers: Mutex::new(Vec::new()),
        overcharge,
        replay: Mutex::new(ReplayCache::default()),
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let gw = Arc::clone(&g);
    tokio::spawn(http::serve(listener, move |req: http::Request| {
        let gw = Arc::clone(&gw);
        async move {
            if req.uri().path() == "/ac/v1/models" {
                return http::json(200, json!({ "object": "list", "data": [{ "id": format!("0x{}", hex::encode(MODEL.0)), "name": "Qwen-test" }] }).to_string());
            }
            let body = http::read_body(req.into_body(), 1 << 24).await.unwrap();
            let rid = recipient_id(&gw.kem.public_key().unwrap()).unwrap();
            let acceptor = Acceptor {
                secret: &gw.kem,
                recipient: rid,
                now: now_secs(),
            };
            let user_key = gw.user_key.clone();
            let accepted = sealed_http::accept(
                &body,
                &acceptor,
                |_| Some(user_key),
                &mut gw.replay.lock().unwrap(),
            );
            let request = match accepted {
                Ok(r) => r,
                Err(r) => return r.response(),
            };
            let (mut w, resp, _, message) = request.respond();
            let replies = gw.reply(msg::decode(&message).unwrap());
            tokio::spawn(async move {
                let n = replies.len();
                for (i, r) in replies.into_iter().enumerate() {
                    w.send(&msg::encode(&r), i + 1 == n).await;
                }
            });
            resp
        }
    }));
    (url, g, kem_pk)
}

struct FixedFacts(ReceiptFacts);

#[async_trait]
impl Facts for FixedFacts {
    async fn receipt_facts(&self, _body: &ReceiptBody) -> Result<ReceiptFacts> {
        Ok(self.0.clone())
    }
}

struct Setup {
    base: String,
    proxy: Arc<Proxy>,
    gateway: Arc<FakeGateway>,
    state: PathBuf,
    url: String,
    kem: KemPublicKey,
}

async fn setup(label: &str, overcharge: bool, max_usd: Option<MicroUsd>) -> Setup {
    let user = key(20);
    let (url, gateway, kem) = fake_gateway(overcharge, &user).await;
    let state = std::env::temp_dir().join(format!(
        "ac-wallet-proxy-{label}-{}.json",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&state);
    let proxy = start(&user, &gateway, (url.clone(), kem.clone()), max_usd, &state).await;
    let base = serve(&proxy).await;
    Setup {
        base,
        proxy,
        gateway,
        state,
        url,
        kem,
    }
}

async fn start(
    user: &SigningKey,
    gateway: &FakeGateway,
    (url, kem): (String, KemPublicKey),
    max_usd: Option<MicroUsd>,
    state: &std::path::Path,
) -> Arc<Proxy> {
    let facts = ReceiptFacts {
        genesis: GENESIS,
        provider_key: key_fingerprint(&gateway.provider.public_key().unwrap()),
        gateway_key: key_fingerprint(&gateway.key.public_key().unwrap()),
        price: Some(PRICE),
    };
    Proxy::new(
        Config {
            user: account(user),
            key: user.clone(),
            genesis: GENESIS,
            gateway: account(&gateway.key),
            gateway_url: url,
            gateway_kem: kem,
            channel: 0,
            max_usd,
            state_path: state.to_path_buf(),
        },
        Box::new(FixedFacts(facts)),
    )
    .unwrap()
}

async fn serve(proxy: &Arc<Proxy>) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let p = Arc::clone(proxy);
    tokio::spawn(http::serve(listener, move |req| Arc::clone(&p).handle(req)));
    base
}

async fn chat(base: &str, stream: bool) -> (u16, Vec<u8>) {
    let body = json!({ "model": "Qwen-test", "messages": [{ "role": "user", "content": "hi" }], "stream": stream });
    let resp = Client::new()
        .unwrap()
        .post(
            &join(base, "/v1/chat/completions"),
            "application/json",
            body.to_string(),
        )
        .await
        .unwrap();
    let status = resp.status().as_u16();
    let mut b = resp.into_body();
    let mut out = Vec::new();
    while let Ok(Some(c)) = next_chunk(&mut b).await {
        out.extend_from_slice(&c);
    }
    (status, out)
}

// SSE lines in OpenAI format with [DONE]; usage equals the receipt's; the bill is paid exactly.
#[tokio::test]
async fn streams_openai_events_and_pays_each_receipt() {
    let s = setup("stream", false, None).await;
    let (status, body) = chat(&s.base, true).await;
    assert_eq!(status, 200);
    let text = String::from_utf8(body.clone()).unwrap();
    assert!(text.starts_with("data: {"), "{text}");
    assert!(text.ends_with("data: [DONE]\n\n"), "{text}");
    let events = SseDecoder::new().push(&body).unwrap();
    let usage = events.iter().find_map(|e| match e {
        SseEvent::Data(d) => usage_of(d),
        SseEvent::Done => None,
    });
    assert_eq!(
        usage.map(|u| (u.prompt_tokens, u.completion_tokens)),
        Some(USAGE)
    );
    assert_eq!(s.proxy.paid().await.total, MicroUsd(200));
    // Non-streamed: one JSON completion with the receipt's usage.
    let (status, body) = chat(&s.base, false).await;
    assert_eq!(status, 200);
    let v: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(v["object"], "chat.completion");
    assert_eq!(v["usage"]["completion_tokens"], USAGE.1);
    assert_eq!(s.proxy.paid().await.total, MicroUsd(400));
    // Vouchers: 0 with the first request, 200 to pay it, 200 with the second, 400 to pay it.
    assert_eq!(*s.gateway.vouchers.lock().unwrap(), [0, 200, 200, 400]);
    // Models are relayed.
    let models = Client::new()
        .unwrap()
        .get_bytes(&join(&s.base, "/v1/models"), 65_536)
        .await
        .unwrap();
    assert!(String::from_utf8_lossy(&models).contains("Qwen-test"));
}

// Scenario "费用不符时拒绝付费".
#[tokio::test]
async fn overcharged_receipts_halt_payments() {
    let s = setup("overcharge", true, None).await;
    let (_, body) = chat(&s.base, true).await;
    assert!(String::from_utf8_lossy(&body).contains("payment_required"));
    assert_eq!(s.proxy.paid().await.total, MicroUsd(0));
    assert!(s.proxy.halted().unwrap().contains("receipt check failed"));
    // Only the request's own voucher (0) was sent: no payment.
    assert_eq!(*s.gateway.vouchers.lock().unwrap(), [0]);
    let (status, _) = chat(&s.base, false).await;
    assert_eq!(status, 402);
    assert_eq!(
        *s.gateway.vouchers.lock().unwrap(),
        [0],
        "halted: the gateway is not contacted"
    );
}

// Scenario "花费上限".
#[tokio::test]
async fn the_spending_limit_stops_requests() {
    let s = setup("limit", false, Some(MicroUsd(200))).await;
    assert_eq!(chat(&s.base, false).await.0, 200);
    let before = s.gateway.vouchers.lock().unwrap().len();
    let (status, body) = chat(&s.base, false).await;
    assert_eq!(status, 402);
    assert!(String::from_utf8_lossy(&body).contains("max-usd"));
    assert_eq!(s.gateway.vouchers.lock().unwrap().len(), before);
}

// Scenario "重启后不重复付费".
#[tokio::test]
async fn restarts_resume_from_the_paid_total() {
    let s = setup("restart", false, None).await;
    chat(&s.base, false).await;
    chat(&s.base, false).await;
    assert_eq!(s.proxy.paid().await.total, MicroUsd(400));
    let again = start(
        &key(20),
        &s.gateway,
        (s.url.clone(), s.kem.clone()),
        None,
        &s.state,
    )
    .await;
    assert_eq!(again.paid().await.total, MicroUsd(400));
    let base = serve(&again).await;
    assert_eq!(chat(&base, false).await.0, 200);
    let v = s.gateway.vouchers.lock().unwrap().clone();
    assert_eq!(&v[v.len() - 2..], [400, 600]);
}

// Two concurrent requests: each voucher equals the gateway's billed total when it arrives.
#[tokio::test]
async fn concurrent_requests_carry_exact_vouchers() {
    let s = setup("concurrent", false, None).await;
    let (a, b) = tokio::join!(chat(&s.base, false), chat(&s.base, true));
    assert_eq!((a.0, b.0), (200, 200));
    assert_eq!(s.proxy.paid().await.total, MicroUsd(400));
    assert_eq!(*s.gateway.billed.lock().unwrap(), 400);
}
