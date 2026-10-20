//! Regression vectors of the receipt content commitment and canonical JSON (m6-token-binding
//! task 2.3), shared with the Python implementation in `scripts/calibrate-toploc.py`. Each case
//! keeps its messages as raw JSON texts (with their original key order and spacing), their
//! canonical texts, a salt, the prompt and output token IDs, the SCALE preimage the commitment
//! hashes and the commitment. (Python checks the canonical texts and the preimage; the hash is
//! `ac-crypto`'s.)
//! Regenerate with `AC_WRITE_VECTORS=1 cargo test -p ac-primitives --test receipt_content_vectors`;
//! the file must not change otherwise.

#![allow(clippy::unwrap_used)] // Test code.

use ac_primitives::market::content::{canonical_json, canonical_messages, content_commit};
use parity_scale_codec::Encode;
use serde_json::{Value, json};

const PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/vectors/receipt_content.json"
);

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// (name, raw messages, salt byte, prompt IDs, output IDs)
type Case = (&'static str, Vec<&'static str>, u8, Vec<u32>, Vec<u32>);

fn cases() -> Vec<Case> {
    vec![
        (
            "one user message",
            vec![r#"{"role": "user", "content": "Write a haiku about a lighthouse."}"#],
            0x01,
            vec![151644, 872, 198, 7985, 264],
            vec![785, 13, 151645],
        ),
        (
            "the same message with its keys in another order",
            vec![r#"{"content":"Write a haiku about a lighthouse.","role":"user"}"#],
            0x01,
            vec![151644, 872, 198, 7985, 264],
            vec![785, 13, 151645],
        ),
        (
            "multi-byte characters",
            vec![
                r#"{"role":"system","content":"你是一个乐于助人的助手。"}"#,
                r#"{"role":"user","content":"Grüße 👋 — ça va? Ελληνικά"}"#,
            ],
            0x02,
            vec![1, 2, 3],
            vec![4, 5, 6, 7],
        ),
        (
            "escaped characters",
            vec![
                r#"{"role":"user","content":"quote \" backslash \\ slash \/ newline \n tab \t cr \r bs \b ff \f nul \u0000 us \u001f del \u007f nbsp  "}"#,
            ],
            0x03,
            vec![10, 11],
            vec![12],
        ),
        (
            "nested objects, integers, booleans and null",
            vec![
                r#"{"role":"user","content":[{"type":"text","text":"What is the weather?"}],"name":"alice"}"#,
                r#"{"role":"assistant","content":null,"tool_calls":[{"id":"call_1","type":"function","function":{"name":"weather","arguments":"{\"city\": \"Paris\"}"},"index":0}],"refusal":false}"#,
                r#"{"role":"tool","tool_call_id":"call_1","content":"18","extra":{"z":-3,"a":[true,null,12345678901234]}}"#,
            ],
            0xa5,
            vec![100, 200, 300, 400],
            vec![500, 600],
        ),
        (
            "no output tokens",
            vec![r#"{"role":"user","content":""}"#],
            0xff,
            vec![0, 4294967295],
            vec![],
        ),
    ]
}

fn vectors() -> Value {
    let cases: Vec<Value> = cases()
        .into_iter()
        .map(|(name, raw, salt, prompt, output)| {
            let parsed: Vec<Value> = raw
                .iter()
                .map(|m| serde_json::from_str(m).unwrap())
                .collect();
            let canonical = canonical_messages(&parsed).unwrap();
            let commit = content_commit(&[salt; 32], &canonical, &prompt, &output).unwrap();
            let preimage = (&[salt; 32], &canonical, &prompt, &output).encode();
            assert_eq!(
                ac_crypto::hash::derive("agentcoin 2026-10 receipt-content v1", &preimage).unwrap(),
                commit
            );
            json!({
                "name": name,
                "messages_json": raw,
                "canonical": canonical
                    .iter()
                    .map(|c| String::from_utf8(c.clone()).unwrap())
                    .collect::<Vec<_>>(),
                "salt": hex(&[salt; 32]),
                "prompt_ids": prompt,
                "output_ids": output,
                "preimage": hex(&preimage),
                "commit": hex(&commit),
            })
        })
        .collect();
    json!({
        "context": "agentcoin 2026-10 receipt-content v1",
        "non_integer_number_is_refused": r#"{"role":"user","content":"x","temperature":0.5}"#,
        "cases": cases,
    })
}

#[test]
fn receipt_content_vectors_are_stable() {
    let text = format!("{}\n", serde_json::to_string_pretty(&vectors()).unwrap());
    if std::env::var_os("AC_WRITE_VECTORS").is_some() {
        std::fs::write(PATH, &text).unwrap();
    }
    let stored = std::fs::read_to_string(PATH).unwrap();
    assert_eq!(
        stored, text,
        "receipt content vectors changed; see the module docs"
    );
}

#[test]
fn key_order_and_spacing_do_not_change_the_commitment() {
    let v = vectors();
    let cases = v["cases"].as_array().unwrap();
    assert_eq!(cases[0]["canonical"], cases[1]["canonical"]);
    assert_eq!(cases[0]["commit"], cases[1]["commit"]);
    assert_eq!(
        cases[1]["canonical"][0], cases[1]["messages_json"][0],
        "an already canonical text is unchanged"
    );
}

#[test]
fn non_integer_numbers_are_refused() {
    let raw = vectors()["non_integer_number_is_refused"]
        .as_str()
        .unwrap()
        .to_owned();
    let value: Value = serde_json::from_str(&raw).unwrap();
    assert!(canonical_json(&value).is_err());
}
