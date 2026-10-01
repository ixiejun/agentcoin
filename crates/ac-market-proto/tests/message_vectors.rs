//! Regression vectors of the user → gateway messages and of the audit evidence exchange
//! (m6-auditor-agent tasks 1.1 and 1.2). Regenerate with `AC_WRITE_VECTORS=1 cargo test -p
//! ac-market-proto --test message_vectors`; the file must not change otherwise.

#![allow(clippy::unwrap_used)] // Test code.

use ac_market_proto::audit::{EvidenceRequest, EvidenceResponse};
use ac_market_proto::msg::{self, Payment, UserMsg};
use ac_primitives::market::voucher::VOUCHER_CONTEXT;
use ac_primitives::market::{MicroUsd, SignedVoucher, VoucherBody};
use parity_scale_codec::Encode;
use serde_json::{Value, json};
use sp_core::H256;
use sp_runtime::AccountId32;

const PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/vectors/messages.json");

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn payment() -> Payment {
    let key = ac_crypto::sig::SigningKey::from_seed(
        ac_crypto::SigAlg::MlDsa44,
        &ac_crypto::dev_seed("vector-voucher").unwrap(),
    )
    .unwrap();
    let body = VoucherBody {
        genesis: H256([0x11; 32]),
        user: AccountId32::new([0x77; 32]),
        gateway: AccountId32::new([0x22; 32]),
        channel: 3,
        cumulative: MicroUsd(1_234),
    };
    Payment::Transparent(SignedVoucher {
        signature: key
            .sign_deterministic(&body.payload().unwrap(), VOUCHER_CONTEXT)
            .unwrap(),
        public_key: key.public_key().unwrap(),
        body,
    })
}

fn vectors() -> Value {
    let request = br#"{"model":"qwen","messages":[{"role":"user","content":"hi"}]}"#.to_vec();
    let user = |name: &str, m: UserMsg| json!({"name": name, "encoding": hex(&msg::encode(&m))});
    let ev = |name: &str, bytes: Vec<u8>| json!({"name": name, "encoding": hex(&bytes)});
    json!({
        "user": [
            user("chat", UserMsg::Chat { request: request.clone(), payment: payment() }),
            user("pay", UserMsg::Pay { payment: payment() }),
            user("chat to provider", UserMsg::ChatTo {
                provider: AccountId32::new([0x33; 32]),
                request,
                payment: payment(),
            }),
        ],
        "evidence": [
            ev("request", EvidenceRequest { dispute: 9, commitment: [0xab; 32] }.encode()),
            ev("evidence", EvidenceResponse::Evidence(vec![1, 2, 3]).encode()),
            ev("refused", EvidenceResponse::Refused.encode()),
        ],
    })
}

#[test]
fn message_vectors_are_stable() {
    let text = format!("{}\n", serde_json::to_string_pretty(&vectors()).unwrap());
    if std::env::var_os("AC_WRITE_VECTORS").is_some() {
        std::fs::write(PATH, &text).unwrap();
    }
    let stored = std::fs::read_to_string(PATH).unwrap();
    assert_eq!(stored, text, "message vectors changed; see the module docs");
}

#[test]
fn chat_to_takes_index_two_and_existing_indices_stay() {
    let v = vectors();
    let prefix = |i: usize| v["user"][i]["encoding"].as_str().unwrap()[..4].to_string();
    assert_eq!([prefix(0), prefix(1), prefix(2)], ["0200", "0201", "0202"]);
    assert_eq!(&v["evidence"][2]["encoding"], "01");
}
