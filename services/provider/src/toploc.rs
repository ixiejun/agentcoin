//! Engine-plugin candidates to TOPLOC proofs (spec `market/provider-agent` "接收引擎插件的候选并
//! 构造证明", design D4–D5 of m5-engine-toploc).
//!
//! The collector listens on a local Unix socket for the engine plugin (`market/engine-plugin`),
//! keeps the candidates of the requests the provider has forwarded, and builds a request's
//! proofs once the engine answered and the plugin marked the request finished. Candidates are
//! held in memory only, never written or logged.

use std::collections::BTreeMap;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ac_market_proto::Usage;
use ac_market_proto::engine::{
    ENGINE_PROTOCOL_VERSION, EngineMsg, EngineReader, market_request_id,
};
use ac_market_proto::toploc::{MARKET_PARAMS, ToplocProofs};
use ac_toploc::{Phase, Segment, build_proofs_from_candidates};
use anyhow::{Context, Result};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::Notify;

use crate::logging::TARGET;

/// How long to wait for the plugin's end marker after the engine answered.
pub const FINISH_WAIT: Duration = Duration::from_secs(2);
/// Requests whose candidates are kept at most this long.
pub const PENDING_TTL: Duration = Duration::from_secs(60);
/// Requests tracked at once.
pub const MAX_PENDING: usize = 4096;
/// Segments kept per request (a prefill in many steps plus the decode steps).
pub const MAX_SEGMENTS: usize = 70_000;

/// Why a request gets no proof (never carries content).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Missing {
    /// The request was not being collected (no plugin configured, `n > 1`, capacity).
    NotCollected,
    /// The plugin did not mark the request finished in time.
    Timeout,
    /// Segments do not fit the usage, or no segments at all.
    Incomplete,
    /// A prefill after decode steps: the engine preempted and recomputed the request.
    Recomputed,
    /// The candidates do not build proofs.
    Invalid,
}

impl Missing {
    /// Short name for logs and counters.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NotCollected => "not collected",
            Self::Timeout => "no end marker",
            Self::Incomplete => "incomplete segments",
            Self::Recomputed => "recomputed",
            Self::Invalid => "invalid candidates",
        }
    }
}

struct Pending {
    since: Instant,
    segments: Vec<Segment>,
    finished: bool,
    overflow: bool,
}

#[derive(Default)]
struct State {
    pending: BTreeMap<[u8; 32], Pending>,
    hidden_size: Option<u32>,
    connections: u64,
}

/// Collects the engine plugin's candidates and builds proofs.
pub struct Collector {
    path: PathBuf,
    state: Mutex<State>,
    notify: Notify,
    missing: AtomicU64,
}

