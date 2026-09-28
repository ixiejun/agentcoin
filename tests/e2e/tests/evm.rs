//! EVM acceptance (m4-evm task 7.4): a development chain, the eth-RPC adapter, `ac-wallet evm`
//! as the only signer, and Foundry (`forge`, `cast`) as the Ethereum tool. Enabled with
//! `AC_E2E=1`; needs `cargo build -p ac-node -p ac-wallet -p ac-eth-rpc` and Foundry on `PATH`
//! (or `FORGE`/`CAST`). `AC_FORGE_OFFLINE=1` builds with already installed compilers only.

// Test code: failures should abort the test.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, missing_docs)]

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use ac_e2e::{Testnet, TestnetConfig, enabled, free_port};
use jsonrpsee::core::client::ClientT;
use jsonrpsee::core::params::ArrayParams;
use jsonrpsee::http_client::{HttpClient, HttpClientBuilder};
use serde_json::{Value, json};

const START: Duration = Duration::from_secs(180);
const DEV_MNEMONIC: &str = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon art";
const SUPPLY: &str = "1000000000000000000000";
const BOB: &str = "0x00000000000000000000000000000000000000b0";
const CAROL: &str = "0x00000000000000000000000000000000000000c0";
/// keccak256("Transfer(address,address,uint256)").
const TRANSFER_TOPIC: &str = "0xddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef";
/// BLAKE3 of the empty input (official test vector).
const BLAKE3_EMPTY: &str = "0xaf1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262";
/// Foundry's first development key (public test key; any Ethereum key is refused).
const ETHEREUM_TEST_KEY: &str =
    "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80";

macro_rules! require_e2e {
    () => {
        if !enabled() {
            eprintln!(
                "end-to-end test disabled; run with AC_E2E=1 after building ac-node, ac-wallet \
                 and ac-eth-rpc, with Foundry installed"
            );
            return;
        }
    };
}

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn binary(env: &str, name: &str) -> PathBuf {
    std::env::var_os(env).map_or_else(|| repo().join("target/debug").join(name), PathBuf::from)
}

fn tool(env: &str, name: &str) -> String {
    std::env::var(env).unwrap_or_else(|_| name.to_string())
}

