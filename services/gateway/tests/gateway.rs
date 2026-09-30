//! Spec `market/gateway-service` against a simulated chain, real provider agents and mock
//! engines (m5-gateway-provider 5.1-5.4, 5.6).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ac_crypto::kem::KemSecretKey;
use ac_crypto::sig::{SecretSeed, SigningKey};
use ac_crypto::{KemAlg, KemPublicKey, OsRng, PqPublicKey, SigAlg, account_id};
use ac_gateway::chain::Chain;
use ac_gateway::logging::{self, Sink};
use ac_gateway::routing::Router;
use ac_gateway::{Config, Gateway};
use ac_market_proto::{ErrorCode, GatewayKey, GatewayMsg, Payment, UserMsg, msg};
use ac_mock_engine::Engine;
use ac_primitives::market::receipt::{ReceiptContext, check_receipt};
use ac_primitives::market::records::{ModelPrice, ProviderStatus, SlaMetrics, Tier};
use ac_primitives::market::voucher::{
    ChannelView, VOUCHER_CONTEXT, VoucherContext, check_voucher, key_fingerprint,
};
use ac_primitives::market::{
    AtcPerUsd, MicroUsd, ModelId, PricePerMTok, SignedVoucher, VoucherBody, VoucherCheck,
    VoucherError,
};
use ac_provider::chain::{GatewayView, StaticDirectory};
use ac_wallet::http::{Client, join};
use ac_wallet::market::{Channel, Provider};
use ac_wallet::sealed_http;
use anyhow::Result;
use async_trait::async_trait;
use sp_core::H256;
use sp_runtime::{AccountId32, BoundedVec};

const GENESIS: H256 = H256([7; 32]);
const MODEL: ModelId = ModelId([1; 32]);
const LONELY: ModelId = ModelId([2; 32]);
const ATC: u128 = 1_000_000_000_000_000_000;
const MARKER: &str = "okapi-marker-91d3";
const CHEAP: PricePerMTok = PricePerMTok {
    input: MicroUsd(100_000),
    output: MicroUsd(200_000),
};
const DEAR: PricePerMTok = PricePerMTok {
    input: MicroUsd(300_000),
    output: MicroUsd(600_000),
};

static LOGS: Mutex<Vec<String>> = Mutex::new(Vec::new());

struct Party {
    account: AccountId32,
    key: SigningKey,
}

fn party(seed: u8) -> Party {
    let key = SigningKey::from_seed(SigAlg::MlDsa44, &SecretSeed::new([seed; 32])).unwrap();
    Party {
        account: AccountId32::new(*account_id(&key.public_key().unwrap()).as_bytes()),
        key,
    }
}

/// The simulated chain.
#[derive(Default)]
struct TestChain {
    keys: Mutex<HashMap<AccountId32, PqPublicKey>>,
    channels: Mutex<HashMap<AccountId32, Channel>>,
    providers: Mutex<BTreeMap<ModelId, Vec<(AccountId32, Provider)>>>,
    gateway: Mutex<Option<AccountId32>>,
}

#[async_trait]
impl Chain for TestChain {
    async fn current_key(&self, who: &AccountId32) -> Result<Option<PqPublicKey>> {
        Ok(self.keys.lock().unwrap().get(who).cloned())
    }
    async fn channel(&self, user: &AccountId32, _gateway: &AccountId32) -> Result<Option<Channel>> {
        Ok(self.channels.lock().unwrap().get(user).cloned())
    }
    async fn rate(&self) -> Result<Option<AtcPerUsd>> {
        Ok(Some(AtcPerUsd(ATC)))
    }
    async fn check_voucher(&self, v: &SignedVoucher) -> Result<Result<VoucherCheck, VoucherError>> {
        let gateway = self.gateway.lock().unwrap().clone().unwrap();
        let view = self
            .channels
            .lock()
            .unwrap()
            .get(&v.body.user)
            .map(|c| ChannelView {
                escrow: c.escrow,
                number: c.number,
                redeemed: c.redeemed,
                key: c.key,
            });
        Ok(check_voucher(
            v,
            &VoucherContext {
                genesis: &GENESIS,
                gateway: &gateway,
                channel: view.as_ref(),
                rate: Some(AtcPerUsd(ATC)),
            },
        ))
    }
    async fn model_ids(&self) -> Result<Vec<ModelId>> {
        Ok(vec![MODEL, LONELY])
    }
    async fn model_name(&self, id: ModelId) -> Result<Option<String>> {
        Ok(Some(if id == MODEL {
            "Qwen-test".into()
        } else {
            "Lonely".into()
        }))
    }
    async fn serviceable(&self, model: ModelId) -> Result<Vec<(AccountId32, Provider)>> {
        Ok(self
            .providers
            .lock()
            .unwrap()
            .get(&model)
            .cloned()
            .unwrap_or_default())
    }
}