impl Collector {
    /// Binds `path` (removing a stale socket file first, permissions 0600) and serves plugin
    /// connections in the background.
    ///
    /// # Errors
    ///
    /// If the socket cannot be bound.
    pub fn listen(path: &Path) -> Result<Arc<Self>> {
        if path.exists() {
            std::fs::remove_file(path)
                .with_context(|| format!("removing the stale socket {}", path.display()))?;
        }
        let listener = UnixListener::bind(path)
            .with_context(|| format!("binding the TOPLOC socket {}", path.display()))?;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
            .with_context(|| format!("restricting {}", path.display()))?;
        let this = Arc::new(Self {
            path: path.to_path_buf(),
            state: Mutex::new(State::default()),
            notify: Notify::new(),
            missing: AtomicU64::new(0),
        });
        let weak = Arc::downgrade(&this);
        tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    continue;
                };
                let Some(this) = weak.upgrade() else {
                    return;
                };
                tokio::spawn(async move { this.serve(stream).await });
            }
        });
        Ok(this)
    }

    /// The socket path.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Requests that got no proof so far.
    #[must_use]
    pub fn missing(&self) -> u64 {
        self.missing.load(Ordering::Relaxed)
    }

    /// Plugin connections accepted so far.
    #[must_use]
    pub fn connections(&self) -> u64 {
        self.state.lock().map_or(0, |s| s.connections)
    }

    /// Starts collecting a request before it is forwarded; candidates of other requests are
    /// dropped. Returns `false` at capacity.
    pub fn expect(&self, id: [u8; 32]) -> bool {
        let Ok(mut s) = self.state.lock() else {
            return false;
        };
        s.pending.retain(|_, p| p.since.elapsed() < PENDING_TTL);
        if s.pending.len() >= MAX_PENDING {
            return false;
        }
        s.pending.insert(
            id,
            Pending {
                since: Instant::now(),
                segments: Vec::new(),
                finished: false,
                overflow: false,
            },
        );
        true
    }

    /// Stops collecting a request (it failed).
    pub fn forget(&self, id: &[u8; 32]) {
        if let Ok(mut s) = self.state.lock() {
            s.pending.remove(id);
        }
    }

    /// Records that a request gets no proof.
    pub fn count_missing(&self, id: &[u8; 32], why: Missing) {
        self.missing.fetch_add(1, Ordering::Relaxed);
        log::info!(
            target: TARGET,
            "toploc_missing: request {} has no proof ({}); {} so far",
            hex::encode(id.get(..6).unwrap_or(id)),
            why.as_str(),
            self.missing()
        );
    }

    /// Waits (at most [`FINISH_WAIT`]) for the plugin to finish request `id`, then builds its
    /// proofs from the candidates, checked against the engine's `usage`.
    ///
    /// # Errors
    ///
    /// The reason there is no proof; the caller counts it with [`Collector::count_missing`].
    pub async fn take(&self, id: &[u8; 32], usage: Usage) -> Result<ToplocProofs, Missing> {
        let deadline = tokio::time::Instant::now() + FINISH_WAIT;
        let pending = loop {
            let notified = self.notify.notified();
            {
                let mut s = self.state.lock().map_err(|_| Missing::NotCollected)?;
                match s.pending.get(id) {
                    None => return Err(Missing::NotCollected),
                    Some(p) if p.finished => {
                        let hidden = s.hidden_size;
                        let p = s.pending.remove(id).ok_or(Missing::NotCollected)?;
                        break (p, hidden);
                    }
                    Some(_) => {}
                }
            }
            if tokio::time::timeout_at(deadline, notified).await.is_err() {
                self.forget(id);
                return Err(Missing::Timeout);
            }
        };
        let (p, hidden) = pending;
        let hidden = hidden.ok_or(Missing::Incomplete)?;
        check_segments(&p, hidden, usage)?;
        let segments = p.segments;
        let built = tokio::task::spawn_blocking(move || {
            build_proofs_from_candidates(&segments, &MARKET_PARAMS)
        })
        .await
        .map_err(|_| Missing::Invalid)?
        .map_err(|e| match e {
            ac_toploc::Error::SegmentOrder => Missing::Recomputed,
            _ => Missing::Invalid,
        })?;
        Ok(ToplocProofs::new(&MARKET_PARAMS, &built))
    }

    async fn serve(self: Arc<Self>, mut stream: UnixStream) {
        let mut reader = EngineReader::new();
        let mut buf = vec![0u8; 64 * 1024];
        let mut greeted = false;
        loop {
            let n = match stream.read(&mut buf).await {
                Ok(0) | Err(_) => return,
                Ok(n) => n,
            };
            reader.push(buf.get(..n).unwrap_or_default());
            loop {
                let msg = match reader.next_msg() {
                    Ok(Some(m)) => m,
                    Ok(None) => break,
                    Err(_) => {
                        log::warn!(target: TARGET, "the TOPLOC plugin sent a malformed frame; closing");
                        return;
                    }
                };
                if !greeted {
                    let EngineMsg::Hello {
                        version,
                        hidden_size,
                    } = msg
                    else {
                        return;
                    };
                    let welcome = EngineMsg::Welcome {
                        version: ENGINE_PROTOCOL_VERSION,
                        topk: u16::try_from(MARKET_PARAMS.topk).unwrap_or(u16::MAX),
                    };
                    let Ok(frame) = welcome.to_frame() else {
                        return;
                    };
                    if stream.write_all(&frame).await.is_err() {
                        return;
                    }
                    if version != ENGINE_PROTOCOL_VERSION {
                        log::warn!(
                            target: TARGET,
                            "the TOPLOC plugin speaks protocol version {version}, this provider {ENGINE_PROTOCOL_VERSION}"
                        );
                        return;
                    }
                    if let Ok(mut s) = self.state.lock() {
                        s.hidden_size = Some(hidden_size);
                        s.connections = s.connections.saturating_add(1);
                    }
                    log::info!(target: TARGET, "TOPLOC plugin connected (hidden size {hidden_size})");
                    greeted = true;
                    continue;
                }
                self.accept(msg);
            }
        }
    }

    fn accept(&self, msg: EngineMsg) {
        let Ok(mut s) = self.state.lock() else {
            return;
        };
        match msg {
            EngineMsg::Segment {
                request,
                phase,
                len,
                candidates,
            } => {
                let Some(p) = market_request_id(&request).and_then(|id| s.pending.get_mut(&id))
                else {
                    return;
                };
                if p.finished || p.segments.len() >= MAX_SEGMENTS {
                    p.overflow = true;
                    return;
                }
                p.segments.push(Segment {
                    phase,
                    len,
                    candidates,
                });
            }
            EngineMsg::Finish { request } => {
                if let Some(p) = market_request_id(&request).and_then(|id| s.pending.get_mut(&id)) {
                    p.finished = true;
                    drop(s);
                    self.notify.notify_waiters();
                }
            }
            _ => {}
        }
    }
}

