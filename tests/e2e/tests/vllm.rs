//! Inference through a real vLLM with the TOPLOC plugin (m5-engine-toploc 7.3; spec
//! `engineering/ci-quality-gates` item 14): on a development chain a model, a provider and a
//! gateway are registered and a user escrows credits; `vllm serve` (CPU build, bfloat16, prefix
//! caching off, the plugin enabled on the provider's socket), `ac-provider`, `ac-gateway` and
//! `ac-wallet market serve` run as processes; the official OpenAI Python SDK calls through the
//! proxy, streamed and not. Every receipt commits to TOPLOC proofs, which the gateway and the
//! proxy check before billing and paying and the provider keeps; no prompt or output appears in
//! the logs or data of the services or of vLLM.
//!
//! Enabled with `AC_E2E=1 AC_VLLM_E2E=1`; needs the binaries of the inference test, a Python
//! with vLLM, the plugin and `tests/e2e/python/requirements.txt` (`scripts/setup-vllm-cpu.sh`),
//! and `AC_VLLM_MODEL` / `AC_VLLM_REVISION` (set by that script under GitHub Actions). The `vllm`
//! executable is taken from `AC_VLLM_BIN` (default `vllm`).

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    missing_docs
)]

mod common;

use std::process::Command;
use std::time::Duration;

use ac_e2e::{Testnet, TestnetConfig, free_port};
use common::{Proc, bin, field, funded_wallet, import_dev_wallet, openai, run, wait_until};

const START: Duration = Duration::from_secs(180);
const MODEL_NAME: &str = "Qwen2.5-0.5B-Instruct";
const ENGINE_MODEL: &str = "engine-model";
const MARKER: &str = "pangolin-marker-6a2d";