/// Runs `cmd`, requires success and returns standard output.
fn run(cmd: &mut Command) -> String {
    let out = cmd.output().unwrap_or_else(|e| panic!("{cmd:?}: {e}"));
    assert!(
        out.status.success(),
        "{cmd:?} failed:\n{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}

/// Runs `cmd`, requires failure and returns (stdout, stderr).
fn run_failing(cmd: &mut Command) -> (String, String) {
    let out = cmd.output().unwrap_or_else(|e| panic!("{cmd:?}: {e}"));
    assert!(!out.status.success(), "{cmd:?} unexpectedly succeeded");
    (
        String::from_utf8(out.stdout).unwrap(),
        String::from_utf8(out.stderr).unwrap(),
    )
}

/// The value of a `key: value` output line.
fn field(output: &str, key: &str) -> String {
    output
        .lines()
        .find_map(|l| l.strip_prefix(&format!("{key}: ")))
        .unwrap_or_else(|| panic!("no {key} in:\n{output}"))
        .trim()
        .to_string()
}

fn hex_u128(value: &Value) -> u128 {
    u128::from_str_radix(value.as_str().unwrap().trim_start_matches("0x"), 16).unwrap()
}

fn topic(address: &str) -> String {
    format!("0x{:0>64}", address.trim_start_matches("0x").to_lowercase())
}

/// The running adapter; killed on drop.
struct Adapter(Child);

impl Drop for Adapter {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

struct Env {
    node_url: String,
    rpc_url: String,
    rpc: HttpClient,
    wallet: PathBuf,
    password: PathBuf,
    me: String,
}

impl Env {
    /// `ac-wallet evm <sub>` with the wallet, passphrase and node options, then `args`.
    fn evm(&self, sub: &[&str], args: &[&str]) -> Command {
        let mut cmd = Command::new(binary("AC_WALLET", "ac-wallet"));
        cmd.arg("evm")
            .args(sub)
            .arg("--wallet")
            .arg(&self.wallet)
            .arg("--password-file")
            .arg(&self.password)
            .args(["--node", &self.node_url])
            .args(args)
            .env("RUST_BACKTRACE", "0");
        cmd
    }

    fn cast(&self, args: &[&str]) -> String {
        run(Command::new(tool("CAST", "cast"))
            .args(args)
            .args(["--rpc-url", &self.rpc_url]))
        .trim()
        .to_string()
    }

    /// A `cast` command that does not talk to a node.
    fn cast_offline(&self, args: &[&str]) -> String {
        run(Command::new(tool("CAST", "cast")).args(args))
            .trim()
            .to_string()
    }

    /// `cast call` of a function returning one value; the value without cast's annotation.
    fn call(&self, to: &str, sig: &str, args: &[&str]) -> String {
        let mut all = vec!["call", to, sig];
        all.extend_from_slice(args);
        self.cast(&all)
            .split_whitespace()
            .next()
            .unwrap()
            .to_string()
    }

    async fn eth(&self, method: &str, params: Value) -> Result<Value, String> {
        let mut array = ArrayParams::new();
        for p in params.as_array().unwrap() {
            array.insert(p).unwrap();
        }
        self.rpc
            .request::<Value, _>(method, array)
            .await
            .map_err(|e| e.to_string())
    }

    async fn receipt(&self, hash: &str) -> Value {
        let deadline = Instant::now() + START;
        loop {
            let receipt = self
                .eth("eth_getTransactionReceipt", json!([hash]))
                .await
                .unwrap();
            if !receipt.is_null() {
                return receipt;
            }
            assert!(Instant::now() < deadline, "no receipt for {hash}");
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }

    async fn balance(&self) -> u128 {
        hex_u128(
            &self
                .eth("eth_getBalance", json!([self.me, "latest"]))
                .await
                .unwrap(),
        )
    }
}

fn forge_build(dir: &Path) {
    let mut cmd = Command::new(tool("FORGE", "forge"));
    cmd.arg("build").current_dir(dir);
    if std::env::var("AC_FORGE_OFFLINE").is_ok_and(|v| v == "1") {
        cmd.arg("--offline");
    }
    run(&mut cmd);
}

async fn start(net: &Testnet) -> (Env, Adapter) {
    let node = &net.nodes[0];
    let port = free_port().unwrap();
    let log = std::fs::File::create(net.base.join("eth-rpc.log")).unwrap();
    let adapter = Adapter(
        Command::new(binary("AC_ETH_RPC", "ac-eth-rpc"))
            .args(["--node-url", &node.url])
            .args(["--listen", &format!("127.0.0.1:{port}")])
            .args(["--log-level", "debug"])
            .stdout(Stdio::null())
            .stderr(log)
            .spawn()
            .unwrap(),
    );
    let rpc_url = format!("http://127.0.0.1:{port}");
    let rpc = HttpClientBuilder::default().build(&rpc_url).unwrap();
    let deadline = Instant::now() + START;
    while rpc
        .request::<String, _>("eth_chainId", ArrayParams::new())
        .await
        .is_err()
    {
        assert!(Instant::now() < deadline, "the adapter did not start");
        tokio::time::sleep(Duration::from_millis(500)).await;
    }

    let wallet = net.base.join("dev-wallet.json");
    let password = net.base.join("password");
    std::fs::write(&password, "e2e-password\n").unwrap();
    let mut import = Command::new(binary("AC_WALLET", "ac-wallet"))
        .arg("import")
        .arg("--wallet")
        .arg(&wallet)
        .arg("--password-file")
        .arg(&password)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    writeln!(import.stdin.take().unwrap(), "{DEV_MNEMONIC}").unwrap();
    assert!(import.wait().unwrap().success());
    let me = run(Command::new(binary("AC_WALLET", "ac-wallet"))
        .args(["evm", "address", "--wallet"])
        .arg(&wallet))
    .trim()
    .to_string();
    (
        Env {
            node_url: node.url.clone(),
            rpc_url,
            rpc,
            wallet,
            password,
            me,
        },
        adapter,
    )
}

// Scenarios "Foundry 读取", "部署 ERC-20", "部署回执", "按主题过滤", "回滚的调用不提交", "回滚回执",
// "估算足够", "raw 交易可经适配器提交", "提交原生合约交易", "拒绝以太坊签名交易",
// "签名供 pq_verify 使用", "finalized 标签", "区块哈希一致" and "节点接受合约区块".
#[tokio::test(flavor = "multi_thread")]
async fn contracts_through_the_wallet_and_the_adapter() {
    require_e2e!();
    let contracts = repo().join("contracts");
    forge_build(&contracts);
    let artifact = |name: &str| {
        contracts
            .join(format!("out/{name}.sol/{name}.json"))
            .display()
            .to_string()
    };

    let config = TestnetConfig {
        label: "evm".to_string(),
        chain: "dev".to_string(),
        authorities: 1,
        args: Vec::new(),
    };
    let net = Testnet::start_with(config, START).await.unwrap();
    net.wait_all(&[0], 1, START).await.unwrap();
    let (env, _adapter) = start(&net).await;

    // Foundry reads the chain through the adapter.
    assert_eq!(env.cast(&["chain-id"]), "4403");
    assert!(env.cast(&["block-number"]).parse::<u64>().unwrap() >= 1);
    assert!(env.cast(&["balance", &env.me]).parse::<u128>().unwrap() > 0);

    // Deploy an ERC-20 with the wallet; its receipt and state read back with Foundry.
    let deployed = run(&mut env.evm(
        &["deploy"],
        &[
            "--artifact",
            &artifact("ERC20"),
            "--constructor",
            "constructor(string,string,uint256)",
            "Token",
            "TKN",
            SUPPLY,
        ],
    ));
    let token = field(&deployed, "contract");
    let receipt = env.receipt(&field(&deployed, "transaction")).await;
    assert_eq!(receipt["status"], "0x1");
    assert_eq!(receipt["to"], Value::Null);
    assert_eq!(
        receipt["contractAddress"].as_str().unwrap(),
        token.to_lowercase()
    );
    assert!(env.cast(&["code", &token]).len() > 2);
    assert_eq!(
        env.call(&token, "balanceOf(address)(uint256)", &[&env.me]),
        SUPPLY
    );
    assert_eq!(env.call(&token, "symbol()(string)", &[]), "\"TKN\"");

    // Transfers and a topic-filtered log query.
    for (to, amount) in [(BOB, "5"), (CAROL, "7")] {
        let sent = run(&mut env.evm(
            &["send"],
            &[
                "--to",
                &token,
                "--sig",
                "transfer(address,uint256)",
                to,
                amount,
            ],
        ));
        assert_eq!(field(&sent, "status"), "success");
    }
    let logs = env
        .eth(
            "eth_getLogs",
            json!([{"fromBlock": "earliest", "toBlock": "latest", "address": token,
                    "topics": [TRANSFER_TOPIC, null, topic(BOB)]}]),
        )
        .await
        .unwrap();
    let logs = logs.as_array().unwrap();
    assert_eq!(logs.len(), 1, "{logs:?}");
    assert_eq!(hex_u128(&logs[0]["data"]), 5);
    assert_eq!(logs[0]["topics"][1], topic(&env.me));
    assert_eq!(
        env.call(&token, "balanceOf(address)(uint256)", &[CAROL]),
        "7"
    );

    // A transfer above the balance: refused after the dry run, nothing submitted.
    let nonce = env.cast(&["nonce", &env.me]);
    let (_, error) = run_failing(&mut env.evm(
        &["send"],
        &[
            "--to",
            &token,
            "--sig",
            "transfer(address,uint256)",
            BOB,
            &format!("{SUPPLY}0"),
        ],
    ));
    assert!(error.contains("ERC20: insufficient balance"), "{error}");
    assert!(error.contains("nothing was submitted"), "{error}");
    assert_eq!(env.cast(&["nonce", &env.me]), nonce);

    // Forced: it lands, reverts, and pays its fee, which the receipt reproduces.
    let before = env.balance().await;
    let (stdout, _) = run_failing(&mut env.evm(
        &["send"],
        &[
            "--force",
            "--to",
            &token,
            "--sig",
            "transfer(address,uint256)",
            BOB,
            &format!("{SUPPLY}0"),
        ],
    ));
    let receipt = env.receipt(&field(&stdout, "transaction")).await;
    assert_eq!(receipt["status"], "0x0");
    assert_eq!(receipt["logs"], json!([]));
    let fee = before - env.balance().await;
    let price = hex_u128(&receipt["effectiveGasPrice"]);
    let charged = hex_u128(&receipt["gasUsed"]) * price;
    assert!(
        fee > 0 && charged >= fee && charged - fee < price,
        "fee {fee}, gasUsed × price {charged}"
    );

    // A call limited to eth_estimateGas's figure (1 gas = 10,000 ps of ref time) succeeds.
    let data = env.cast_offline(&["calldata", "transfer(address,uint256)", CAROL, "1"]);
    let gas = hex_u128(
        &env.eth(
            "eth_estimateGas",
            json!([{"from": env.me, "to": token, "data": data}]),
        )
        .await
        .unwrap(),
    );
    let ref_time = (gas * 10_000).to_string();
    let sent = run(&mut env.evm(
        &["send"],
        &[
            "--ref-time-limit",
            &ref_time,
            "--to",
            &token,
            "--data",
            &data,
        ],
    ));
    assert_eq!(field(&sent, "status"), "success");

    // A raw transaction relayed by the adapter keeps the wallet's hash.
    let raw = run(&mut env.evm(
        &["raw", "send"],
        &[
            "--to",
            &token,
            "--sig",
            "transfer(address,uint256)",
            CAROL,
            "2",
        ],
    ));
    let hash = env
        .eth("eth_sendRawTransaction", json!([field(&raw, "raw")]))
        .await
        .unwrap();
    assert_eq!(hash.as_str().unwrap(), field(&raw, "transaction"));
    assert_eq!(env.receipt(hash.as_str().unwrap()).await["status"], "0x1");
    assert_eq!(
        env.call(&token, "balanceOf(address)(uint256)", &[CAROL]),
        "10"
    );

    // An Ethereum-signed transaction is refused and never reaches the pool.
    let signed = env.cast(&[
        "mktx",
        "--private-key",
        ETHEREUM_TEST_KEY,
        "--nonce",
        "0",
        "--gas-limit",
        "21000",
        "--gas-price",
        "1000000000",
        "--priority-gas-price",
        "1",
        BOB,
    ]);
    let error = env
        .eth("eth_sendRawTransaction", json!([signed]))
        .await
        .unwrap_err();
    assert!(
        error.contains("-32000") && error.contains("ML-DSA"),
        "{error}"
    );
    let node = net.nodes[0].rpc().unwrap();
    let pending: Vec<String> = node
        .request("author_pendingExtrinsics", ArrayParams::new())
        .await
        .unwrap();
    assert!(pending.is_empty(), "{pending:?}");

    // A wallet message signature is accepted by pq_verify from a contract.
    let demo = field(
        &run(&mut env.evm(&["deploy"], &["--artifact", &artifact("PqVerifyDemo")])),
        "contract",
    );
    let signed = run(Command::new(binary("AC_WALLET", "ac-wallet"))
        .args(["evm", "sign-message", "--wallet"])
        .arg(&env.wallet)
        .arg("--password-file")
        .arg(&env.password)
        .args(["--message", "hello agentcoin"]));
    let (alg, pk, msg, sig) = (
        field(&signed, "alg"),
        field(&signed, "public key"),
        field(&signed, "message"),
        field(&signed, "signature"),
    );
    let check = "check(uint8,bytes,bytes,bytes)(bool)";
    assert_eq!(env.call(&demo, check, &[&alg, &pk, &msg, &sig]), "true");
    assert_eq!(env.call(&demo, check, &[&alg, &pk, "0x00", &sig]), "false");
    assert_eq!(
        env.call(&demo, "digest(bytes)(bytes32)", &["0x"]),
        BLAKE3_EMPTY
    );
    let accepted = run(&mut env.evm(
        &["send"],
        &[
            "--to",
            &demo,
            "--sig",
            "accept(uint8,bytes,bytes,bytes)",
            &alg,
            &pk,
            &msg,
            &sig,
        ],
    ));
    assert_eq!(field(&accepted, "status"), "success");
    let accepted = env
        .eth(
            "eth_getLogs",
            json!([{"fromBlock": "earliest", "address": demo}]),
        )
        .await
        .unwrap();
    assert_eq!(accepted.as_array().unwrap().len(), 1);

    // `finalized` is AC-BFT's finalized block.
    let deadline = Instant::now() + START;
    loop {
        let before = net.nodes[0].finalized().await.unwrap().0;
        let block = env
            .eth("eth_getBlockByNumber", json!(["finalized", false]))
            .await
            .unwrap();
        let after = net.nodes[0].finalized().await.unwrap().0;
        let tagged = u64::try_from(hex_u128(&block["number"])).unwrap();
        if before == after {
            assert_eq!(tagged, before);
            break;
        }
        assert!(Instant::now() < deadline, "finality kept moving");
    }

    // A block's hash is what BLOCKHASH returns for it: dry-run, from a funded account (a
    // deployment pays a storage deposit), init code that returns blockhash(n) (PUSH8 n,
    // BLOCKHASH, MSTORE at 0, RETURN 32 bytes).
    let latest = hex_u128(&env.eth("eth_blockNumber", json!([])).await.unwrap());
    let n = u64::try_from(latest).unwrap() - 2;
    let code = format!("0x67{n:016x}4060005260206000f3");
    let returned = env
        .eth(
            "eth_call",
            json!([{"from": env.me, "data": code}, "latest"]),
        )
        .await
        .unwrap();
    let block = env
        .eth("eth_getBlockByNumber", json!([format!("{n:#x}"), false]))
        .await
        .unwrap();
    assert_eq!(returned, block["hash"]);

    // Every contract block was accepted: finality reaches the last one and the node never
    // refused a block (supply check, spec node/invariants).
    let last = net.nodes[0].height().await.unwrap();
    let deadline = Instant::now() + START;
    while net.nodes[0].finalized().await.unwrap().0 < last {
        assert!(Instant::now() < deadline, "not finalized");
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    let log = std::fs::read_to_string(net.nodes[0].log_path()).unwrap();
    assert!(!log.contains("rejecting block"), "a block was rejected");
    assert!(!log.contains("constitution invariant violated"));
}