/// The segments must fit the engine's usage: the prefill values are the prompt tokens times the
/// hidden size, and there is one decode segment of `hidden` values per output token but the
/// last (spec: 解码段数 = 输出 token 数 − 1).
fn check_segments(p: &Pending, hidden: u32, usage: Usage) -> Result<(), Missing> {
    if p.overflow || p.segments.is_empty() {
        return Err(Missing::Incomplete);
    }
    let prefill: u64 = p
        .segments
        .iter()
        .take_while(|s| s.phase == Phase::Prefill)
        .map(|s| u64::from(s.len))
        .sum();
    let decode: Vec<&Segment> = p
        .segments
        .iter()
        .skip_while(|s| s.phase == Phase::Prefill)
        .collect();
    if decode.iter().any(|s| s.phase == Phase::Prefill) {
        return Err(Missing::Recomputed);
    }
    let expected_prefill = u64::from(usage.prompt_tokens).saturating_mul(u64::from(hidden));
    let expected_decode = usize::try_from(usage.completion_tokens.saturating_sub(1))
        .map_err(|_| Missing::Incomplete)?;
    if prefill != expected_prefill
        || decode.len() != expected_decode
        || decode.iter().any(|s| s.len != hidden)
    {
        return Err(Missing::Incomplete);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ac_market_proto::engine::request_id_header;
    use ac_toploc::{Bf16, build_proofs, top_k_candidates};

    const HIDDEN: u32 = 64;

    fn values(seed: u32, n: u32) -> Vec<Bf16> {
        (0..n)
            .map(|i| {
                let x = i
                    .wrapping_mul(2_654_435_761)
                    .wrapping_add(seed.wrapping_mul(97));
                Bf16(u16::try_from(x % 0x7f00).unwrap())
            })
            .collect()
    }

    struct Plugin {
        stream: UnixStream,
    }

    impl Plugin {
        async fn connect(path: &Path) -> Self {
            let mut stream = UnixStream::connect(path).await.unwrap();
            let hello = EngineMsg::Hello {
                version: ENGINE_PROTOCOL_VERSION,
                hidden_size: HIDDEN,
            };
            stream.write_all(&hello.to_frame().unwrap()).await.unwrap();
            let mut reply = [0u8; 8];
            stream.read_exact(&mut reply).await.unwrap();
            assert_eq!(
                EngineMsg::decode(&reply[4..]).unwrap(),
                EngineMsg::Welcome {
                    version: ENGINE_PROTOCOL_VERSION,
                    topk: 128
                }
            );
            Self { stream }
        }

        async fn send(&mut self, msg: EngineMsg) {
            self.stream
                .write_all(&msg.to_frame().unwrap())
                .await
                .unwrap();
        }

        async fn segment(&mut self, id: &[u8; 32], phase: Phase, v: &[Bf16]) {
            self.send(EngineMsg::Segment {
                request: format!("chatcmpl-{}-0badcafe", request_id_header(id)),
                phase,
                len: u32::try_from(v.len()).unwrap(),
                candidates: top_k_candidates(v, 128),
            })
            .await;
        }

        async fn finish(&mut self, id: &[u8; 32]) {
            self.send(EngineMsg::Finish {
                request: format!("chatcmpl-{}-0badcafe", request_id_header(id)),
            })
            .await;
        }
    }

    fn socket() -> (tempdir::Dir, PathBuf) {
        let dir = tempdir::Dir::new();
        let path = dir.0.join("toploc.sock");
        (dir, path)
    }

    mod tempdir {
        pub struct Dir(pub std::path::PathBuf);
        impl Dir {
            pub fn new() -> Self {
                let p = std::env::temp_dir().join(format!(
                    "ac-provider-toploc-{}-{}",
                    std::process::id(),
                    rand_suffix()
                ));
                std::fs::create_dir_all(&p).unwrap();
                Self(p)
            }
        }
        impl Drop for Dir {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        fn rand_suffix() -> u64 {
            use std::sync::atomic::{AtomicU64, Ordering};
            static N: AtomicU64 = AtomicU64::new(0);
            N.fetch_add(1, Ordering::Relaxed)
        }
    }

    fn usage(prompt: u32, completion: u32) -> Usage {
        Usage {
            prompt_tokens: prompt,
            completion_tokens: completion,
        }
    }

    /// A request of `prompt` tokens (prefill in two steps) and `out` output tokens.
    async fn run(plugin: &mut Plugin, id: &[u8; 32], prompt: u32, out: u32) -> Vec<Vec<Bf16>> {
        let pre = values(1, prompt * HIDDEN);
        let (a, b) = pre.split_at(usize::try_from(HIDDEN).unwrap());
        plugin.segment(id, Phase::Prefill, a).await;
        plugin.segment(id, Phase::Prefill, b).await;
        let mut acts = vec![pre.clone()];
        for s in 0..out.saturating_sub(1) {
            let d = values(s + 2, HIDDEN);
            plugin.segment(id, Phase::Decode, &d).await;
            acts.push(d);
        }
        plugin.finish(id).await;
        acts
    }

    // Scenario "带证明的收据".
    #[tokio::test]
    async fn builds_the_proofs_of_a_finished_request() {
        let (_dir, path) = socket();
        let c = Collector::listen(&path).unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        let mut plugin = Plugin::connect(&path).await;
        let id = [7u8; 32];
        assert!(c.expect(id));
        let acts = run(&mut plugin, &id, 5, 40).await;
        let proofs = c.take(&id, usage(5, 40)).await.unwrap();
        assert_eq!(proofs.proofs.len(), 3);
        let whole: Vec<&[Bf16]> = acts.iter().map(Vec::as_slice).collect();
        let expected = build_proofs(&whole, &MARKET_PARAMS).unwrap();
        assert_eq!(proofs, ToplocProofs::new(&MARKET_PARAMS, &expected));
        // Taken once.
        assert_eq!(c.take(&id, usage(5, 40)).await, Err(Missing::NotCollected));
    }

    // Scenario "插件不在线": nothing arrives before the deadline.
    #[tokio::test]
    async fn no_plugin_times_out() {
        let (_dir, path) = socket();
        let c = Collector::listen(&path).unwrap();
        let id = [1u8; 32];
        assert!(c.expect(id));
        let started = Instant::now();
        assert_eq!(c.take(&id, usage(3, 3)).await, Err(Missing::Timeout));
        assert!(started.elapsed() >= FINISH_WAIT);
        c.count_missing(&id, Missing::Timeout);
        assert_eq!(c.missing(), 1);
    }

    // Scenario "步骤数不符".
    #[tokio::test]
    async fn too_few_decode_steps_give_no_proof() {
        let (_dir, path) = socket();
        let c = Collector::listen(&path).unwrap();
        let mut plugin = Plugin::connect(&path).await;
        let id = [2u8; 32];
        assert!(c.expect(id));
        run(&mut plugin, &id, 4, 6).await;
        assert_eq!(c.take(&id, usage(4, 10)).await, Err(Missing::Incomplete));
        // The prompt size must fit too.
        assert!(c.expect(id));
        run(&mut plugin, &id, 4, 6).await;
        assert_eq!(c.take(&id, usage(5, 6)).await, Err(Missing::Incomplete));
    }

    #[tokio::test]
    async fn recomputed_requests_and_unknown_ids() {
        let (_dir, path) = socket();
        let c = Collector::listen(&path).unwrap();
        let mut plugin = Plugin::connect(&path).await;
        // A prefill after decode steps: preempted and recomputed.
        let id = [3u8; 32];
        assert!(c.expect(id));
        let pre = values(1, 2 * HIDDEN);
        plugin.segment(&id, Phase::Prefill, &pre).await;
        plugin.segment(&id, Phase::Decode, &values(2, HIDDEN)).await;
        plugin.segment(&id, Phase::Prefill, &pre).await;
        plugin.finish(&id).await;
        assert_eq!(c.take(&id, usage(2, 2)).await, Err(Missing::Recomputed));
        // Candidates of a request the provider did not forward are dropped.
        let other = [4u8; 32];
        run(&mut plugin, &other, 2, 2).await;
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(
            c.take(&other, usage(2, 2)).await,
            Err(Missing::NotCollected)
        );
    }

    #[tokio::test]
    async fn a_plugin_on_another_version_is_turned_away() {
        let (_dir, path) = socket();
        let c = Collector::listen(&path).unwrap();
        let mut stream = UnixStream::connect(&path).await.unwrap();
        let hello = EngineMsg::Hello {
            version: 9,
            hidden_size: HIDDEN,
        };
        stream.write_all(&hello.to_frame().unwrap()).await.unwrap();
        let mut reply = Vec::new();
        stream.read_to_end(&mut reply).await.unwrap();
        // The Welcome tells the plugin our version, then the connection closes.
        assert_eq!(reply.len(), 8);
        assert_eq!(c.connections(), 0);
    }

    #[tokio::test]
    async fn a_stale_socket_file_is_replaced() {
        let (_dir, path) = socket();
        std::fs::write(&path, b"stale").unwrap();
        let c = Collector::listen(&path).unwrap();
        let _plugin = Plugin::connect(c.path()).await;
    }
}
