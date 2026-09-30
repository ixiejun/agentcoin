//! Builds TOPLOC proofs from engine-plugin candidates (used by CI to compare with the
//! reference implementation, `scripts/check-vllm-plugin.py`).
//!
//! Input on stdin (JSON):
//! `{"params": {"decode_batching_size": 32, "topk": 128, "skip_prefill": false},
//!   "segments": [{"phase": "prefill" | "decode", "len": N, "candidates": [[index, bits], …]}, …]}`
//!
//! Output on stdout (JSON): `{"proofs": ["<hex>", …], "commitment": "<hex>"}`.

use std::io::Read;

use ac_toploc::{
    Bf16, Candidate, Params, Phase, Segment, build_proofs_from_candidates, commitment,
};
use serde::Deserialize;

#[derive(Deserialize)]
struct Input {
    params: InputParams,
    segments: Vec<InputSegment>,
}

#[derive(Deserialize)]
struct InputParams {
    decode_batching_size: u32,
    topk: u32,
    skip_prefill: bool,
}

#[derive(Deserialize)]
struct InputSegment {
    phase: String,
    len: u32,
    candidates: Vec<(u32, u16)>,
}

fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Converts the JSON input into the JSON output.
///
/// # Errors
///
/// Malformed JSON, an unknown phase, or candidates that do not prove.
pub fn run(input: &str) -> Result<String, String> {
    let input: Input = serde_json::from_str(input).map_err(|e| format!("bad input: {e}"))?;
    let params = Params {
        decode_batching_size: input.params.decode_batching_size,
        topk: input.params.topk,
        skip_prefill: input.params.skip_prefill,
    };
    let segments = input
        .segments
        .into_iter()
        .map(|s| {
            let phase = match s.phase.as_str() {
                "prefill" => Phase::Prefill,
                "decode" => Phase::Decode,
                other => return Err(format!("unknown phase {other:?}")),
            };
            Ok(Segment {
                phase,
                len: s.len,
                candidates: s
                    .candidates
                    .into_iter()
                    .map(|(index, bits)| Candidate {
                        index,
                        value: Bf16(bits),
                    })
                    .collect(),
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    let proofs = build_proofs_from_candidates(&segments, &params).map_err(|e| e.to_string())?;
    let commit = commitment(&params, &proofs).map_err(|e| e.to_string())?;
    let out = serde_json::json!({
        "proofs": proofs.iter().map(|p| to_hex(&p.to_bytes())).collect::<Vec<_>>(),
        "commitment": to_hex(&commit),
    });
    Ok(out.to_string())
}

#[allow(dead_code)] // Also compiled into tests/candidates_example.rs, which calls only `run`.
fn main() -> Result<(), String> {
    let mut input = String::new();
    std::io::stdin()
        .read_to_string(&mut input)
        .map_err(|e| format!("cannot read stdin: {e}"))?;
    println!("{}", run(&input)?);
    Ok(())
}
