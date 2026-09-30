//! Helpers shared by the market end-to-end tests (inference through the mock engine and
//! through a real vLLM): service processes, wallets, the OpenAI SDK client and polling.

// Each test binary uses its own subset of the helpers.
#![allow(dead_code)]

use std::io::{BufRead, BufReader, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use ac_e2e::TestNode;
use ac_runtime::AccountId;
use parity_scale_codec::Decode;

pub const DEV_MNEMONIC: &str = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon art";

#[macro_export]
macro_rules! require_e2e {
    () => {
        if !ac_e2e::enabled() {
            eprintln!("end-to-end test disabled; run with AC_E2E=1 after building the binaries");
            return;
        }
    };
}

pub fn bin(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/debug")
        .join(name)
}

pub fn run(cmd: &mut Command) -> String {
    let out = cmd.output().unwrap_or_else(|e| panic!("{cmd:?}: {e}"));
    assert!(
        out.status.success(),
        "{cmd:?} failed:\n{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}

pub fn field(output: &str, key: &str) -> String {
    output
        .lines()
        .find_map(|l| l.strip_prefix(&format!("{key}: ")))
        .unwrap_or_else(|| panic!("no {key} in:\n{output}"))
        .trim()
        .to_string()
}

/// A long-running service process, killed on drop; its stderr goes to a log file.
pub struct Proc {
    child: Child,
}

impl Proc {
    /// Starts `cmd` and waits for its `listening on <addr>` line.
    pub fn start(mut cmd: Command, log: PathBuf) -> (Self, String) {
        let err = std::fs::File::create(&log).unwrap();
        let mut child = cmd
            .stdout(Stdio::piped())
            .stderr(err)
            .spawn()
            .unwrap_or_else(|e| panic!("{cmd:?}: {e}"));
        let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
        let deadline = Instant::now() + Duration::from_secs(120);
        let addr = loop {
            assert!(
                Instant::now() < deadline,
                "{cmd:?} did not start; see {}",
                log.display()
            );
            match lines.next() {
                Some(Ok(l)) => {
                    if let Some(a) = l.strip_prefix("listening on ") {
                        break a
                            .trim()
                            .trim_start_matches("http://")
                            .trim_end_matches("/v1")
                            .to_string();
                    }
                }
                _ => panic!(
                    "{cmd:?} exited: {}",
                    std::fs::read_to_string(&log).unwrap_or_default()
                ),
            }
        };
        // Keep draining stdout so the process never blocks on a full pipe.
        std::thread::spawn(move || for _ in lines {});
        (Self { child }, addr)
    }
}

impl Proc {
    /// Starts `cmd` without waiting for anything (for programs that do not print
    /// `listening on`); stdout and stderr go to the log file.
    pub fn spawn(mut cmd: Command, log: &Path) -> Self {
        let out = std::fs::File::create(log).unwrap();
        let child = cmd
            .stdout(out.try_clone().unwrap())
            .stderr(out)
            .spawn()
            .unwrap_or_else(|e| panic!("{cmd:?}: {e}"));
        Self { child }
    }
}

impl Drop for Proc {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

pub struct W {
    pub file: PathBuf,
    pub password: PathBuf,
    pub node: String,
}

impl W {
    pub fn cmd(&self, args: &[&str]) -> Command {
        let mut cmd = Command::new(bin("ac-wallet"));
        cmd.args(args)
            .arg("--wallet")
            .arg(&self.file)
            .arg("--password-file")
            .arg(&self.password)
            .args(["--node", &self.node]);
        cmd
    }

    pub fn address(&self) -> String {
        run(Command::new(bin("ac-wallet"))
            .arg("address")
            .arg("--wallet")
            .arg(&self.file))
        .trim()
        .to_string()
    }
}

pub fn import_dev_wallet(base: &Path, node: &str) -> W {
    let password = base.join("password");
    std::fs::write(&password, "e2e-password\n").unwrap();
    let file = base.join("user.json");
    let mut import = Command::new(bin("ac-wallet"))
        .arg("import")
        .arg("--wallet")
        .arg(&file)
        .arg("--password-file")
        .arg(&password)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    writeln!(import.stdin.take().unwrap(), "{DEV_MNEMONIC}").unwrap();
    assert!(import.wait().unwrap().success());
    W {
        file,
        password,
        node: node.to_string(),
    }
}

pub fn funded_wallet(base: &Path, name: &str, from: &W, funds: &str) -> W {
    let w = W {
        file: base.join(format!("{name}.json")),
        password: from.password.clone(),
        node: from.node.clone(),
    };
    run(Command::new(bin("ac-wallet"))
        .arg("new")
        .arg("--wallet")
        .arg(&w.file)
        .arg("--password-file")
        .arg(&w.password)
        .stderr(Stdio::null()));
    run(&mut from.cmd(&["transfer", "--to", &w.address(), "--amount", funds]));
    w
}

pub async fn api<T: Decode>(node: &TestNode, method: &str, args: &[u8]) -> T {
    T::decode(&mut &node.state_call(method, args, None).await.unwrap()[..]).unwrap()
}

pub fn account(address: &str) -> AccountId {
    AccountId::new(ac_primitives::decode_address(address).unwrap())
}

pub fn python() -> String {
    std::env::var("AC_E2E_PYTHON").unwrap_or_else(|_| "python3".into())
}

/// Runs the OpenAI SDK client and returns its JSON report.
pub fn openai(base_url: &str, model: &str, runs: u32, marker: &str) -> serde_json::Value {
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("python/openai_client.py");
    let out = run(Command::new(python()).arg(script).args([
        "--base-url",
        base_url,
        "--model",
        model,
        "--runs",
        &runs.to_string(),
        "--marker",
        marker,
    ]));
    serde_json::from_str(&out).unwrap_or_else(|e| panic!("{e}: {out}"))
}

pub async fn wait_until<F, Fut>(what: &str, timeout: Duration, mut f: F)
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    let deadline = Instant::now() + timeout;
    while !f().await {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}
