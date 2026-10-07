//! m6-toploc-verify 6.1–6.3: re-checks against the mock engine (spec `market/auditor-agent`).
//! A prove-mode mock engine answers a chat request and its plugin's candidates become a case
//! (`calibration::case_from`); a verify-mode mock engine re-checks it. The same model seed plays
//! an honest provider, another seed another model.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    missing_docs
)]

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use ac_auditor::calibration::{CalibrationInput, InputSegment, case_from};
use ac_auditor::case::{CaseUsage, FailReason, Inconclusive, Outcome};
use ac_auditor::logging::{self, Sink};
use ac_auditor::{EngineClient, RecheckCase, Rows, Verifier};
use ac_market_proto::engine::{
    ENGINE_PROTOCOL_VERSION, EngineMode, EngineMsg, EngineReader, request_id_header,
};
use ac_market_proto::toploc::Metric;
use ac_mock_engine::{Config, PluginConfig, spawn};
use ac_primitives::market::model::QuantType;
use ac_toploc::Phase;
use ac_wallet::http::{Client, join, read_body};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

static LOGS: Mutex<Vec<String>> = Mutex::new(Vec::new());
const MODEL: &str = "0x0707070707070707070707070707070707070707070707070707070707070707";
const MARKER: &str = "wombat-marker-3c9f";

