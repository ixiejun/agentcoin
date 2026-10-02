//! Evaluation and embedding units against the mock engine (m6-public-jobs task 6.3, design D12):
//! two engines of one model seed give equal summaries; another seed plays another model, whose
//! summaries the comparison rules v1 reject.

#![allow(clippy::unwrap_used)] // Test code.

use ac_mock_engine::Config;
use ac_primitives::market::public::{RULES_V1, agree};
use ac_primitives::market::work::JobKind;
use ac_worker::EngineClient;
use ac_worker::exec::{embed_unit, eval_unit};
use ac_worker::manifest::EvalItem;

const MODEL: &str = "mock-model";

async fn engine(seed: u64) -> EngineClient {
    let e = ac_mock_engine::spawn(
        "127.0.0.1:0",
        Config {
            model_seed: seed,
            ..Config::default()
        },
    )
    .await
    .unwrap();
    EngineClient::new(&e.url()).unwrap()
}

fn items() -> Vec<EvalItem> {
    (0..40)
        .map(|i| EvalItem {
            context: format!("Question number {i} asks which answer is right:"),
            choices: (0..4).map(|c| format!(" answer {c} of item {i}")).collect(),
        })
        .collect()
}

fn texts() -> Vec<String> {
    (0..20)
        .map(|i| format!("Text {i} for the embedding unit."))
        .collect()
}

#[tokio::test]
async fn one_model_agrees_with_itself_and_not_with_another() {
    let (a, b, other) = (engine(0).await, engine(0).await, engine(1).await);

    let eval = |e| async move { eval_unit(e, MODEL, &items()).await.unwrap() };
    let (ea, eb, eo) = (eval(&a).await, eval(&b).await, eval(&other).await);
    assert_eq!(ea, eb);
    assert_eq!(ea.summary.len(), 40);
    assert!(agree(JobKind::Eval, RULES_V1, &ea.summary, &eb.summary));
    assert!(!agree(JobKind::Eval, RULES_V1, &ea.summary, &eo.summary));

    let embed = |e| async move { embed_unit(e, MODEL, 7, &texts()).await.unwrap() };
    let (ma, mb, mo) = (embed(&a).await, embed(&b).await, embed(&other).await);
    assert_eq!(ma, mb);
    assert!(agree(JobKind::Embed, RULES_V1, &ma.summary, &mb.summary));
    assert!(!agree(JobKind::Embed, RULES_V1, &ma.summary, &mo.summary));
}