#[tokio::test(flavor = "multi_thread")]
async fn inference_through_vllm_with_toploc_proofs() {
    require_e2e!();
    if std::env::var("AC_VLLM_E2E").is_err() {
        eprintln!("vLLM end-to-end test disabled; run with AC_VLLM_E2E=1 (see the file header)");
        return;
    }
    let hf_model = std::env::var("AC_VLLM_MODEL").expect("AC_VLLM_MODEL");
    let revision = std::env::var("AC_VLLM_REVISION").expect("AC_VLLM_REVISION");
    let vllm = std::env::var("AC_VLLM_BIN").unwrap_or_else(|_| "vllm".into());

    let config = TestnetConfig {
        label: "vllm".to_string(),
        chain: "dev".to_string(),
        authorities: 1,
        args: Vec::new(),
    };
    let net = Testnet::start_with(config, START).await.unwrap();
    net.wait_all(&[0], 1, START).await.unwrap();
    let node = &net.nodes[0];
    let base = net.base.clone();
    let user = import_dev_wallet(&base, &node.url);
    let data = |n: &str| base.join(format!("data-{n}"));

    // The model and the provider.
    let manifest = base.join("model.json");
    std::fs::write(
        &manifest,
        format!(r#"{{"name":"{MODEL_NAME}","arch":"qwen2","quant":"bf16","shards":["0x{}"],"licenseTag":"apache-2.0"}}"#, "02".repeat(32)),
    )
    .unwrap();
    let model = field(
        &run(&mut user.cmd(&[
            "market",
            "model",
            "register",
            "--file",
            manifest.to_str().unwrap(),
        ])),
        "model id",
    );
    let wallet = funded_wallet(&base, "provider", &user, "1000");
    let kem_file = base.join("provider-kem.json");
    let kem = field(
        &run(Command::new(bin("ac-provider"))
            .args(["keygen", "--out"])
            .arg(&kem_file)
            .arg("--password-file")
            .arg(&wallet.password)),
        "kem key",
    );
    let port = free_port().unwrap();
    run(&mut wallet.cmd(&[
        "market",
        "provider",
        "register",
        "--tier",
        "t2",
        "--endpoint",
        &format!("http://127.0.0.1:{port}"),
        "--kem-key",
        &kem,
        "--model",
        &format!("{model}:0.1:0.2"),
    ]));

    // The provider listens on the TOPLOC socket; vLLM's plugin connects to it.
    let socket = base.join("toploc.sock");
    let engine_port = free_port().unwrap();
    let _provider = Proc::start(
        {
            let mut c = Command::new(bin("ac-provider"));
            c.arg("run")
                .arg("--wallet")
                .arg(&wallet.file)
                .arg("--password-file")
                .arg(&wallet.password)
                .arg("--kem-key")
                .arg(&kem_file)
                .args([
                    "--node",
                    &node.url,
                    "--engine",
                    &format!("http://127.0.0.1:{engine_port}"),
                ])
                .args(["--model", &format!("{model}={ENGINE_MODEL}")])
                .args(["--listen", &format!("127.0.0.1:{port}")])
                .arg("--data-dir")
                .arg(data("provider"))
                .arg("--toploc-socket")
                .arg(&socket)
                .args(["--log-level", "debug"]);
            c
        },
        base.join("provider.log"),
    );
    let _engine = Proc::spawn(
        {
            let mut c = Command::new(&vllm);
            c.args(["serve", &hf_model, "--revision", &revision])
                .args(["--served-model-name", ENGINE_MODEL])
                .args(["--dtype", "bfloat16", "--no-enable-prefix-caching"])
                .args([
                    "--max-model-len",
                    "1024",
                    "--port",
                    &engine_port.to_string(),
                ])
                .env("AGENTCOIN_TOPLOC_SOCKET", &socket)
                .env("VLLM_USE_V2_MODEL_RUNNER", "0");
            c
        },
        &base.join("vllm.log"),
    );
    let client = ac_wallet::http::Client::new().unwrap();
    let models = format!("http://127.0.0.1:{engine_port}/v1/models");
    wait_until("vLLM to serve", Duration::from_secs(900), || {
        let (client, models) = (&client, &models);
        async move { client.get_bytes(models, 1 << 20).await.is_ok() }
    })
    .await;
    wait_until("the plugin to connect", Duration::from_secs(60), || async {
        std::fs::read_to_string(base.join("provider.log"))
            .is_ok_and(|l| l.contains("TOPLOC plugin connected"))
    })
    .await;

    // The gateway and the proxy.
    let gateway = funded_wallet(&base, "gateway", &user, "5000");
    let gw_kem = base.join("gateway-kem.json");
    run(Command::new(bin("ac-gateway"))
        .args(["keygen", "--out"])
        .arg(&gw_kem)
        .arg("--password-file")
        .arg(&gateway.password));
    let gw_port = free_port().unwrap();
    run(&mut gateway.cmd(&[
        "market",
        "gateway",
        "register",
        "--endpoint",
        &format!("http://127.0.0.1:{gw_port}"),
        "--fee-bps",
        "300",
    ]));
    let _gateway = Proc::start(
        {
            let mut c = Command::new(bin("ac-gateway"));
            c.arg("run")
                .arg("--wallet")
                .arg(&gateway.file)
                .arg("--password-file")
                .arg(&gateway.password)
                .arg("--kem-key")
                .arg(&gw_kem)
                .args([
                    "--node",
                    &node.url,
                    "--listen",
                    &format!("127.0.0.1:{gw_port}"),
                ])
                .args(["--report-interval", "5", "--log-level", "debug"])
                .arg("--data-dir")
                .arg(data("gateway"));
            c
        },
        base.join("gateway.log"),
    );
    let g = gateway.address();
    run(&mut user.cmd(&[
        "market",
        "escrow",
        "deposit",
        "--gateway",
        &g,
        "--amount",
        "10",
    ]));
    let proxy_port = free_port().unwrap();
    let (_proxy, proxy_addr) = Proc::start(
        {
            let mut c = user.cmd(&[
                "market",
                "serve",
                "--gateway",
                &g,
                "--listen",
                &format!("127.0.0.1:{proxy_port}"),
            ]);
            c.arg("--state").arg(base.join("serve-state.json"));
            c
        },
        base.join("proxy.log"),
    );

    // Two streamed requests and one whole completion through the market.
    let report = openai(&format!("http://{proxy_addr}/v1"), MODEL_NAME, 2, MARKER);
    for s in report["streamed"].as_array().unwrap() {
        assert!(s["usage"]["completion_tokens"].as_u64().unwrap() > 0, "{s}");
    }
    assert!(
        report["completion"]["usage"]["prompt_tokens"]
            .as_u64()
            .unwrap()
            > 0
    );

    // Every receipt commits to proofs (checked by the gateway and the proxy, which paid), and
    // the provider keeps them with the co-signed receipts.
    wait_until(
        "3 co-signed receipts with proofs",
        Duration::from_secs(60),
        || async {
            let files: Vec<String> = std::fs::read_dir(data("provider").join("receipts"))
                .into_iter()
                .flatten()
                .flatten()
                .filter_map(|e| std::fs::read_to_string(e.path()).ok())
                .collect();
            files.len() == 3 && files.iter().all(|t| t.contains("\"toploc\""))
        },
    )
    .await;
    let paid: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(base.join("serve-state.json")).unwrap())
            .unwrap();
    assert!(paid["paid"].as_str().unwrap().parse::<u128>().unwrap() > 0);
    let provider_log = std::fs::read_to_string(base.join("provider.log")).unwrap();
    assert!(
        !provider_log.contains("toploc_missing"),
        "a request had no proof"
    );

    // No request content in any log or data file, vLLM's included.
    let mut paths: Vec<std::path::PathBuf> =
        ["provider.log", "gateway.log", "proxy.log", "vllm.log"]
            .iter()
            .map(|f| base.join(f))
            .collect();
    let mut stack = vec![data("provider"), data("gateway")];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
            if e.path().is_dir() {
                stack.push(e.path());
            } else if e.path().extension().is_none_or(|x| x != "sock") {
                paths.push(e.path());
            }
        }
    }
    for p in paths {
        let text = String::from_utf8_lossy(&std::fs::read(&p).unwrap_or_default()).into_owned();
        assert!(
            !text.contains(MARKER),
            "{} contains request content",
            p.display()
        );
    }
}