fn record(endpoint: &str, kem_pk: KemPublicKey, price: PricePerMTok) -> Provider {
    Provider {
        tier: Tier::T2,
        endpoint: BoundedVec::truncate_from(endpoint.as_bytes().to_vec()),
        kem_pk,
        models: BoundedVec::truncate_from(vec![ModelPrice {
            model: MODEL,
            price,
        }]),
        stake: 0,
        unlocking: BoundedVec::default(),
        status: ProviderStatus::Active,
        last_heartbeat: 0,
        metrics: SlaMetrics::default(),
        attestation: None,
        registered_at: 0,
    }
}

struct ProviderNode {
    party: Party,
    url: String,
    kem: KemPublicKey,
    engine: Engine,
    service: Arc<ac_provider::Service>,
}

/// A real provider agent in front of a mock engine; `local_price` is what it charges.
async fn provider(
    seed: u8,
    gateway: &Party,
    engine: ac_mock_engine::Config,
    local_price: PricePerMTok,
) -> ProviderNode {
    let engine = ac_mock_engine::spawn("127.0.0.1:0", engine).await.unwrap();
    let party = party(seed);
    let kem_secret = KemSecretKey::from_seed(
        KemAlg::XWing,
        &SecretSeed::new([seed.wrapping_add(100); 32]),
    )
    .unwrap();
    let kem = kem_secret.public_key().unwrap();
    let dir = HashMap::from([(
        gateway.account.clone(),
        GatewayView {
            key: gateway.key.public_key().unwrap(),
            active: true,
        },
    )]);
    let service = ac_provider::Service::new(
        ac_provider::Config {
            account: party.account.clone(),
            key: party.key.clone(),
            kem: kem_secret,
            kem_public: kem.clone(),
            genesis: GENESIS,
            models: BTreeMap::from([(
                MODEL,
                ac_provider::ModelEntry {
                    engine_name: "mock-model".into(),
                    price: local_price,
                },
            )]),
            engine: engine.url(),
            store_dir: temp(&format!("provider-{seed}")),
        },
        Box::new(StaticDirectory(dir)),
    )
    .unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let svc = Arc::clone(&service);
    tokio::spawn(ac_wallet::http::serve(listener, move |req| {
        Arc::clone(&svc).handle(req)
    }));
    ProviderNode {
        party,
        url,
        kem,
        engine,
        service,
    }
}

