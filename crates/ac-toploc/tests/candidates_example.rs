//! The `toploc_from_candidates` example gives the proofs and commitment of `build_proofs`.

#![allow(clippy::unwrap_used, clippy::indexing_slicing)] // Test code.

#[path = "../examples/toploc_from_candidates.rs"]
mod example;

use ac_toploc::{Bf16, Params, build_proofs, commitment, top_k_candidates};

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn values(seed: u16, n: u16) -> Vec<Bf16> {
    (0..n)
        .map(|i| Bf16(i.wrapping_mul(7919).wrapping_add(seed) % 0x7f00))
        .collect()
}

fn segment(phase: &str, v: &[Bf16], k: usize) -> serde_json::Value {
    let c: Vec<(u32, u16)> = top_k_candidates(v, k)
        .iter()
        .map(|c| (c.index, c.value.0))
        .collect();
    serde_json::json!({"phase": phase, "len": v.len(), "candidates": c})
}

#[test]
fn example_output_matches_build_proofs() {
    let (pre1, pre2) = (values(1, 40), values(2, 24));
    let prefill: Vec<Bf16> = pre1.iter().chain(&pre2).copied().collect();
    let decode: Vec<Vec<Bf16>> = (3..8).map(|s| values(s, 16)).collect();
    let params = Params {
        decode_batching_size: 2,
        topk: 8,
        skip_prefill: false,
    };
    let mut acts: Vec<&[Bf16]> = vec![&prefill];
    acts.extend(decode.iter().map(Vec::as_slice));
    let proofs = build_proofs(&acts, &params).unwrap();

    let mut segs = vec![segment("prefill", &pre1, 8), segment("prefill", &pre2, 8)];
    segs.extend(decode.iter().map(|d| segment("decode", d, 8)));
    let input = serde_json::json!({
        "params": {"decode_batching_size": 2, "topk": 8, "skip_prefill": false},
        "segments": segs,
    });
    let out: serde_json::Value =
        serde_json::from_str(&example::run(&input.to_string()).unwrap()).unwrap();
    let expected: Vec<String> = proofs.iter().map(|p| hex(&p.to_bytes())).collect();
    assert_eq!(out["proofs"], serde_json::json!(expected));
    assert_eq!(
        out["commitment"],
        serde_json::json!(hex(&commitment(&params, &proofs).unwrap()))
    );
}

// Whole activations given as `values` give the proofs of `build_proofs`, ties included (lower
// index first, the order the vLLM plugin uses).
#[test]
fn example_accepts_whole_activations() {
    // Heavy ties: values from a grid of 7 magnitudes.
    let tied = |seed: u16, n: u16| -> Vec<Bf16> {
        (0..n)
            .map(|i| Bf16(0x3f80 + 0x80 * (i.wrapping_mul(31).wrapping_add(seed) % 7)))
            .collect()
    };
    let (prefill, decode) = (tied(1, 64), [tied(2, 16), tied(3, 16), tied(4, 16)]);
    let params = Params {
        decode_batching_size: 2,
        topk: 8,
        skip_prefill: false,
    };
    let mut acts: Vec<&[Bf16]> = vec![&prefill];
    acts.extend(decode.iter().map(Vec::as_slice));
    let proofs = build_proofs(&acts, &params).unwrap();
    let whole = |phase: &str, v: &[Bf16]| serde_json::json!({"phase": phase, "values": v.iter().map(|b| b.0).collect::<Vec<_>>()});
    let mut segs = vec![
        whole("prefill", &prefill[..40]),
        whole("prefill", &prefill[40..]),
    ];
    segs.extend(decode.iter().map(|d| whole("decode", d)));
    let input = serde_json::json!({
        "params": {"decode_batching_size": 2, "topk": 8, "skip_prefill": false},
        "segments": segs,
    });
    let out: serde_json::Value =
        serde_json::from_str(&example::run(&input.to_string()).unwrap()).unwrap();
    let expected: Vec<String> = proofs.iter().map(|p| hex(&p.to_bytes())).collect();
    assert_eq!(out["proofs"], serde_json::json!(expected));
}

#[test]
fn example_rejects_bad_input() {
    assert!(example::run("{").is_err());
    let input = r#"{"params": {"decode_batching_size": 1, "topk": 1, "skip_prefill": false},
        "segments": [{"phase": "middle", "len": 1, "candidates": [[0, 16256]]}]}"#;
    assert!(example::run(input).unwrap_err().contains("unknown phase"));
}
