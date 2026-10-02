//! Regression vectors of data cleaning rules v1 (m6-public-jobs task 6.2): inputs, the kept
//! documents' JSON Lines and the summary. Regenerate with
//! `AC_WRITE_VECTORS=1 cargo test -p ac-worker --test clean_vectors`; the file must not change
//! otherwise: a change of the cleaning (or of the Unicode tables of `unicode-normalization`) is a
//! new rules version.

#![allow(clippy::unwrap_used)] // Test code.

use ac_worker::exec::clean_unit;
use serde_json::{Value, json};

const PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/vectors/public_clean_v1.json"
);

fn inputs() -> Vec<String> {
    [
        "short",
        "  The quick brown fox jumps over the lazy dog near the river bank.  ",
        "The quick brown fox jumps over the lazy dog near the river bank.",
        "The quick brown fox jumps over the lazy dog near the river bank!",
        "Caf\u{65}\u{301} au lait, s'il vous pla\u{ee}t, avec un croissant tr\u{e8}s frais.",
        "Line one of a document\twith a tab.\n\n\n   Line   two   after blank lines.\u{7}",
        "Hash functions map data of any size to values of a fixed size, quickly and repeatably.",
        "数据清洗规则第一版：规范化、去除控制字符、合并空白、按长度过滤、精确去重与近似去重。",
    ]
    .iter()
    .map(|s| (*s).to_string())
    .collect()
}

fn vectors() -> Value {
    let docs = inputs();
    let out = clean_unit(&docs).unwrap();
    json!({
        "rules": 1,
        "inputs": docs,
        "result": String::from_utf8(out.result).unwrap(),
        "summary": hex::encode(&out.summary),
    })
}

#[test]
fn clean_vectors_are_stable() {
    let text = format!("{}\n", serde_json::to_string_pretty(&vectors()).unwrap());
    if std::env::var_os("AC_WRITE_VECTORS").is_some() {
        std::fs::write(PATH, &text).unwrap();
    }
    let stored = std::fs::read_to_string(PATH).unwrap();
    assert_eq!(
        stored, text,
        "cleaning vectors changed; see the module docs"
    );
}
