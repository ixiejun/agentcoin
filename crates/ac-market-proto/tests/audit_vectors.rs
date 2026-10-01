//! Regression vectors of audit evidence and of the on-chain verdict encoding (m6-audit-chain
//! design D6; spec `market/audit` "审计证据与承诺"). Regenerate with `AC_WRITE_VECTORS=1 cargo
//! test -p ac-market-proto --test audit_vectors`; the file must not change otherwise.

#![allow(clippy::unwrap_used, clippy::indexing_slicing)] // Test code.

use ac_market_proto::audit::{AuditEvidence, EVIDENCE_VERSION, EvidenceUsage};
use ac_market_proto::toploc::ToplocProofs;
use ac_primitives::market::audit::{
    AuditMetric, FailReason, InconclusiveReason, VerdictOutcome, Vote,
};
use ac_primitives::market::receipt::RECEIPT_CONTEXT;
use ac_primitives::market::work::JobKind;
use ac_primitives::market::{MicroUsd, ModelId, ReceiptBody, SignedReceipt};
use parity_scale_codec::Encode;
use serde_json::{Value, json};
use sp_core::H256;
use sp_runtime::AccountId32;

const PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/vectors/audit_evidence.json"
);

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn evidence(proofs: bool) -> AuditEvidence {
    let key = |n: &str| {
        ac_crypto::sig::SigningKey::from_seed(
            ac_crypto::SigAlg::MlDsa44,
            &ac_crypto::dev_seed(n).unwrap(),
        )
        .unwrap()
    };
    let (p, g) = (key("vector-provider"), key("vector-gateway"));
    let body = ReceiptBody {
        genesis: H256([0x11; 32]),
        gateway: AccountId32::new([0x22; 32]),
        provider: AccountId32::new([0x33; 32]),
        kind: JobKind::Inference,
        model: ModelId([0x44; 32]),
        request_id: [0x55; 32],
        in_tokens: 21,
        out_tokens: 40,
        fee: MicroUsd(17),
        toploc_commit: if proofs { [0x66; 32] } else { [0; 32] },
        ttft_ms: 85,
        total_ms: 910,
    };
    let payload = body.payload().unwrap();
    let receipt = SignedReceipt {
        provider_sig: p.sign_deterministic(&payload, RECEIPT_CONTEXT).unwrap(),
        provider_key: p.public_key().unwrap(),
        gateway_sig: g.sign_deterministic(&payload, RECEIPT_CONTEXT).unwrap(),
        gateway_key: g.public_key().unwrap(),
        body,
    };
    AuditEvidence {
        version: EVIDENCE_VERSION,
        messages: br#"[{"role":"user","content":"Write a poem about tides."}]"#.to_vec(),
        output: "The moon pulls softly, \u{1f30a} and returns."
            .as_bytes()
            .to_vec(),
        finish_reason: b"stop".to_vec(),
        usage: EvidenceUsage {
            prompt_tokens: 21,
            completion_tokens: 40,
        },
        receipt,
        proofs: proofs.then(|| ToplocProofs {
            decode_batching_size: 32,
            topk: 128,
            skip_prefill: false,
            proofs: vec![(0..258u16).map(|i| (i % 251) as u8).collect(); 3],
        }),
    }
}

fn vectors() -> Value {
    let ev = |name: &str, e: AuditEvidence| {
        json!({
            "name": name,
            "evidence": hex(&e.to_bytes()),
            "commitment": hex(&e.commitment().unwrap()),
        })
    };
    let outcome =
        |name: &str, o: VerdictOutcome| json!({"name": name, "encoding": hex(&o.encode())});
    json!({
        "evidence": [ev("with proofs", evidence(true)), ev("without proofs", evidence(false))],
        "outcomes": [
            outcome("pass", VerdictOutcome::Pass),
            outcome("fail no proof", VerdictOutcome::Fail(FailReason::NoProof)),
            outcome("fail commitment", VerdictOutcome::Fail(FailReason::CommitmentMismatch)),
            outcome("fail params or chunks", VerdictOutcome::Fail(FailReason::ParamsOrChunks)),
            outcome(
                "fail chunk 3 median",
                VerdictOutcome::Fail(FailReason::Threshold { chunk: 3, metric: AuditMetric::MantissaMedian }),
            ),
            outcome("inconclusive tokens", VerdictOutcome::Inconclusive(InconclusiveReason::Tokens)),
            outcome("inconclusive precision", VerdictOutcome::Inconclusive(InconclusiveReason::Precision)),
        ],
        "votes": [
            {"name": "confirm", "encoding": hex(&Vote::Confirm.encode())},
            {"name": "reject", "encoding": hex(&Vote::Reject.encode())},
        ],
    })
}

#[test]
fn audit_vectors_are_stable() {
    let text = format!("{}\n", serde_json::to_string_pretty(&vectors()).unwrap());
    if std::env::var_os("AC_WRITE_VECTORS").is_some() {
        std::fs::write(PATH, &text).unwrap();
    }
    let stored = std::fs::read_to_string(PATH).unwrap();
    assert_eq!(stored, text, "audit vectors changed; see the module docs");
}

#[test]
fn stored_evidence_opens_with_its_commitment() {
    let stored: Value = serde_json::from_str(&std::fs::read_to_string(PATH).unwrap()).unwrap();
    for v in stored["evidence"].as_array().unwrap() {
        let bytes = decode_hex(v["evidence"].as_str().unwrap());
        let c: [u8; 32] = decode_hex(v["commitment"].as_str().unwrap())
            .try_into()
            .unwrap();
        AuditEvidence::open(&bytes, &c).unwrap();
    }
}

fn decode_hex(s: &str) -> Vec<u8> {
    hex::decode(s).unwrap()
}