fn dir(label: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("ac-auditor-{label}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A prove-mode receiver: the segments of each request, up to its end marker, as the provider
/// would get them.
async fn prove_receiver(
    path: &Path,
) -> tokio::sync::mpsc::UnboundedReceiver<Vec<(Phase, u32, Vec<(u32, u16)>)>> {
    let listener = tokio::net::UnixListener::bind(path).unwrap();
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    tokio::spawn(async move {
        let (mut s, _) = listener.accept().await.unwrap();
        let mut reader = EngineReader::new();
        let mut buf = vec![0u8; 1 << 16];
        let (mut greeted, mut current) = (false, Vec::new());
        loop {
            let n = s.read(&mut buf).await.unwrap();
            if n == 0 {
                return;
            }
            reader.push(&buf[..n]);
            while let Some(m) = reader.next_msg().unwrap() {
                match m {
                    EngineMsg::Hello { .. } if !greeted => {
                        let w = EngineMsg::Welcome {
                            version: ENGINE_PROTOCOL_VERSION,
                            topk: 128,
                        };
                        s.write_all(&w.to_frame().unwrap()).await.unwrap();
                        greeted = true;
                    }
                    EngineMsg::Segment {
                        phase,
                        len,
                        candidates,
                        ..
                    } => current.push((
                        phase,
                        len,
                        candidates.iter().map(|c| (c.index, c.value.0)).collect(),
                    )),
                    EngineMsg::Finish { .. } => tx.send(std::mem::take(&mut current)).unwrap(),
                    other => panic!("unexpected {other:?}"),
                }
            }
        }
    });
    rx
}

fn mock(socket: PathBuf, mode: EngineMode, seed: u64) -> Config {
    Config {
        ttft: Duration::ZERO,
        token_interval: Duration::ZERO,
        toploc: Some(PluginConfig {
            socket,
            half_decode: false,
            preempt_after: None,
            mode,
        }),
        model_seed: seed,
        ..Config::default()
    }
}

/// A provider (mock engine of `seed`) answers `words` with up to `max` tokens; the case of it.
async fn answered(label: &str, seed: u64, words: &str, max: u32, proofs: bool) -> RecheckCase {
    let d = dir(&format!("prover-{label}"));
    let socket = d.join("p.sock");
    let mut rx = prove_receiver(&socket).await;
    let engine = spawn("127.0.0.1:0", mock(socket, EngineMode::Prove, seed))
        .await
        .unwrap();
    let messages = json!([{"role": "user", "content": words}]);
    let resp = Client::new()
        .unwrap()
        .post_with(
            &join(&engine.url(), "/v1/chat/completions"),
            "application/json",
            &[("x-request-id", &request_id_header(&[seed as u8; 32]))],
            json!({"model": "mock-model", "messages": messages, "max_tokens": max}).to_string(),
        )
        .await
        .unwrap();
    let reply: Value =
        serde_json::from_slice(&read_body(resp.into_body(), 1 << 20).await.unwrap()).unwrap();
    let segments = rx.recv().await.unwrap();
    let usage = CaseUsage {
        prompt_tokens: reply["usage"]["prompt_tokens"].as_u64().unwrap() as u32,
        completion_tokens: reply["usage"]["completion_tokens"].as_u64().unwrap() as u32,
    };
    let input = CalibrationInput {
        model: MODEL.into(),
        engine_model: "mock-model".into(),
        messages,
        output: reply["choices"][0]["message"]["content"]
            .as_str()
            .unwrap()
            .into(),
        finish_reason: reply["choices"][0]["finish_reason"]
            .as_str()
            .unwrap()
            .into(),
        usage,
        segments: proofs.then(|| {
            segments
                .into_iter()
                .map(|(phase, len, candidates)| InputSegment {
                    phase: if phase == Phase::Prefill {
                        "prefill"
                    } else {
                        "decode"
                    }
                    .into(),
                    len,
                    candidates,
                })
                .collect()
        }),
    };
    std::fs::remove_dir_all(&d).unwrap();
    case_from(input).unwrap()
}

/// An auditor whose verify-mode engine plays the model `seed`, once its plugin connected.
async fn auditor(label: &str, seed: u64) -> (Verifier, PathBuf) {
    logging::init(log::LevelFilter::Debug, Sink::Buffer(&LOGS));
    let d = dir(&format!("verifier-{label}"));
    let socket = d.join("v.sock");
    let rows = Rows::listen(&socket).unwrap();
    let engine = spawn("127.0.0.1:0", mock(socket, EngineMode::Verify, seed))
        .await
        .unwrap();
    // The mock's plugin connects on its first request; warm it up.
    let client = EngineClient::new(&engine.url()).unwrap();
    client
        .prefill("mock-model", &[1, 2], &"00".repeat(32))
        .await
        .unwrap();
    for _ in 0..100 {
        if rows.connections() > 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(rows.connections(), 1);
    let mut v = Verifier::new(client, rows);
    v.rows_wait = Duration::from_secs(2);
    (v, d)
}

const WORDS: &str = "the quick brown fox jumps over the lazy dog again and again";

// Scenarios "诚实提供者" and "因长度上限结束": 40 output tokens ending on the length limit
// give 1 + ⌈39 / 32⌉ = 3 chunks, all within the thresholds. No content in the logs (scenario
// "复核不记录内容").
#[tokio::test]
async fn an_honest_provider_passes() {
    let case = answered("honest", 7, &format!("{WORDS} {MARKER}"), 40, true).await;
    assert_eq!(case.finish_reason, "length");
    let (v, d) = auditor("honest", 7).await;
    let report = v.recheck(&case, QuantType::Bf16).await;
    assert_eq!(report.verdict, Outcome::Pass, "{report:?}");
    assert_eq!(report.chunks.len(), 3);
    assert!(
        report
            .chunks
            .iter()
            .all(|c| c.exp_mismatches == 0 && c.mant_err_sum == 0)
    );
    let logs = LOGS.lock().unwrap().join("\n");
    assert!(logs.contains("re-check"), "{logs}");
    assert!(!logs.contains(MARKER));
    assert!(!format!("{case:?}").contains(MARKER));
    std::fs::remove_dir_all(d).unwrap();
}

// Scenario "换了模型".
#[tokio::test]
async fn another_model_fails_on_a_chunk() {
    let case = answered("swap", 8, WORDS, 20, true).await;
    let (v, d) = auditor("swap", 7).await;
    let report = v.recheck(&case, QuantType::Bf16).await;
    assert!(
        matches!(
            report.verdict,
            Outcome::Fail(FailReason::Threshold {
                chunk: 0,
                metric: Metric::ExpMismatches
            })
        ),
        "{report:?}"
    );
    assert_eq!(report.outcome, "fail");
    assert!(report.reason.starts_with("chunk 0"));
    std::fs::remove_dir_all(d).unwrap();
}

// Scenarios "全零承诺", "int4 模型" and "证明与承诺不符", and receipts of another inference.
#[tokio::test]
async fn proofs_precision_and_inputs() {
    let (v, d) = auditor("inputs", 7).await;
    let no_proof = answered("noproof", 7, WORDS, 10, false).await;
    assert!(no_proof.toploc.is_none());
    assert_eq!(
        v.recheck(&no_proof, QuantType::Bf16).await.verdict,
        Outcome::Fail(FailReason::NoProof)
    );
    // An int4 model is never failed, not even without proofs.
    assert_eq!(
        v.recheck(&no_proof, QuantType::Int4).await.verdict,
        Outcome::Inconclusive(Inconclusive::UnsupportedPrecision)
    );
    // Proofs of another inference with the same counts.
    let case = answered("a", 7, WORDS, 10, true).await;
    let other = answered("b", 8, WORDS, 10, true).await;
    let mut swapped = case.clone();
    swapped.toploc.clone_from(&other.toploc);
    assert_eq!(
        v.recheck(&swapped, QuantType::Bf16).await.verdict,
        Outcome::Fail(FailReason::CommitmentMismatch)
    );
    // A receipt whose counts are not the answer's.
    let mut wrong = case.clone();
    wrong.usage.completion_tokens += 1;
    assert_eq!(
        v.recheck(&wrong, QuantType::Bf16).await.verdict,
        Outcome::Inconclusive(Inconclusive::InputMismatch)
    );
    std::fs::remove_dir_all(d).unwrap();
}

// Scenario "token 数不符", and finish reasons the rules do not cover.
#[tokio::test]
async fn tokens_that_cannot_be_recreated() {
    let (v, d) = auditor("tokens", 7).await;
    let case = answered("tokens", 7, WORDS, 12, true).await;
    let mut short = case.clone();
    let words: Vec<&str> = case.output.split_whitespace().collect();
    short.output = words[..words.len() - 2]
        .iter()
        .map(|w| format!("{w} "))
        .collect();
    assert_eq!(
        v.recheck(&short, QuantType::Bf16).await.verdict,
        Outcome::Inconclusive(Inconclusive::Tokens)
    );
    // With "stop" the text shows one token less than the count: 12 shown is one too many.
    let mut stop = case.clone();
    stop.finish_reason = "stop".into();
    assert_eq!(
        v.recheck(&stop, QuantType::Bf16).await.verdict,
        Outcome::Inconclusive(Inconclusive::Tokens)
    );
    let mut filtered = case.clone();
    filtered.finish_reason = "content_filter".into();
    assert_eq!(
        v.recheck(&filtered, QuantType::Bf16).await.verdict,
        Outcome::Inconclusive(Inconclusive::Tokens)
    );
    std::fs::remove_dir_all(d).unwrap();
}

// m6-toploc-gpu-calibration 1.5, spec engineering/ci-quality-gates "案例包被改动": a case
// changed after it left the prover (its prompt, its answer or its proofs) fails the re-check,
// and says why.
#[tokio::test]
async fn a_changed_case_fails() {
    let (v, d) = auditor("changed", 7).await;
    let case = answered("changed", 7, WORDS, 40, true).await;
    assert_eq!(
        v.recheck(&case, QuantType::Bf16).await.verdict,
        Outcome::Pass
    );
    // Another prompt word: the prefill chunk no longer matches.
    let mut prompt = case.clone();
    prompt.messages = json!([{"role": "user", "content": WORDS.replace("lazy", "sleepy")}]);
    let report = v.recheck(&prompt, QuantType::Bf16).await;
    assert_eq!(report.outcome, "fail", "{report:?}");
    assert!(report.reason.starts_with("chunk 0"), "{report:?}");
    // Another answer word (same count): a decode chunk no longer matches. (Not the last word:
    // no activation is computed for the last token, the engine never feeds it back.)
    let mut answer = case.clone();
    let first = case.output.split_whitespace().next().unwrap();
    answer.output = case.output.replacen(first, &format!("{first}x"), 1);
    let report = v.recheck(&answer, QuantType::Bf16).await;
    assert_eq!(report.outcome, "fail", "{report:?}");
    assert!(report.reason.starts_with("chunk "), "{report:?}");
    assert!(!report.reason.starts_with("chunk 0"), "{report:?}");
    // One proof byte flipped: the proofs no longer open the receipt's commitment.
    let mut proofs = case.clone();
    let mut bytes = hex::decode(case.toploc.as_deref().unwrap()).unwrap();
    let at = bytes.len() / 2;
    bytes[at] ^= 1;
    proofs.toploc = Some(hex::encode(bytes));
    assert_eq!(
        v.recheck(&proofs, QuantType::Bf16).await.verdict,
        Outcome::Fail(FailReason::CommitmentMismatch)
    );
    std::fs::remove_dir_all(d).unwrap();
}

// A re-check engine whose rows never arrive (its plugin sends elsewhere) is inconclusive.
#[tokio::test]
async fn missing_rows_are_an_engine_error() {
    let d = dir("norows");
    let rows = Rows::listen(&d.join("v.sock")).unwrap();
    let engine = spawn(
        "127.0.0.1:0",
        mock(d.join("elsewhere.sock"), EngineMode::Verify, 7),
    )
    .await
    .unwrap();
    let mut v = Verifier::new(EngineClient::new(&engine.url()).unwrap(), rows);
    v.rows_wait = Duration::from_millis(300);
    let case = answered("norows", 7, WORDS, 10, true).await;
    assert_eq!(
        v.recheck(&case, QuantType::Bf16).await.verdict,
        Outcome::Inconclusive(Inconclusive::Engine)
    );
    // An engine that is not there at all.
    let v = Verifier::new(
        EngineClient::new("http://127.0.0.1:9").unwrap(),
        Rows::listen(&d.join("w.sock")).unwrap(),
    );
    assert_eq!(
        v.recheck(&case, QuantType::Bf16).await.verdict,
        Outcome::Inconclusive(Inconclusive::Engine)
    );
    std::fs::remove_dir_all(d).unwrap();
}

// Scenario "复核程序拒绝证明模式": a prove-mode plugin gets a top-k of 0 and is not counted.
#[tokio::test]
async fn a_prove_mode_plugin_is_turned_away() {
    let d = dir("prove-plugin");
    let path = d.join("v.sock");
    let rows = Rows::listen(&path).unwrap();
    let mut stream = tokio::net::UnixStream::connect(&path).await.unwrap();
    let hello = EngineMsg::Hello {
        version: ENGINE_PROTOCOL_VERSION,
        hidden_size: 256,
        mode: EngineMode::Prove,
    };
    stream.write_all(&hello.to_frame().unwrap()).await.unwrap();
    let mut reply = Vec::new();
    stream.read_to_end(&mut reply).await.unwrap();
    assert_eq!(
        EngineMsg::decode(&reply[4..]).unwrap(),
        EngineMsg::Welcome {
            version: ENGINE_PROTOCOL_VERSION,
            topk: 0
        }
    );
    assert_eq!(rows.connections(), 0);
    std::fs::remove_dir_all(d).unwrap();
}

// m6-audit-chain, spec market/auditor-agent "审计证据" / "导出再复核": the exported evidence
// re-checks exactly as the case it came from, and the CLI prints the same commitment.
#[tokio::test]
async fn evidence_rechecks_as_its_case() {
    use ac_auditor::evidence::{case_of, evidence_of};
    let case = answered("evidence", 8, WORDS, 20, true).await;
    let (v, d) = auditor("evidence", 7).await;
    let original = v.recheck(&case, QuantType::Bf16).await;
    assert_eq!(original.outcome, "fail");

    let e = evidence_of(&case).unwrap();
    let rebuilt = case_of(&e, &case.engine_model).unwrap();
    let again = v.recheck(&rebuilt, QuantType::Bf16).await;
    assert_eq!(again.verdict, original.verdict);
    assert_eq!(again.chunks, original.chunks);

    let case_file = d.join("case.json");
    std::fs::write(&case_file, serde_json::to_string(&case).unwrap()).unwrap();
    let out = d.join("evidence.bin");
    let run = std::process::Command::new(env!("CARGO_BIN_EXE_ac-auditor"))
        .args(["evidence", "--case"])
        .arg(&case_file)
        .arg("--out")
        .arg(&out)
        .output()
        .unwrap();
    assert!(run.status.success());
    let stdout = String::from_utf8(run.stdout).unwrap();
    let printed: Value = serde_json::from_str(stdout.trim()).unwrap();
    assert_eq!(
        printed["commitment"],
        json!(hex::encode(e.commitment().unwrap()))
    );
    assert_eq!(std::fs::read(&out).unwrap(), e.to_bytes());
    assert!(!stdout.contains(MARKER));
    std::fs::remove_dir_all(d).unwrap();
}

// Scenario "承诺不符": a wrong commitment is reported without any engine (the engine URL and
// socket here lead nowhere, and the plugin wait would fail).
#[tokio::test]
async fn evidence_that_does_not_match_never_reaches_the_engine() {
    let case = answered("mismatch", 7, WORDS, 10, true).await;
    let d = dir("mismatch-cli");
    let file = d.join("evidence.bin");
    std::fs::write(
        &file,
        ac_auditor::evidence::evidence_of(&case).unwrap().to_bytes(),
    )
    .unwrap();
    let run = std::process::Command::new(env!("CARGO_BIN_EXE_ac-auditor"))
        .args(["recheck", "--evidence"])
        .arg(&file)
        .args([
            "--commitment",
            &"ab".repeat(32),
            "--engine-model",
            "mock-model",
        ])
        .args([
            "--engine",
            "http://127.0.0.1:9",
            "--quant",
            "bf16",
            "--connect-wait",
            "1",
        ])
        .arg("--socket")
        .arg(d.join("nowhere.sock"))
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    let line: Value = serde_json::from_slice(&run.stdout).unwrap();
    assert_eq!(line["outcome"], "mismatch");
    assert!(!d.join("nowhere.sock").exists());
    std::fs::remove_dir_all(d).unwrap();
}