fn temp(label: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("ac-gateway-it-{label}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    d
}

struct Net {
    chain: Arc<TestChain>,
    router: Arc<Router>,
    gateway: Arc<Gateway>,
    gw: Party,
    url: String,
    kem: KemPublicKey,
    user: Party,
    data: PathBuf,
}

async fn net(label: &str, escrow: u128, providers: &[(&ProviderNode, PricePerMTok)]) -> Net {
    logging::init(log::LevelFilter::Debug, Sink::Buffer(&LOGS));
    let (gw, user) = (party(1), party(2));
    let chain = Arc::new(TestChain::default());
    *chain.gateway.lock().unwrap() = Some(gw.account.clone());
    for p in [&gw, &user] {
        chain
            .keys
            .lock()
            .unwrap()
            .insert(p.account.clone(), p.key.public_key().unwrap());
    }
    for (p, _) in providers {
        chain
            .keys
            .lock()
            .unwrap()
            .insert(p.party.account.clone(), p.party.key.public_key().unwrap());
    }
    chain.providers.lock().unwrap().insert(
        MODEL,
        providers
            .iter()
            .map(|(p, price)| {
                (
                    p.party.account.clone(),
                    record(&p.url, p.kem.clone(), *price),
                )
            })
            .collect(),
    );
    chain.channels.lock().unwrap().insert(
        user.account.clone(),
        Channel {
            escrow,
            number: 0,
            redeemed: MicroUsd::ZERO,
            key: key_fingerprint(&user.key.public_key().unwrap()),
            pending_withdrawal: None,
            pending_key: None,
        },
    );
    let router = Arc::new(Router::new());
    router.refresh(chain.as_ref()).await.unwrap();
    let kem_secret = KemSecretKey::from_seed(KemAlg::XWing, &SecretSeed::new([3; 32])).unwrap();
    let kem = kem_secret.public_key().unwrap();
    let data = temp(label);
    let gateway = Gateway::new(
        Config {
            account: gw.account.clone(),
            key: gw.key.clone(),
            kem: kem_secret,
            kem_public: kem.clone(),
            genesis: GENESIS,
            max_output_tokens: 64,
            data_dir: data.clone(),
        },
        Arc::clone(&chain) as Arc<dyn Chain>,
        Arc::clone(&router),
    )
    .unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let g = Arc::clone(&gateway);
    tokio::spawn(ac_wallet::http::serve(listener, move |req| {
        Arc::clone(&g).handle(req)
    }));
    Net {
        chain,
        router,
        gateway,
        gw,
        url,
        kem,
        user,
        data,
    }
}

impl Net {
    fn voucher(&self, cumulative: u128) -> SignedVoucher {
        let body = VoucherBody {
            genesis: GENESIS,
            user: self.user.account.clone(),
            gateway: self.gw.account.clone(),
            channel: 0,
            cumulative: MicroUsd(cumulative),
        };
        SignedVoucher {
            signature: self
                .user
                .key
                .sign(
                    &body.payload().unwrap(),
                    VOUCHER_CONTEXT,
                    &mut OsRng::new().unwrap(),
                )
                .unwrap(),
            public_key: self.user.key.public_key().unwrap(),
            body,
        }
    }

    async fn send(&self, m: &UserMsg) -> Vec<GatewayMsg> {
        let mut resp = sealed_http::post(
            &Client::new().unwrap(),
            &self.url,
            &self.kem,
            (&self.user.account, &self.user.key),
            &msg::encode(m),
        )
        .await
        .unwrap();
        let mut out = Vec::new();
        while let Some(b) = resp.next().await.unwrap() {
            out.push(msg::decode(&b).unwrap());
        }
        out
    }

    async fn chat(&self, model: &str, prompt: &str, stream: bool, paid: u128) -> Vec<GatewayMsg> {
        let request = serde_json::json!({
            "model": model,
            "messages": [{ "role": "user", "content": prompt }],
            "max_tokens": 6,
            "stream": stream,
        });
        self.send(&UserMsg::Chat {
            request: request.to_string().into_bytes(),
            payment: Payment::Transparent(self.voucher(paid)),
        })
        .await
    }

    async fn pay(&self, cumulative: u128) -> Vec<GatewayMsg> {
        self.send(&UserMsg::Pay {
            payment: Payment::Transparent(self.voucher(cumulative)),
        })
        .await
    }
}

fn fast() -> ac_mock_engine::Config {
    ac_mock_engine::Config {
        ttft: Duration::ZERO,
        token_interval: Duration::ZERO,
        ..Default::default()
    }
}

fn billing(reply: &[GatewayMsg]) -> (ac_primitives::market::SignedReceipt, MicroUsd, MicroUsd) {
    match reply.last() {
        Some(GatewayMsg::Billing {
            receipt,
            fee,
            billed_total,
            ..
        }) => (receipt.clone(), *fee, *billed_total),
        other => panic!("no billing: {other:?}"),
    }
}

fn error_code(reply: &[GatewayMsg]) -> ErrorCode {
    match reply.last() {
        Some(GatewayMsg::Error { code, .. }) => *code,
        other => panic!("no error: {other:?}"),
    }
}

// Scenarios "客户端核对网关公钥" and "模型列表只含可服务模型".
#[tokio::test]
async fn key_and_model_list() {
    let gw = party(1);
    let p = provider(10, &gw, fast(), CHEAP).await;
    let n = net("key", ATC, &[(&p, CHEAP)]).await;
    let client = Client::new().unwrap();
    let ann = GatewayKey::from_json(
        &client
            .get_bytes(&join(&n.url, "/ac/v1/key"), 65_536)
            .await
            .unwrap(),
    )
    .unwrap();
    ann.verify(GENESIS, &n.gw.account, &n.gw.key.public_key().unwrap())
        .unwrap();
    assert!(
        ann.verify(GENESIS, &n.gw.account, &n.user.key.public_key().unwrap())
            .is_err()
    );
    assert_eq!(ann.kem_key, n.kem);
    let models: serde_json::Value = serde_json::from_slice(
        &client
            .get_bytes(&join(&n.url, "/ac/v1/models"), 65_536)
            .await
            .unwrap(),
    )
    .unwrap();
    let ids: Vec<&str> = models["data"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, [format!("0x{}", hex::encode(MODEL.0))]);
    // Every provider stops being serviceable: after a refresh the model is gone.
    n.chain.providers.lock().unwrap().clear();
    n.router.refresh(n.chain.as_ref()).await.unwrap();
    let models: serde_json::Value = serde_json::from_slice(
        &client
            .get_bytes(&join(&n.url, "/ac/v1/models"), 65_536)
            .await
            .unwrap(),
    )
    .unwrap();
    assert!(models["data"].as_array().unwrap().is_empty());
}

// Scenarios "流式请求", "用户拿到双签收据", "未付清上一请求", "多付的凭证被拒绝"; the provider
// keeps the co-signed receipt; non-streamed requests get one completion.
#[tokio::test]
async fn postpaid_requests_with_double_signed_receipts() {
    let gw = party(1);
    let p = provider(11, &gw, fast(), CHEAP).await;
    let n = net("postpaid", ATC, &[(&p, CHEAP)]).await;
    let reply = n.chat("Qwen-test", "one two three", true, 0).await;
    assert!(
        reply
            .iter()
            .any(|m| matches!(m, GatewayMsg::Delta(d) if ac_market_proto::openai::has_content(d)))
    );
    let (receipt, fee, billed) = billing(&reply);
    assert_eq!(fee, receipt.body.fee);
    assert_eq!(billed, fee);
    // 3 prompt words + 6 output tokens at $0.1 / $0.2 per million, rounded up.
    assert_eq!(
        fee,
        ac_primitives::market::receipt::fee_for(&CHEAP, 3, 6).unwrap()
    );
    check_receipt(
        &receipt,
        &ReceiptContext {
            genesis: &GENESIS,
            provider_key: &key_fingerprint(&p.party.key.public_key().unwrap()),
            gateway_key: &key_fingerprint(&n.gw.key.public_key().unwrap()),
            price: Some(&CHEAP),
        },
    )
    .unwrap();

    // Not paid yet: refused, and the engine sees nothing.
    let before = p.engine.requests();
    assert_eq!(
        error_code(&n.chat("Qwen-test", "again", true, 0).await),
        ErrorCode::PaymentRequired
    );
    assert_eq!(p.engine.requests(), before);
    // Overpaying is refused; paying exactly is accepted.
    assert_eq!(
        error_code(&n.pay(fee.0 + 1).await),
        ErrorCode::PaymentRequired
    );
    assert!(
        matches!(n.pay(fee.0).await.last(), Some(GatewayMsg::Paid { billed_total }) if *billed_total == fee)
    );
    // Non-streamed: one completion, then the bill.
    let reply = n
        .chat(
            &format!("0x{}", hex::encode(MODEL.0)),
            "four five",
            false,
            fee.0,
        )
        .await;
    assert!(
        matches!(
            &reply[..],
            [GatewayMsg::Completion(_), GatewayMsg::Billing { .. }]
        ),
        "{reply:?}"
    );
    let GatewayMsg::Completion(c) = &reply[0] else {
        panic!("no completion")
    };
    let c: serde_json::Value = serde_json::from_slice(c).unwrap();
    assert_eq!(c["usage"]["completion_tokens"], 6);
    // The provider got its co-signed receipts back.
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(p.service.store().list().unwrap().len(), 2);
}

// Scenario "托管不足": the provider never sees the request.
#[tokio::test]
async fn escrow_must_cover_the_maximum_fee() {
    let gw = party(1);
    let p = provider(12, &gw, fast(), CHEAP).await;
    let n = net("escrow", 1, &[(&p, CHEAP)]).await;
    assert_eq!(
        error_code(&n.chat("Qwen-test", "hi", true, 0).await),
        ErrorCode::PaymentRequired
    );
    assert_eq!(p.engine.requests(), 0);
}

// Scenarios "名称不唯一" (single model here: unknown names and IDs) and "首 token 前切换",
// "不可服务的提供者不被选中", plus a failure after the first token is not billed.
#[tokio::test]
async fn routing_and_failover() {
    let gw = party(1);
    let cheap = provider(13, &gw, fast(), CHEAP).await;
    let dear = provider(14, &gw, fast(), DEAR).await;
    let n = net("route", ATC, &[(&cheap, CHEAP), (&dear, DEAR)]).await;
    assert_eq!(
        error_code(&n.chat("nope", "hi", true, 0).await),
        ErrorCode::ModelNotFound
    );
    assert_eq!(
        error_code(&n.chat("Lonely", "hi", true, 0).await),
        ErrorCode::ModelNotFound
    );
    // The cheapest provider serves.
    let (r, fee, _) = billing(&n.chat("Qwen-test", "hi", true, 0).await);
    assert_eq!(r.body.provider, cheap.party.account);
    n.pay(fee.0).await;
    // It becomes unreachable (its endpoint is a closed port): the next one serves, at its price.
    {
        let mut providers = n.chain.providers.lock().unwrap();
        for (who, rec) in providers.get_mut(&MODEL).unwrap().iter_mut() {
            if *who == cheap.party.account {
                rec.endpoint = BoundedVec::truncate_from(b"http://127.0.0.1:9".to_vec());
            }
        }
    }
    n.router.refresh(n.chain.as_ref()).await.unwrap();
    let (r2, fee2, billed) = billing(&n.chat("Qwen-test", "hi", true, fee.0).await);
    assert_eq!(r2.body.provider, dear.party.account);
    assert_eq!(
        fee2,
        ac_primitives::market::receipt::fee_for(&DEAR, 1, 6).unwrap()
    );
    assert_eq!(billed.0, fee.0 + fee2.0);
    n.pay(billed.0).await;
    // Unserviceable providers are not listed by the chain, so they are never chosen.
    n.chain
        .providers
        .lock()
        .unwrap()
        .get_mut(&MODEL)
        .unwrap()
        .retain(|(w, _)| *w == cheap.party.account);
    n.router.refresh(n.chain.as_ref()).await.unwrap();
    // (The remaining provider is paused after failing, so nobody is left.)
    let before = dear.engine.requests();
    assert_eq!(
        error_code(&n.chat("Qwen-test", "hi", true, billed.0).await),
        ErrorCode::NoProvider
    );
    assert_eq!(dear.engine.requests(), before);
}

#[tokio::test]
async fn failure_after_the_first_token_is_not_billed() {
    let gw = party(1);
    let flaky = provider(
        15,
        &gw,
        ac_mock_engine::Config {
            fail_after: Some(2),
            ..fast()
        },
        CHEAP,
    )
    .await;
    let n = net("flaky", ATC, &[(&flaky, CHEAP)]).await;
    let reply = n.chat("Qwen-test", "one two three", true, 0).await;
    assert!(reply.iter().any(|m| matches!(m, GatewayMsg::Delta(_))));
    assert_eq!(error_code(&reply), ErrorCode::ProviderFailed);
    assert_eq!(
        n.gateway.book().get(&n.user.account).unwrap().billed,
        MicroUsd::ZERO
    );
    assert!(
        n.gateway
            .book()
            .get(&n.user.account)
            .unwrap()
            .inflight
            .is_empty()
    );
}

// Scenario "提供者多报费用": the provider charges more than its on-chain price.
#[tokio::test]
async fn overcharging_providers_are_not_billed_and_are_paused() {
    let gw = party(1);
    let greedy = provider(16, &gw, fast(), DEAR).await;
    let n = net("greedy", ATC, &[(&greedy, CHEAP)]).await;
    assert_eq!(
        error_code(&n.chat("Qwen-test", "hi", true, 0).await),
        ErrorCode::ProviderFailed
    );
    assert_eq!(
        n.gateway.book().get(&n.user.account).unwrap().billed,
        MicroUsd::ZERO
    );
    // Paused: the next request does not reach it.
    let before = greedy.engine.requests();
    assert_eq!(
        error_code(&n.chat("Qwen-test", "hi", true, 0).await),
        ErrorCode::NoProvider
    );
    assert_eq!(greedy.engine.requests(), before);
}

// Scenario "日志中没有 prompt": at debug level nothing of the request or the output (which echoes
// it) is in the logs or the data directory.
#[tokio::test]
async fn no_request_content_is_logged_or_stored() {
    let gw = party(1);
    let p = provider(17, &gw, fast(), CHEAP).await;
    let n = net("privacy", ATC, &[(&p, CHEAP)]).await;
    let reply = n
        .chat("Qwen-test", &format!("repeat {MARKER}"), true, 0)
        .await;
    assert!(
        reply.iter().any(
            |m| matches!(m, GatewayMsg::Delta(d) if String::from_utf8_lossy(d).contains(MARKER))
        )
    );
    let (_, fee, _) = billing(&reply);
    n.pay(fee.0).await;
    let logs = LOGS.lock().unwrap().join("\n");
    assert!(logs.contains("request"), "{logs}");
    assert!(!logs.contains(MARKER));
    let mut stack = vec![n.data.clone()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
            if e.path().is_dir() {
                stack.push(e.path());
            } else {
                assert!(
                    !String::from_utf8_lossy(&std::fs::read(e.path()).unwrap()).contains(MARKER)
                );
            }
        }
    }
}
